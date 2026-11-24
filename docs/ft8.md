# FT8 port notes

The source is the sibling `wsjtx` checkout, revision
`967c85a6119110788b0df3782cf796485b2cee14` (working-tree source hashes are recorded
in the repository root `UPSTREAM-SHA256.txt`). No existing FT8 implementation was
substituted. No WSJT-X binary, C++, Fortran, FFTW, FFI decoder, or external decoder
process is built or invoked. All receive processing here is Rust; RustFFT
supplies the FFT primitive, `libm` the error function, and `hound` WAV I/O.

| Rust module | WSJT-X source routines/tables |
| --- | --- |
| `crates/ft8/src/lib.rs` | `lib/ft8/sync8.f90`, `ft8_downsample.f90`, `sync8d.f90`, `ft8b.f90`, `normalizebmet` |
| `crates/ft8/src/ldpc.rs` | `bpdecode174_91.f90`, `decode174_91.f90`, `osd174_91.f90`, `encode174_91.f90`, `lib/crc14.cpp` |
| `crates/ft8/src/tables.rs` | `ldpc_174_91_c_parity.f90`, `ldpc_174_91_c_generator.f90` |
| `crates/ft8/src/message.rs`, `crates/ft8/src/message_tables.rs` | `lib/77bit/packjt77.f90`: receive unpacking, callsigns, hashing, grids, contest tables |
| `crates/ft8/src/transmit.rs` | `lib/77bit/packjt77.f90`: restricted `pack28` / `pack77_1` standard transmit packing |
| `crates/ft8/src/waveform.rs` | `gen_ft8wave.f90`, `lib/ft2/gfsk_pulse.f90`, `subtractft8.f90` |
| `crates/audio/src/lib.rs`, `crates/dsp/src/lib.rs` | Rust WAV input/channel averaging and general windowed-sinc resampling |

The Costas pattern, Gray mapping, LDPC matrices, five soft metrics, CRC
polynomial, frequency taper, fine synchronization, GFSK pulse and subtraction
filter come from WSJT-X. Algorithm changes for this offline application:

- Search the entire recording using the original 40 ms / 3.125 Hz coarse grid,
  retaining local correlation peaks rather than assuming an aligned slot.
  Search includes partially captured leading/trailing frames when at least two
  Costas blocks remain; decoding still requires sufficient payload information.
- Bound candidates to 160 per recording second. Coarse correlation uses the
  signal/background ratio directly rather than WSJT-X's per-slot percentile
  normalization and QSO-frequency prioritization.
- Cache a 16-second FFT of a zero-padded 15-second buffer per candidate-start
  second; downconvert to 200 complex samples/second using the original filter.
  Time-ordered decoding retains one spectrum. Priority/sync ordering retains
  up to 32 spectra with least-recently-used eviction (about 47 MiB), scoped to
  one subtraction pass. This covers candidate-start seconds in a 30-second live
  window without recomputing a large FFT each time priorities revisit a second.
  Longer batch recordings retain the same bounded cache. FFT scratch is reused.
- Use Rust's `tanh`/`atanh` with finite saturation for belief propagation.
  The fallback uses a bit-packed most-reliable basis and order-two patterns
  with the depth-2 parity screening rule. It retains CRC validation of only the
  minimum-distance candidate. Higher-depth OSD preprocessing is not included.
- Subtract decoded signals between full-recording passes. Reference waveform
  phase is calculated directly rather than using the original lookup table.
- Support standard messages, `/R` and `/P`, nonstandard callsigns, free text,
  telemetry, DXpedition, Field Day, RTTY, EU VHF and the packed WSPR forms.

This increment does **not** claim decoding parity with the full WSJT-X FT8
receiver. QSO-state-dependent a-priori passes, the additional `ft8c`/`ft8d`/a7
assisted paths, higher OSD depths, baseline-derived SNR calibration, and optional
subtraction DT refinement remain unported. The current shared framework also supports WSPR and raw PCM input; see the main
README. The browser application now captures and decodes live FT8 using these same
adapters; its transport, sample-clock/UTC mapping and buffering limits are
documented in the main README. No FT8 algorithm or upstream source bytes changed
for browser integration. The browser additionally schedules a non-consuming
attempt on the latest 16 seconds at UTC slot +13.5 seconds, sharing the same
message context and overlap deduplication as the regular recovery windows.
The CLI `--realtime` option uses that same 16-second UTC-slot attempt for FT8
stdin and does not run a parallel recovery window.
This assumes an approximately synchronized computer/radio clock for early
results; off-clock signals still have the original 30-second recovery windows.
No sensitivity or real-time completion guarantee is implied by the early attempt.
The browser now includes explicit one-message FT8 audio playback; radio control
is separate. There is no native sound-card driver or network server.

## Independent slot and recovery scheduling

The browser now uses separate slot and recovery WASM workers over identical
continuous PCM. The slot receiver uses `AudioStream::push_buffered` to advance
bounded history without automatic full-window work, and schedules only the
recent-audio attempt at UTC +13.5 s. Recovery uses the unchanged automatic
30-second/15-second-hop path and skips the additional slot attempts. Both retain
the full passband, two passes, priorities, LDPC/CRC/OSD, continuous resampling and
stateful FT8 message contexts. No recovery work is truncated to meet a deadline.
The default combined `Receiver` and ordinary `AudioStream::push` behavior remain
unchanged for library callers and regression baselines.

A shared credit ledger releases each input chunk only after both lanes consume
it, preserving the aggregate 60-second in-flight bound. Recovery cannot block
the slot worker's event loop; sustained overload still stops capture explicitly
rather than dropping PCM. Stop flushes and waits for both tails. Abort/unmount,
startup cancellation and errors terminate both workers and release input.
Results are deduplicated at the presentation boundary, and lane-specific attempt
labels, durations and last recovery lag are displayed. Each lane retains its own
hash context; emitted text is never rewritten using another lane's later context.
This adds a WASM instance and resampling state and uses another CPU thread; it
does not guarantee deadlines on every device or calibrate physical audio latency.

The browser regression deliberately blocks the recovery thread for eight
seconds while delivering consecutive slot frames. Both slot CQ results must
arrive before recovery resumes. All original independent fixture payloads and
the combined receiver's payload/period set on the same derived input must remain
present after recovery and final flush. Transport regressions verify credit
ownership, the 60-second bound and cleanup of both workers.

## Standard FT8 transmit audio

`transmit::pack` ports the standard base-callsign mixed-radix encoding and type-1
bit layout (`28+1+28+1+1+15+3`) from `pack28` / `pack77_1` in the same recorded
`packjt77.f90` source bytes. It handles four-character locators, signed reports
-50 through +49 (including the upstream remapping below -30), R-reports, RRR,
RR73, 73 and plain CQ with a grid. It rejects unsupported forms instead of
falling back to hashes or free text. `/R`, `/P`, special prefix workarounds,
nonstandard calls and contest transmit formats remain unported.

`transmit::audio` reuses the existing augmented CRC-14, (174,91) LDPC, Gray/Costas
mapping and BT=2 GFSK waveform. It returns the imaginary (sine) component as
151,680 mono f32 samples at 12 kHz, peak 0.5, including the existing 20 ms cosine
edge ramps. Frequency is the lowest tone, limited to 100–4000 audio Hz. The
library has no device, clock, browser, CLI or radio-control dependency.

## Priority and incremental result delivery

`decode_with_context_and_observer` optionally reorders retained candidates near
queued frequencies (±60 Hz), trying the strongest sync peaks in those regions
first, then continuing the original order elsewhere. It retains the entire
normal passband, coarse candidate grid/caps, fine synchronization, metrics,
LDPC/CRC/OSD and subtraction passes. No processing path is removed to meet a
transmit deadline. Without priorities, candidate ordering remains unchanged.
The observer runs when a validated candidate is accepted, before the remaining
candidates and passes complete. Final batch-only APIs keep their hash-resolution
behavior; early event text reflects only context learned at emission.

The shared `AudioDecoder` observer has a default batch fallback for other modes.
`AudioStream::*_with_observer` adjusts stream origins and deduplicates before
notifying; returned batches include those same first emissions, sorted. Existing
batch APIs remain available. The browser sends decode-found events from the WASM
worker while it is still computing, ingests them immediately, and deduplicates
the eventual update batch. Capture credits still return only after processing,
so the existing bounded PCM transport and backpressure remain intact.

Queued frequencies update through a bounded worker message, without changing
input PCM or resampler history. CQ cards retain frame UTC internally and display time ago from the CQ end,
updated each second, plus first decode-arrival delay and missed-deadline diagnostics. Transmit tone follows the decoded station's
lowest tone and outgoing report follows received SNR, not the report of us in
their payload. The manual transmit frequency/report inputs have been removed.
The independent fixture regression compares all 19 payloads between default and
prioritized decoding and checks that the queued CQ is observed first. The WASM
browser regression checks immediate delivery before decode-completed, full-band
payload preservation, automatic tone matching and queue use of early results.
Both regressions also check 100 priority frequencies across the passband. A
cache regression checks reordered seconds against independently constructed
zero-padded FFT inputs, one transform per retained second, and bounded eviction.
Candidate ordering, caps, both passes and all decoding checks are unchanged;
this is a Rust-side cache optimization, not a change to upstream source bytes.
This does not establish full WSJT-X sensitivity or physical timing parity.

The browser owns a CQ-triggered station queue (`contact_queue.rs`), separate from
receive-only conversation estimates. Start contacting explicitly primes the
selected audio context and enables automatic responses. The first queued station
with a fresh CQ and an upcoming immediate response slot is selected. Ordinary
messages showing a different partner make that station ineligible until a new
CQ; stale recovery decodes cannot initiate a later call. Reply progression uses
only ordinary messages from the partner addressed to our configured callsign.
Each unanswered stage is limited to three attempts, with eight frames/180 seconds
per exchange, followed by demotion and a fresh-CQ requirement. One exchange runs
at a time; priority changes/removal cancel an active attempt. The queue is bounded
to 100 stations and 12 evidence messages each. This is limited ordinary-exchange
automation, not full WSJT-X assisted/contest QSO support.

Output selection uses
[`AudioContext.setSinkId`](https://developer.chrome.com/blog/audiocontext-setsinkid).
The start gesture unlocks the context, waits for the sink change, then resumes
again if the device change suspended it. A three-second preparation deadline
reports blocked/unavailable audio. The prepared output remains active between
frames while the queue is enabled, supporting later CQ-triggered audio without
requiring another autoplay gesture. Missing/changed sinks fail without fallback.
Computer-clock accuracy and physical output latency remain uncalibrated.

The explicit frame deadline is nominal slot +0.5 s. PTT is requested 500 ms before
it and must succeed before audio starts; receive is requested 200 ms after the
frame. Late keying or disconnection cancels audio and serializes unkey after any
pending key request. Normal completion retains the radio connection and audio
context; Stop, hidden tabs, input Stop/restart, errors and unmount disarm and
release them. The existing 30-second serial watchdog remains a fallback.

Validation includes recorded payload/PCM round trips, queue priority/freshness,
busy/other-partner cancellation, retry caps, bounded evidence, preserved receive
fixtures, sink-switch suspension recovery, browser queue controls and automatic
reply/PTT sequencing with simulated hardware. None establishes physical-radio
timing, RF delivery or full decoder/transmitter parity. Local sent information is
never injected into received evidence. Original source hashes remain unchanged.

The port now lives in the independent `wsjt-ft8` workspace crate. The shared
adapters in `crates/decoder` depend on it; the `wsjt-rusty` library re-exports its
original FT8 API paths. The crate split changes no protocol algorithms or
upstream source bytes; `UPSTREAM-SHA256.txt` remains unchanged.

## Validation

```sh
cargo test --workspace --release
cargo clippy --workspace --all-targets -- -D warnings
```

Tests cover every payload generator basis vector against all 83 parity checks,
CRC against independent polynomial division, upstream callsign-hash vectors,
bit-error recovery, zero-word rejection, the upstream FT8 WAV, shifted recording
alignment, overlapping stations, a truncated final frame, subtraction energy,
a partially captured leading frame, resampling/alias rejection, and silence/noise. The bundled sample is copied unchanged from
`wsjtx/samples/FT8/210703_133430.wav`.

The default two-pass run decodes **121 transmissions** from the supplied recording
in approximately 25 seconds on the development host. Transmission starts cluster
around **3.455 seconds modulo 15**, giving an estimated slot boundary at
**2.955 seconds**. The bundled upstream sample yields **19 transmissions**,
including a frame that begins before the recording.

The supplied recording's decoded messages are saved as readable text and JSON
lines in `results/` alongside the run diagnostics. Results are CRC- and LDPC-validated, but have not been compared
against an independently run WSJT-X executable; neither complete station
recovery nor exact sensitivity parity is established.

## License

GPL-3.0-or-later, matching WSJT-X. The port and copied tables/sample derive from
WSJT-X by Joseph H. Taylor, K1JT (upstream copyright 2001–2023), and the WSJT development team. See `LICENSE`
for the upstream license text. Rust crate dependencies retain their own licenses.

Browser transmit output uses a separate Web Audio gain stage with saved 0–60 dB
attenuation and smooth live changes (15 ms time constant). It does not change
the generated FT8 PCM. Queued station cards reuse the conversation stage renderer
and bounded receive tracker, retaining both directions and late stage evidence
while queued. Local transmissions remain separate from received stage evidence.

## Browser grid locations

`crates/web/src/geography.rs` adapts Maidenhead arithmetic from WSJT-X
`lib/grid2deg.f90` (WSJT development team, GPL-3.0-or-later), whose hash is
recorded in `UPSTREAM-SHA256.txt`. Display longitude is east-positive (upstream
returns west-positive). Four/six-character locations use actual cell centers;
upstream's missing-subsquare `mm` default is slightly off the four-character
center. Approximate distance uses spherical haversine with a 6371 km Earth
radius, not upstream ellipsoidal `geodist`, azimuth or propagation predictions.
No receive algorithm changes.

Only ordinary CQ/locator payloads supply a sender location. Bounded conversation
and queue models retain the latest grid by signal timestamp independently of
trimmed evidence; delayed earlier messages cannot replace newer grids.
OpenLayers 10.10.0 (BSD-2-Clause, license in `crates/web/vendor`) is vendored for
browser-only display. Visible OSM tiles use HTTPS, attribution and browser cache.
Map opening is explicit; unknown locations are omitted. No spot service is used.
