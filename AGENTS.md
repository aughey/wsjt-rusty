# Codex guidance for wsjt-rusty

## Project intent and constraints

This project incrementally ports WSJT-X's C++/C/Fortran decoding algorithms to
Rust. It is a headless receive application, intended eventually to provide a
server that other applications can consume. The current implementation supports
FT8 and WSPR-2 through a command-line program and a reusable Rust library, plus
an optional browser FT8 receiver. Keep the receive libraries headless.

- Port the algorithms from the sibling `../wsjtx` checkout. Do not substitute an
  unrelated implementation or wrap an existing decoder executable.
- Production decoding and signal processing must be Rust. Do not add C/C++/
  Fortran build steps, FFTW bindings, native-decoder FFI, or decoder subprocesses.
  Rust crates for primitives, such as RustFFT and WAV I/O, are already used.
- Keep the library independent of CLI, GUI, sound-card and network transports.
  A future server should call the library, not spawn or parse the CLI.
- Preserve working modes while adding new ones. Reuse the shared input,
  scheduling and output contracts rather than building a separate application
  or input loop for each protocol.
- Keep algorithm adaptations traceable and document limits honestly. A matching
  fixture does not establish complete WSJT-X sensitivity or feature parity.
- Preserve upstream copyright notices and GPL-3.0-or-later licensing.

## Read first and source provenance

- `README.md`: current usage, public library example and framework behavior.
- `docs/ft8.md`, `docs/wspr.md`: source-to-Rust routine mappings, algorithm
  adaptations, fixture provenance and unported paths. Read the relevant note
  before changing a decoder; extend this documentation for each new mode.
- `UPSTREAM-SHA256.txt`: exact upstream working-tree files used for the ports.
  The upstream base revision is `967c85a6119110788b0df3782cf796485b2cee14`, but
  recorded file hashes identify the actual source bytes. Do not assume the
  sibling checkout is clean or modify it as part of ordinary Rust work.
- `LICENSE`, `Cargo.toml`, `Cargo.lock`: license, Rust 2024 workspace, dependencies
  and locked dependency versions.

## Workspace and dependencies

The root `Cargo.toml` is a virtual workspace using resolver 3. Every member
inherits `version`, `edition` and `license` from `[workspace.package]`. Declare
all dependency versions, features and local crate paths in the root
`[workspace.dependencies]`; member dependencies must use `workspace = true`.
Keep the single root `Cargo.lock` and shared `target/` directory.

- `wsjt-dsp` (`crates/dsp`): reusable signal processing, currently windowed-sinc
  downsampling to any equal or lower nonzero sample rate, with batch and stateful
  streaming APIs. No protocol or file I/O dependencies.
- `wsjt-audio` (`crates/audio`): WAV I/O and channel averaging, using `wsjt-dsp`
  for normalization to 12 kHz. No decoder or CLI dependency.
- `wsjt-ft8` (`crates/ft8`): FT8 DSP, LDPC/CRC, messages and waveforms.
- `wsjt-wspr` (`crates/wspr`): WSPR DSP, Fano coding and messages.
- `wsjt-rusty` (`crates/decoder`): common contracts, adapters and streaming;
  depends on the protocol crates and re-exports the original library module
  paths, including `audio` from `wsjt-audio`, for caller compatibility.
- `wsjt-rusty-cli` (`crates/cli`): binary-only package with the executable name
  `wsjt-rusty`. Depends on the framework and audio crates. `serde_json` belongs
  in presentation/transport crates (CLI and web); decoder libraries do not
  depend on command-line or browser presentation.
- `wsjt-web` (`crates/web`): browser-only Leptos CSR shell, built with Trunk.
  Owns continuous AudioWorklet microphone capture, waveform/FFT waterfall, UTC
  session timing and a dedicated WASM worker using `Ft8Decoder`/`AudioStream`.
  Its library target is a WASM entry point, preserving the CLI as the only
  native binary.
  Resample continuous PCM with `wsjt-dsp::StreamingDownsampler`; analyser snapshots
  are visualization-only. Bound transport to 60 seconds; fail explicitly on
  overrun/discontinuity. Stop releases input and flushes the decoder.
  Early browser attempts use `AudioStream::decode_pending` on the latest 16
  seconds at UTC slot +13.5 s; regular 30-second recovery windows stay active.
  Transport chunks are quarter-second; the queue still holds at most 60 seconds.
  Keep capture/UI dependencies here, outside the processing libraries.
  `spectrum.rs` performs visualization FFTs with RustFFT; `waterfall.rs` owns
  bounded canvas history. `annotations.rs` clips decode boxes to the passband
  and history using the same audio-clock epoch and FFT window centers. Display
  snapshots and automatic color leveling do not feed or alter decoder PCM. The
  waterfall range follows FT8's default settings, with 60 seconds at 10 rows/s.
  `message_kind.rs` owns presentation categories, outline colors and symbol
  badges; keep the legend and hover details consistent with this mapping.

Protocol crates are usable independently and must not depend on the framework,
CLI, or audio-file loader. Share equivalent DSP through `wsjt-dsp`; keep distinct
protocol algorithms in their protocol crates. Integration regressions belong
in `crates/decoder/tests`, with original fixtures retained at `tests/fixtures`.
Algorithm-local unit tests and audio input regressions remain in their owning
crates. `cargo run --release -- ...` works at the root because the workspace has
one binary; explicit CLI selection is `cargo run -p wsjt-rusty-cli --release -- ...`.

## Architecture and code map

The normal path is:

```text
WAV -> audio::read (mix channels, low-pass resample to 12 kHz) --+
                                                            |
PCM16LE stdin -> CLI conversion to normalized mono f32 -------+
                                                            v
                                       decoder::AudioStream::push
                                       independent worker per mode
                                       overlapping audio windows
                                                            v
                                       decoder::AudioDecoder adapter
                                       protocol receive algorithms
                                                            v
                                       decoder::Decode result envelope
                                       window-origin adjustment/dedup
                                                            v
                                       library caller or CLI text/JSONL
```

| Location | Responsibility and reuse guidance |
| --- | --- |
| `crates/decoder/src/lib.rs` | Public module exports; register new library modules here. |
| `crates/audio/src/lib.rs` | Shared WAV loading, channel averaging and resampling to 12 kHz via `wsjt-dsp`. Reuse this input normalization. |
| `crates/dsp/src/lib.rs` | General windowed-sinc anti-alias downsampling, independent of file formats and modes. |
| `crates/decoder/src/decoder.rs` | Shared `Mode`, `Settings`, `Decode`, `AudioDecoder`, adapters, `AudioStream`, bounded audio-history buffers and overlap deduplication. This is the integration point for every mode. |
| `crates/cli/src/main.rs` | Argument parsing, decoder construction, WAV/raw-stdin input, text/JSONL output and FT8 phase summary. Keep protocol DSP out of this file. |
| `crates/ft8/src/lib.rs` | FT8 Costas candidate search, downconversion, fine synchronization, soft metrics, decode/subtraction passes and FT8-local result type. |
| `crates/ft8/src/ldpc.rs` | FT8 (174,91) LDPC, augmented CRC-14, encoding and ordered-statistics fallback. The generic filename does not make this a universal coding module. |
| `crates/ft8/src/tables.rs` | Upstream FT8 parity/generator matrices. |
| `crates/ft8/src/message.rs`, `crates/ft8/src/message_tables.rs` | WSJT-X 77-bit message unpacking, callsign hashing, grids and contest tables. These are distinct from WSPR's 50-bit messages. |
| `crates/ft8/src/waveform.rs` | FT8 tone generation, GFSK reference waveform and signal subtraction. Used by decoding and tests, not a transmit application. |
| `crates/wspr/src/lib.rs` | WSPR FFT downconversion, spectrum/time/frequency/drift search, block demodulation, soft metrics and coherent signal subtraction. |
| `crates/wspr/src/fano.rs` | WSPR K=32 rate-1/2 convolutional code, bit-reversal interleaver and bounded Fano sequential decoder. |
| `crates/wspr/src/message.rs` | WSPR message types 1–3, grid/power unpacking and its 15-bit lookup3 callsign hash. |
| `crates/wspr/src/tables.rs` | Upstream WSPR synchronization sequence and simulated metric table. Decimal table values must remain exact. |
| `crates/web/src/lib.rs`, `crates/web/src/app.rs` | Browser WASM entry point and Leptos UI, capture lifecycle, decode list, annotation overlays and accessible hover/focus details. The same WASM module also loads in a worker; only mount the UI when a browser window exists. |
| `crates/web/src/audio.rs`, `crates/web/src/capture-runtime.js`, `crates/web/capture-worklet.js` | Browser transport ownership, microphone capture, cancellation and continuous mono PCM delivery. JavaScript handles transport/resource lifecycle; keep receive DSP in Rust. |
| `crates/web/decoder-worker.js`, `crates/web/src/worker.rs` | Worker transport and WASM bindings around the Rust receiver. Return queue credits only after processing a chunk. |
| `crates/web/src/receiver.rs` | Browser session wrapper over `StreamingDownsampler` and the shared FT8 `AudioStream`; validates continuity and adds UTC timestamps without changing the common decoder envelope. |
| `crates/web/src/spectrum.rs`, `crates/web/src/waterfall.rs` | RustFFT/Hann visualization, automatic background leveling, yellow-highlight color palette and bounded scrolling canvas. Display processing never modifies decoder PCM. |
| `crates/web/src/annotations.rs` | Nominal FT8 time/frequency rectangles, clipped to the passband and captured history using FFT row-center timestamps. |
| `crates/web/src/message_kind.rs` | Presentation-only message classification, colors and icons. Classification is not QSO tracking or a replacement for protocol message unpacking. |
| `crates/decoder/tests/decoder.rs` | FT8, error-correction, waveform and partial-frame regressions. |
| `crates/audio/tests/read.rs` | WAV channel averaging and resampling/alias rejection regression. |
| `crates/decoder/tests/wspr.rs` | Independent coding/soft-symbol vectors, real WSPR fixture, unknown timing, mixed protocols, live windows and EOF deduplication. |
| Inline unit tests | `crates/decoder/src/decoder.rs` tests windowing/lifecycle; `ldpc.rs` and `message.rs` test parity, CRC and upstream hash vectors. |
| `tests/fixtures/` | Versioned WAV recordings and native-generated independent golden vectors; no native tools needed to run tests. |
| `results/` | Saved readable decodes, JSONL and run logs, plus the native WSPR comparison. Historical FT8 JSON and newer `*-framework.jsonl` have different envelopes. |

Dependencies: `hound` for WAV, `rustfft` for FFT/complex primitives, `libm` for
FT8's Gaussian pulse error function, and `serde`/`serde_json` for result output.
Do not duplicate these services in a new protocol. Synchronization patterns,
codes, filters, downconversion rates and hash algorithms remain protocol-specific
unless equivalence is established. Extract a shared DSP helper when there is a
real equivalent operation, with regression coverage; do not force incompatible
algorithms through one generic implementation merely to reduce duplication.

## Shared contracts to preserve

- All `AudioDecoder` input is normalized, finite mono `f32` at
  `decoder::SAMPLE_RATE = 12000`. Raw stdin is headerless signed PCM16LE at that
  exact rate. WAV parsing/resampling is performed before the decoder boundary.
  A protocol needing another internal rate converts inside its own DSP path.
- `AudioDecoder: Send` is stateful and declares `mode`, `minimum_samples`,
  `window_samples`, `hop_samples`, and `decode(&mut self, &[f32]) -> Vec<Decode>`.
  Construction validates settings; the stream validates window sizes and input.
- Adapter results contain start times relative to the current window. Only
  `AudioStream` adds the recording/stream origin. Do not add it twice.
- `Decode.start_seconds` is `f64`, relative to the first input sample, not UTC.
  Negative values can represent a partially captured leading transmission.
  Never silently assume recording start coincides with a protocol's UTC slot.
- `frequency_hz` is audio frequency, not RF. Preserve `frequency_reference`:
  FT8 uses `lowest_tone`, WSPR uses `center`. A future RF/dial-frequency layer
  must explicitly account for these conventions.
- `payload` preserves raw message bits: 77 for FT8 and 50 for WSPR. The common
  result also carries mode, SNR and rendered message. WSPR has typed metadata
  for callsign, locator, transmitted power, message type and total drift.
  Extend typed metadata deliberately for new modes; do not put their fields
  into `WsprData` or discard them into an opaque text-only result.
- Keep protocol contexts alive across windows. FT8 adapters use
  `ft8::decode_with_context`; convenience `decode`/`decode_with_passes` create
  fresh message contexts. WSPR also retains its own `Messages` context.
  FT8 and WSPR hashes are different algorithms; do not merge their hash maps.
  Unknown hashes are `<...>`; emitted events are not rewritten retroactively.
- `AudioStream::push` accepts arbitrary chunk sizes, decodes synchronously and
  returns new results. Audio-history buffers are bounded by window size rather
  than stream duration. Retain this behavior for future continuous operation.
- `finish` flushes a sufficiently long partial window once, is idempotent, and
  rejects subsequent `push`. Do not assume a stream always ends on a full frame.
- Overlap deduplication uses payload, start time and frequency. Repeated messages
  in different transmission periods must remain separate. Different protocol
  latencies mean emitted batches need not be globally chronological.
- stdout is text or JSON lines; progress/diagnostics go to stderr. Do not mix
  logs into machine-readable output. Current processing applies backpressure;
  a future asynchronous server must provide bounded queues and an explicit
  capture/buffering policy rather than silently dropping samples.

### Current mode settings

All durations below are seconds; the trait returns sample counts.

| Mode | Minimum attempted window | Stream window | Hop | Default audio search | Passes |
| --- | --- | --- | --- | --- | --- |
| FT8 | 13 | 30 | 15 | 100–4000 Hz | 2 |
| WSPR-2 | 108 | 132 | 16 | 1390–1610 Hz | 3 |

FT8 transmission periods are 15 seconds. WSPR-2 frames last 110.592 seconds in
120-second periods; WSPR is limited to one search subband at most 300 Hz wide.
Minimum lengths permit partial-frame attempts, not guaranteed recovery. Initial
live decoding waits for a full stream window; EOF can trigger an earlier final
attempt. Choose window/hop sizes for new modes to cover frame boundaries at
arbitrary recording offsets, with tests demonstrating that coverage.

### Browser receiver contracts

- Keep continuous capture in the AudioWorklet and decoding in the dedicated
  WASM worker. Analyser snapshots feed only the waveform/waterfall. Preserve
  streaming resampler history and fractional phase across chunks; never
  independently resample successive chunks with the batch API.
- Anchor UTC once from the computer clock and AudioContext clock. Derive
  subsequent sample times from audio frame counts, not decode arrival or UI
  animation time. Input latency and clock drift remain uncalibrated. Stop or
  report discontinuities/suspensions rather than joining audio across a gap.
- Stop releases microphone input and flushes the partial transport chunk and
  decoder tail. Cancellation during permission/worker startup must release
  late-granted tracks; unmount must also abort an in-progress final flush.
  Preserve bounded transport (60 seconds of PCM) and the 500-row result limit.
- Annotation geometry uses the decoder's lowest-tone frequency, nominal 50 Hz
  bandwidth and 12.64-second duration. Position boxes against painted FFT
  window centers; retain a frozen time reference after Stop so final decodes
  can still be inspected. Preserve clipping, expiry, hover and keyboard focus.
- Message outline colors/icons distinguish CQ/calling, reply/locator, signal
  report, R-report, RRR, 73/RR73 and Other. Check the 77-bit payload type before
  classifying rendered text: current ordinary exchange labels cover `i3` 1, 2
  and 4. Free text, telemetry and specialized contest forms remain Other even
  if their text contains CQ or 73. Do not infer an established QSO state from
  a single received message. Keep category labels in accessible descriptions.
- Automatic display leveling estimates the median passband background;
  manual floor control remains available. The dark navy/green/yellow/pale-yellow scale
  spans 50 dB and applies to new rows only. Display levels are FFT-bin dBFS,
  not decoder SNR or RF power. Preserve dark backgrounds without altering DSP.
- Use release WASM for live operation. Trunk's generated JS/WASM and copied
  worker/worklet assets must be served together. File hashing is currently
  disabled because the worker imports the generated module by its stable
  name; update that import strategy before changing asset naming. Avoid
  simultaneous builds into a running dev server's `dist`; use an isolated
  output such as `target/web-preview` for browser QA when needed.

## Adding another decoder

1. Trace its actual upstream receive path and dependencies. Record source files,
   hashes, attribution and algorithm adaptations in a new `docs/<mode>.md`.
   Obtain a public recorded fixture or independent upstream vectors, with
   source URL/checksum and known messages where possible.
2. Implement protocol DSP/coding/message handling in its own workspace crate,
   register it in the root workspace members and dependencies, and add an inherited
   dependency/re-export in `crates/decoder`. Reuse existing primitives and
   equivalent message or coding routines where mathematically appropriate.
   Inspect the FT8-oriented modules in `crates/ft8` before copying their code,
   but do not assume another mode uses the same CRC, LDPC code, symbol mapping,
   waveform or callsign hashes.
3. Add a stateful `AudioDecoder` adapter in `crates/decoder/src/decoder.rs`. Map local results
   into the common envelope, retain learned context, and select appropriate
   sample windows, overlap and payload/time/frequency conventions.
4. Extend `Mode`, its display/serialization, and `Settings::defaults`/validation.
   Currently `Worker::collect` chooses a 2-second WSPR deduplication tolerance
   versus 0.35 seconds otherwise, with a 4 Hz frequency tolerance. Audit or
   generalize this choice for the new protocol instead of inheriting it blindly.
5. Update CLI parsing, decoder construction, help text and the explicit `all`
   mode list in `crates/cli/src/main.rs`. Its current file minimum-length check chooses
   13 seconds when FT8 is selected, otherwise 108; generalize it to the selected
   decoders when adding another mode. Do not create another input/output loop.
6. Add independent coding/recording regressions and stream integration tests:
   arbitrary chunking and timing, cross-window frames, live emission before EOF,
   final flush, deduplication, repeated messages in later periods, noise, and
   coexistence with existing modes. Test meaningful failure cases for that code.
7. Run the checks below and compare existing payloads/messages, not only decode
   counts. Update user docs, mode notes, provenance and relevant saved results.
   Keep this guide current when architectural contracts change.

## Validation and established baselines

Use release builds for DSP verification and timing:

```sh
cargo test --workspace --release
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
cargo check -p wsjt-web --target wasm32-unknown-unknown
cargo clippy -p wsjt-web --target wasm32-unknown-unknown -- -D warnings
node --test crates/web/tests/capture-worklet.cjs
```

For browser changes, also build from `crates/web` with `trunk build --release`.
Native workspace checks do not compile WASM-only UI code. Check the browser
with a simulated input or independent FT8 recording, including live emission,
Stop/restart and input release. When changing visualization/annotations, verify
frequency/time alignment, scrolling/freeze, hover/focus details, message-type
colors/icons and responsive layout. Browser checks with a simulated device do
not establish physical-device timing or sensitivity parity. Do not rely on
temporary QA scripts in `/tmp` surviving between sessions.

At the FT8/WSPR milestone, all 20 tests passed. The workspace split preserves
those regressions and adds a general resampler test. Important reference recordings:

- `digirig-2026-11-21-2min.wav`: 120 seconds of mono 48 kHz FT8 traffic.
  The shared framework recovers **121 transmissions**, matching the original
  full-recording decoder's payload/period set, not just its count. See
  `results/digirig-2026-11-21-2min-framework.jsonl` and its log. The estimated
  slot boundary is about 2.96 seconds into the file, not an absolute UTC time.
- `tests/fixtures/ft8-210703-133430.wav`: WSJT-X FT8 recording. The initial
  two-pass port recovered 19 transmissions including a leading partial frame.
  Tests also cover time shifts, overlapping stations and truncated endings.
- `tests/fixtures/wspr-150426-0918.wav`: public WSJT-X WSPR sample. Recover
  **9 messages**, matching the independently run native `wsprd` message set.
  Its URL, checksum, native output interpretation and oracle setup are in
  `docs/wspr.md`; results are `results/wspr-150426-0918.*` and
  `results/wspr-150426-0918-native.txt`.
- `tests/fixtures/wspr-code-vectors.txt` and `wspr-g8vdq-soft.bin`: golden data
  generated by native WSJT-X, testing coding, hashes, message forms and a weak
  signal independently of the Rust waveform generator.

Native WSJT-X is permitted as an independent development oracle only; it is not
an application/test runtime dependency. Existing tests consume fixed fixtures.
Do not rely on development binaries in `/tmp` surviving between sessions. Keep
original fixture bytes and independent expected messages intact; investigate
regressions rather than weakening assertions to fit a changed implementation.
Do not claim CRC validation for WSPR: it has no CRC and uses sequential-code,
zero-tail and message-plausibility checks. FT8 does validate its augmented CRC.

Known limits remain documented in the mode notes. FT8 lacks some specialized
assisted paths and full sensitivity parity; WSPR lacks WSPR-15, Jelinek/optional
OSD alternatives and `.c2` input. The browser shell provides microphone waveform
visualization and live FT8 decoding; browser WSPR decoding, native sound-card
capture, networking and spot upload have not been implemented. These are future
scope, not existing shared services.
