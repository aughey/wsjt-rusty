# Current processing architecture

This describes the implementation at source revision
`fc37e41aec07b094a2061d181ccd67d7f212354a`, inspected on 2026-11-22.
It documents existing behavior, with emphasis on browser receive processing.
It does not propose a replacement architecture. Function names below identify
the implementation points; links are relative to this document.

## Execution and communication model

The browser application has a main-thread Rust/WASM UI, an AudioWorklet capture
processor, and two JavaScript module Web Workers, each loading its own copy of
the Rust/WASM receiver. The workers have the roles **slot** and **recovery**.
The same generated WASM module runs in the window and workers;
[`lib.rs`](../crates/web/src/lib.rs) mounts Leptos only when a window exists.

There is no Rust broadcast channel, MPSC channel, Tokio runtime, Rayon pool,
shared-memory PCM ring, or Rust thread connecting receive stages. The actual
mechanisms are:

- Browser `MessageChannel` / `MessagePort` messages carry PCM and consumption
  credits between the worklet and slot worker, and between slot and recovery.
- Worker `postMessage` events deliver results and status to the window.
- `wasm-bindgen` exposes synchronous Rust methods to worker JavaScript and
  connects JavaScript capture functions to the UI's Rust code.
- Synchronous function calls and mutable owned state connect DSP, windowing,
  decoding, and result collection inside each receiver.
- On the main thread, a retained `Closure<dyn FnMut(String)>` receives events;
  ordinary Rust calls update Leptos signals. `Effect` and reactive view closures
  respond to state changes. These signals are UI state, not audio queues.
- `Rc<RefCell<Monitor>>`, `Rc<Capture>`, and weak references manage local session
  ownership. `spawn_local`, JavaScript promises, and `JsFuture` handle asynchronous
  startup/stop/device operations on the local event loop. They do not run FT8
  decoding concurrently within a worker.

The `Worker` struct inside the headless `AudioStream` is a per-protocol state
holder, **not** a thread or browser Worker. `AudioDecoder: Send` permits moving
a decoder between threads in a caller, but `AudioStream` itself calls its
decoders sequentially and synchronously.

## 1. Input capture and the first split

[`capture-runtime.js`](../crates/web/src/capture-runtime.js), `openCapture`,
creates an `AudioContext` and requests a mono microphone stream with echo
cancellation, noise suppression, and automatic gain control disabled. The
context's actual sample rate is passed to both receivers; input is not assumed
to be 12 kHz.

The `MediaStreamAudioSourceNode` connects to two consumers:

1. An `AnalyserNode` retaining a 16,384-sample time-domain snapshot for display.
2. An `AudioWorkletNode` running `wsjt-capture` for continuous PCM delivery.

The worklet connects to the audio destination to keep the graph rendering, but
outputs silence. Receive monitoring is not implemented by playing the input
through that node.

[`capture-worklet.js`](../crates/web/capture-worklet.js), `CaptureProcessor`,
copies the first input channel's samples into a `Float32Array` of length
`ceil(sampleRate / 4)`. Thus a normal transport packet is approximately a quarter
second: 12,000 samples at 48 kHz. The packet contains `kind: 'samples'`, the
absolute AudioContext `frame` of its first sample, and `samples`. Its ArrayBuffer
is transferred to the slot worker through a message port, and the worklet
allocates the next buffer. Stop can send a shorter final packet.

These packets are transport units, not independent FT8 frames. The worklet
does no resampling, FFT, filtering, or decoding. It verifies successive render
blocks against `currentFrame`; a discontinuity or disappearance after input has
started ends capture instead of silently inserting silence.

## 2. Transport fan-out and overload handling

`openCapture` starts two instances of
[`decoder-worker.js`](../crates/web/decoder-worker.js). It supplies a worklet
port to the slot worker and another message channel between slot and recovery.
Both workers must report ready before the capture graph starts delivering PCM.
Worker startup has a 30-second timeout.

For each packet, the slot worker's `receive` handler:

1. Verifies its frame offset is contiguous.
2. Registers the packet in an `outstanding` map, keyed by its first frame.
3. Posts a structured clone of the packet to recovery. This is a copy, not an
   ArrayBuffer transfer or shared PCM allocation.
4. Calls its own WASM `DecoderEngine.push` synchronously.
5. Posts the returned update to the window and records slot consumption.

Recovery receives the same packet stream, calls its own synchronous engine,
posts its update to the window, and sends a credit back to the slot worker.
Only after both lane credits exist does the slot worker delete that ledger
entry and return one credit to the worklet. A credit means processing the
packet has completed, including any decode attempt triggered by that push.

The worklet counts uncredited packets, including packets being processed. Its
limit corresponds to 60 seconds of normal-sized packets. The slot ledger also
checks at most 60 seconds of samples and 240 outstanding packets. This is one
aggregate in-flight policy, not separate allowances of 60 seconds per lane.
PCM copies and separate receiver histories still consume additional memory.

Capture cannot pause the physical audio source while decoding catches up.
Instead, exceeding the bound stops capture with an explicit error and retains
only the contiguous delivered prefix. It does not discard an old packet and
continue across an unreported gap.

The lanes execute independently once recovery has received a packet. A long
recovery decode cannot occupy the slot worker's event loop. However, the slot
worker is the forwarding point: while it is inside a synchronous decode it
cannot forward subsequent packets or handle new priority messages. Recovery
also stops accepting new work on its event loop while its own decode runs.
The worklet can continue capturing during these calls until the credit bound
is reached. Independence does not guarantee a particular completion deadline.

## 3. Sample clocks and Rust receiver ownership

Capture computes one anchor:
`epoch = Date.now() - AudioContext.currentTime * 1000`.
At the first packet each worker constructs its engine with
`start_utc_ms = epoch + firstFrame / sampleRate * 1000`.
Subsequent engine offsets are `packet.frame - firstFrame`.

[`worker.rs`](../crates/web/src/worker.rs), `DecoderEngine`, is a thin
`wasm-bindgen` wrapper over
[`receiver.rs`](../crates/web/src/receiver.rs), `Receiver`. Each lane owns:

- One `StreamingDownsampler` converting continuous device-rate PCM to 12 kHz.
- One `AudioStream` containing one FT8 `ReportingDecoder` / `Ft8Decoder`.
- Its own capture origin, input sample counter, next slot-attempt time, and
  finished flag.
- Its own retained FT8 callsign/hash context, audio history, and overlap
  deduplication state. These are not shared between lanes.

`Receiver::push` requires the input offset to equal its received-sample count
and rejects non-finite samples. It resamples, pushes the resulting samples into
the stream, advances the input count, and checks scheduling. The reported capture
end is the origin plus received input samples divided by the input rate. It is
a processed-audio watermark, not the wall time at which the result arrives.

[`StreamingDownsampler`](../crates/dsp/src/lib.rs) retains windowed-sinc filter
history and fractional sampling phase across pushes in a `VecDeque`. It does
not resample each packet as a separate recording. At unequal rates it waits
for right-hand filter support, approximately 32 output samples, before emitting
the newest output; `finish` emits its boundary tail. At 12 kHz it copies samples.
Its history is bounded by filter support. Input rates below 12 kHz are rejected.

UTC timestamps follow sample counts after the initial anchor. They do not move
with later computer-clock adjustments. Physical input latency and clock drift
are not calibrated. UI arrival times are separately stamped with `Date.now()`.

## 4. Accumulation and the two decode schedules

The reusable implementation is
[`decoder.rs`](../crates/decoder/src/decoder.rs), `AudioStream`. Each internal
`Worker` owns a `Box<dyn AudioDecoder>`, `Vec<f32>` audio buffer, absolute sample
origin, and `VecDeque<Decode>` of recent emissions.

Pushes are internally subdivided into at most 4,096 samples before appending to
each protocol buffer. FT8 declares a 13-second minimum attempted length, a
30-second normal window, and a 15-second hop, all at 12 kHz. After each push the
retained buffer is shorter than 30 seconds; it can briefly reach a window plus
an internal append. History is not retained for the entire capture.

### Slot lane

The slot role uses `AudioStream::push_buffered`. This appends audio and advances
the same 30-second/15-second-hop buffer bookkeeping, but skips automatic window
decoding. At each processed input end crossing UTC slot +13.5 seconds,
`Receiver::push` calls `decode_pending_with_observer(16 * SAMPLE_RATE)`.

That call takes up to the latest 16 seconds **available in the retained buffer**,
requires at least 13 seconds, and decodes without consuming the buffer or moving
its origin. It is not a separately stored file or an exact 15-second slot cut.
The buffer can contain less than 16 seconds after hop advancement or startup.
The next threshold is then calculated strictly after the current processed end.

This scheduling is driven by audio pushes, not a timer interrupt at +13.5.
Packet boundaries, resampler support, worker backlog, and decode duration affect
when computation actually starts and ends. If a push crosses multiple thresholds,
the code makes one pending attempt and calculates the next future threshold;
it does not loop through all missed UTC slot deadlines.

### Recovery lane

The recovery role uses normal `AudioStream::push_with_observer`. Each time its
buffer reaches 30 seconds, it synchronously decodes the first 30 seconds, drains
15 seconds, advances the origin, and continues. The first full attempt requires
30 seconds; later windows overlap by 15 seconds. These windows are relative to
capture start, not aligned to UTC. Recovery skips the extra +13.5 slot attempts.

Both lanes use the same default FT8 search, 100–4000 audio Hz and two configured
passes. The `Combined` receiver role remains available and performs both schedules
in one receiver, but `openCapture` explicitly selects separate slot/recovery roles.

## 5. What happens inside an FT8 attempt

`ReportingDecoder` brackets the actual decode call with activity callbacks;
`Ft8Decoder` adapts headless FT8 results into the common `Decode` envelope.
[`ft8/src/lib.rs`](../crates/ft8/src/lib.rs),
`decode_with_context_and_observer`, executes this work synchronously:

1. Copy the attempted audio into a residual buffer and search Costas
   synchronization candidates across time and the configured passband.
2. Optionally reorder retained candidates around queued station frequencies
   within ±60 Hz, strongest synchronization peaks first there. Other candidates
   remain in the search.
3. For each candidate, use FFT-based downconversion to 200 complex samples/s,
   refine timing/frequency, and form the 79 symbols' tone measurements.
4. Form soft bit metrics and run the FT8 LDPC decoder, including its CRC checks
   and ordered-statistics fallback. Apply plausibility/quality checks and unpack
   accepted 77-bit messages using retained `Messages` context.
5. Notify the observer for a newly accepted transmission. Continue searching
   the remaining candidates.
6. Between passes, subtract accepted signals from the local residual and search
   again. A pass with no new accepted results ends the pass loop early.

An attempt returns individual transmissions, not conversations. Each result
contains message text, raw payload, SNR, lowest-tone audio frequency, and start
time relative to that attempted window. No conversation state controls this DSP
path. Algorithm adaptations and unported sensitivity paths are detailed in
[`ft8.md`](ft8.md).

`AudioStream::Worker::decode_window` adds the retained origin plus pending-window
offset exactly once. Results then have start seconds relative to the first input
sample. `Receiver` adds the UTC origin and a nominal 12.64-second end time, creating
`TimedDecode`. Negative stream start times can represent leading partial frames.

## 6. Early events, batches, and deduplication

Inside Rust, observers are direct `FnMut` calls on the decoding stack; there is
no observer queue. `worker.rs` turns them into worker messages:

- `decode-started` and `decode-completed` report lane and attempt duration.
- `decode-found` carries one `TimedDecode` while the attempt is still computing.
- After the synchronous `engine.push` returns JSON, worker JavaScript parses it
  and posts `update`, including that push's result batch and audio watermark.

The window's capture runtime maps recovery updates to `recovery-results` with
decodes and a separate `recovery_end_ms`. Slot updates drive primary processed
audio progress; slow recovery does not replace it. Events are stringified for
the Rust capture callback, then deserialized by `app.rs`. PCM does not use JSON.

There are several distinct duplicate checks:

- The FT8 pass wrapper suppresses repeated payload/time/frequency results across
  passes at less than 0.3 seconds and 6 Hz, with additional candidate-local checks.
- `AudioStream` suppresses matching payloads at less than 0.35 seconds and 4 Hz
  for FT8, covering overlapping attempts and observer-versus-returned results.
  Its recent cache is pruned before attempts against retained origin minus
  10 seconds.
- `app.rs`'s `ingest` suppresses matching payloads at less than 350 ms and 4 Hz
  against the retained UI message history. This merges both lanes, early events,
  final push batches, and final-flush results. History is capped at 500 rows, so
  this is not a permanent session-wide identity index.

The FT8 batch routine can resolve hashes again after finishing an attempt, but
the incremental stream path keeps its first emission and deduplicates the later
batch. Earlier event text is not rewritten. Each lane learns its own hash context;
neither lane updates the other's previously emitted text.

## 7. Display processing is a separate snapshot path

[`app.rs`](../crates/web/src/app.rs), `animate`, runs a requestAnimationFrame
loop on the main thread while Stations or Spectrum is visible and capture is
running. It calls `AnalyserNode.get_float_time_domain_data` into a reusable
16,384-sample vector. The waveform draws the latest 2,048 samples. The waterfall
uses the full snapshot at device rate, independently of worker normalization.

[`spectrum.rs`](../crates/web/src/spectrum.rs) applies a Hann window and RustFFT,
then pools peak bin levels into 1,200 horizontal pixels over 100–4000 Hz.
These are FFT-bin dBFS levels, not decoder SNR. Automatic leveling estimates a
smoothed median background; manual floor control and a 50 dB color range affect
display only.

[`waterfall.rs`](../crates/web/src/waterfall.rs) scrolls a 1,200 × 600 canvas
at nominal 100 ms row intervals, representing 60 seconds. It paints one current
snapshot at the newest row; skipped animation rows are blank rather than
reconstructed. There is no continuous waterfall PCM queue. A throttled or hidden
view does not reconstruct every spectrum later from decoder history.

Row time is the capture AudioContext-derived time minus half the FFT snapshot
duration. `annotations.rs` projects decode start/end and lowest-tone frequency
onto these FFT-center coordinates, clips to visible time/passband, and adds tone
padding for a nominal 50 Hz rectangle. Boxes read decoded rows and display clock;
they do not feed detections back to decoding. Stop freezes the display reference
so final-flush results can still be inspected.

There is one additional consumer of actual spectrum rows:
[`open_frequency.rs`](../crates/web/src/open_frequency.rs), `Occupancy`, keeps
up to 600 rows/60 seconds of per-pixel excess above each row's median, plus running
totals. It estimates a quiet transmit footprint within 100–3000 Hz. This uses raw
FFT levels before display coloring and feeds optional transmit placement, not
receive decoding. During local TX and until an entire receive-only snapshot is
available afterward, the waterfall scrolls blank rows and excludes those spectra
from occupancy. This gate does not stop or replace worker PCM capture.

## 8. From transmissions to conversations and station state

The common presentation boundary is the `ingest` closure in `app.rs`. For each
incoming event/batch it performs the following synchronous updates:

1. Deduplicate against message history, stamp first arrival with `Date.now()`,
   sort history by descending signal start time then frequency, and retain 500 rows.
2. Sort fresh rows by signal start time and call `Conversations::update`.
3. If Stations is visible and auto-add is enabled, consider newly heard CQs for
   addition to the station queue. This also requires loaded contact history.
4. Call `StationQueue::hear` and `record_arrival` for each fresh row.

No downstream stage subscribes to a Rust broadcast stream. `ingest` explicitly
calls each consumer. Leptos signals expose resulting state to the views; a
transmission list, waterfall annotations, conversation cards, and station cards
are different presentations/models over these updates.

### Receive-only conversation tracker

[`conversations.rs`](../crates/web/src/conversations.rs) contains a plain Rust
`Conversations` model stored in a Leptos signal. It has no worker, timer, audio
buffer, or decoder. `message_kind.rs` first checks a valid 77-bit payload with
ordinary exchange type `i3` 1, 2, or 4, then categorizes rendered text as CQ,
locator/reply, report, R-report, RRR, or RR73/73. Other forms stay in message
history but do not become ordinary conversation evidence.

For a CQ, the tracker extracts the transmitting callsign and initially has no
partner. For directed messages it reads recipient then sender, rejects unresolved
or invalid callsigns and self-pairs, and matches an unordered callsign pair within
the retained contact's time interval. It chooses the nearest observed interval
when several contacts match. A directed exchange can adopt an existing CQ from
either participant; a late earlier CQ can join a pair. A new locator/report after
a later sign-off can start another contact rather than reopening the old one.
This grouping does not use frequency or SNR as an identity key.

The tracker records heard messages, heard senders, monotonic furthest progress,
and six stage summaries: CQ, callback, report, roger-report, acknowledgment, closing
73. Retries contribute to a stage count, and each summary retains the latest
message at that stage. Late evidence can fill earlier gaps. RR73 provides
acknowledgment and terminal progress while leaving a closing 73 optional.
Participant/direction and missing-stage estimates are marked as inferred; no
missing transmission is manufactured as received evidence.

Each update ingests its batch before advancing `now_ms` monotonically and
expiring entries. With a slot update, `ingest` supplies the processed capture end.
With an individual or recovery event, it uses the newest fresh decode end, or
the existing tracker clock if there is none. Thus conversation aging follows
processed audio/evidence rather than the UI's one-second wall-clock display.
Late recovery does not turn that clock backward.

Nonterminal contacts become stale 60 seconds after last heard frame end and
expire after another 45 seconds. Sign-off contacts expire 45 seconds after last
heard frame end. The tracker holds at most 200 contacts and 64 detailed messages
per contact, plus six summaries that survive detail trimming. At capacity it
replaces the least recently heard contact. Cards retain stable cell indices
through ordinary updates and neighboring expiry; explicit sorts reassign cells.

### Station queue and the feedback path

[`contact_queue.rs`](../crates/web/src/contact_queue.rs), `StationQueue`, is
separate state from the receive-only board. Selecting a conversation queues its
caller and imports its bounded received history/stage summaries; it does not start audio.
The queue retains up to 100 stations, 12 received evidence messages per station,
and its own bounded `Conversations` tracker for both-direction stage display.
That tracker is fed for relevant queued calls independently of board expiry.
Local successful playback is recorded separately from received evidence.

Fresh decoded messages can update CQ freshness, frequency/SNR, busy/partner state,
and the active exchange. Only appropriate ordinary replies addressed from the
partner to our callsign advance that exchange. Late recovery can fill evidence
and recent archived contact logs without automatically qualifying as a fresh CQ.

`transmit_ui.rs` uses a main-thread 100 ms interval across application pages to
check playback, queue plans, and armed transmit scheduling; per-second checks
handle sorting/expiry. This timer is distinct from receive decode scheduling.
The zero-dependency [`manager::decide`](../crates/conversations/src/manager.rs)
returns Listen/Start/Transmit/End; [`session::Contact`](../crates/conversations/src/session.rs)
uses the shared conversation decision to progress validated partner evidence.
Selection, slot turns, receive-readiness gates and attempt budgets live there.
The web queue stores facts and executes the policy, revalidating reserved messages
before keying and after PTT acknowledgment. The Stations view shows that decision
and separates active from waiting conversations.
Audio generation, output, and serialized radio/PTT control are separate paths.
Durable attempt/evidence writes use a separate contact-log worker and IndexedDB,
as described in [`contact-log.md`](contact-log.md); it is not another PCM stage.

Queue changes also trigger a Leptos `Effect` in `app.rs`: it obtains
`StationQueue::priorities`, sends changed frequencies to both decoder workers,
and each engine updates its decoder's priority list. `Ft8Decoder` filters to
finite in-passband values and retains at most 100. This is an explicit control
feedback path that changes candidate order; it does not narrow the passband,
replace audio, or interrupt an already running synchronous attempt.

## 9. Stop, restart, navigation, and failure

Normal Stop asks the worklet to send its partial packet and then `finish` in
port order. When the worklet reports stopped, the runtime releases tracks,
disconnects input nodes, and closes the input context. The slot worker forwards
finish to recovery. Each receiver flushes the resampler tail into `AudioStream`
and then calls stream finish to attempt a retained tail of at least 13 seconds.
The tail push uses normal automatic stream ingestion even for the slot role;
EOF processing is not restricted to another scheduled +13.5 attempt.

The runtime waits for both workers' finished messages, combines their final
decode batches, emits `finished`, and terminates the workers. Incremental events
from final decoding can arrive before that combined batch. Rust finish is
idempotent and pushes after finish are rejected.

Startup cancellation releases late-granted microphone tracks. Monitor generations
and weak references reject callbacks from replaced/disposed sessions. Unmount
and abort terminate both workers without waiting for normal tail decoding;
dropping `Capture` aborts its runtime. Worker failures abort the session, whereas
input disconnection/suspension requests a contiguous final flush where possible.

A new Start clears message rows, board conversations, waterfall clock, and
queue observations, and creates new receivers/contexts. Selected queue stations
remain. Stop retains visible received results, with final events still able to
update them. Tab navigation keeps capture and receive processing alive but
suspends hidden visualization work; station scheduling is active only in Stations.

## 10. Relationship to headless and CLI processing

The browser reuses the headless DSP and FT8 adapter rather than implementing a
second protocol decoder. The dependency direction is browser to framework to
protocol/DSP crates. Capture, DOM, UTC presentation, device ownership, conversation
correlation, and network/browser transports are outside those headless libraries.

The CLI in [`main.rs`](../crates/cli/src/main.rs) normalizes WAV input through
`wsjt-audio`, or converts mono 12 kHz PCM16LE stdin, then calls `AudioStream` and
prints text/JSONL results. Multiple configured modes have separate retained
windows but are called sequentially by `AudioStream`. `--realtime` is FT8 stdin
only: it uses `push_buffered` and, at each system-clock UTC slot + 13.5 s, calls
`decode_pending_with_observer` on the latest 16 seconds. Those attempts do not
run the automatic 30-second windows; EOF still flushes the retained tail.
CLI/library WSPR has its
own adapter/context and 108-second minimum, 132-second window, and 16-second hop;
the browser receiver currently constructs FT8 only.

The reusable stream handles arbitrary chunks, bounded overlapping history,
stream-relative origins, duplicate suppression, observers, and EOF. Browser
message ports, two-worker lane fan-out, UTC slot attempts, visualization,
conversation models, and station automation are additional application layers.
