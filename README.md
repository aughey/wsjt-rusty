# wsjt-rusty

An incremental Rust port of WSJT-X's digital-mode receive algorithms, with
**FT8 and WSPR-2** decoders. Both run behind the same protocol interface on WAV
files or chunked audio streams. No GUI, C++/Fortran decoder, FFTW, FFI decoder,
or external decoder process is needed to build or run the application.

## Workspace

The root is a Cargo workspace. All dependency versions and local crate paths
are declared in `[workspace.dependencies]`; members use `workspace = true`.
Package version, Rust edition and license are inherited from `[workspace.package]`.

| Package | Location | Responsibility |
| --- | --- | --- |
| `wsjt-dsp` | `crates/dsp` | General windowed-sinc downsampling, independent of audio formats and protocols. |
| `wsjt-audio` | `crates/audio` | WAV loading, channel averaging and conversion to 12 kHz using `wsjt-dsp`. |
| `wsjt-ft8` | `crates/ft8` | FT8 receive algorithms, LDPC/CRC, 77-bit messages and waveforms. |
| `wsjt-wspr` | `crates/wspr` | WSPR receive algorithms, Fano coding and 50-bit messages. |
| `wsjt-rusty` | `crates/decoder` | Shared result/settings contracts, protocol adapters and bounded streaming. |
| `wsjt-rusty-cli` | `crates/cli` | Standalone `wsjt-rusty` binary: arguments, input and text/JSONL output. |
| `wsjt-web` | `crates/web` | Leptos browser FT8 receiver with continuous microphone capture, waveform and UTC timestamps. |

The protocol crates can be used directly without the CLI or WAV loader. The
framework depends on both protocol crates and re-exports the original
`wsjt_rusty::{audio, ft8, ldpc, message, waveform, wspr, decoder}` library paths.
Framework integration tests live in `crates/decoder/tests`, audio input tests
in `crates/audio/tests`, and algorithm unit tests in their owning crates.
Unchanged recordings and golden vectors stay in the root `tests/fixtures`
directory.

## Command line

```sh
# FT8 remains the default; the workspace has one binary, so this still works.
cargo run --release -- digirig-2026-11-21-2min.wav

# WSPR, using the public WSJT-X sample included as a test fixture.
cargo run --release -- --mode wspr tests/fixtures/wspr-150426-0918.wav

# Run both decoders against the same audio; JSON lines for downstream consumers.
cargo run --release -- --mode all --json recording.wav

# Narrow or move a single protocol's audio-frequency search range.
cargo run --release -- --mode wspr --min-hz 1390 --max-hz 1610 recording.wav
```

| Option | Behavior |
| --- | --- |
| `--mode ft8\|wspr\|all` | Default: `ft8`. `all` feeds the same audio to both decoders. |
| `--json` | One JSON object per decoded transmission; diagnostics go to stderr. |
| `--passes N` | 1–4 decode/subtraction passes; defaults: FT8 2, WSPR 3. |
| `--min-hz F --max-hz F` | Custom search bounds for a single selected mode. |
| `--stdin` | Read raw PCM16LE, mono, **12000 Hz**, until EOF instead of a WAV path. |

FT8 defaults to 100–4000 audio Hz. WSPR defaults to 1390–1610 Hz, matching
WSJT-X's normal 1500 Hz center and ±110 Hz search. WSPR searches one subband up
to 300 Hz wide; select another subband by changing both limits. It is WSPR-2,
not the historical WSPR-15 mode. A WSPR-2 frame has 162 symbols and lasts
**110.592 seconds**, normally transmitted in a two-minute period.

WAV input supports integer PCM and float samples, channel averaging, and
sample rates of at least 12 kHz. The shared low-pass resampler converts to
12 kHz. A single-mode file needs at least 13 seconds for FT8 or 108 seconds for
an attempted partial WSPR frame. Severe clipping can prevent a decode.

To feed an existing PCM producer:

```sh
pcm_producer | target/release/wsjt-rusty --mode all --stdin --json
```

The producer must provide headerless signed 16-bit little-endian mono samples
at exactly 12 kHz. For a WAV file use the file-path form; do not pipe its header
to the raw PCM input. Decoding is synchronous: the consumer provides
backpressure, and an upstream live-capture system must buffer audio accordingly.
The decoder flushes a final partial window at EOF. No sound-card driver or
network server is implemented here.

## Browser FT8 receiver

Try the public receiver at **[aughey.github.io/wsjt-rusty](https://aughey.github.io/wsjt-rusty/)**.
The [GitHub Pages workflow](.github/workflows/pages.yml) builds and deploys the
release WebAssembly application on every push to `main`, or when run manually.
It serves the browser receiver only; the Rust libraries and CLI remain available
from the [source repository](https://github.com/aughey/wsjt-rusty).

The Leptos client captures audio continuously and decodes FT8 locally in a
WebAssembly worker. From the repository root:

```sh
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
cd crates/web
trunk serve --release --open
```

Use the release build for real-time decoding; debug WASM can fall behind.
Open `http://localhost:8080` if your browser does not open automatically. Click
**Start listening** and grant microphone permission. Route your radio's receive
audio into the browser's default input (select it in browser/OS settings).
Microphone access and AudioWorklet require localhost or HTTPS. Audio stays in
the browser; the capture graph outputs silence to avoid speaker feedback.

The FFT waterfall and waveform remain live while a dedicated worker runs the existing Rust
`Ft8Decoder` through `AudioStream`: 100–4000 audio Hz, two passes, 30-second
windows and 15-second hops. First results arrive after 30 seconds plus processing
time; subsequent attempts occur every 15 seconds. Arbitrary capture offsets are
supported; starting exactly on a UTC slot boundary is unnecessary. **Stop**
releases the input and flushes a final partial window (at least 13 seconds for
an FT8 attempt), retaining overlap deduplication and message-hash context.
Starting again creates a new session and clears the displayed messages.
WSPR remains available through the library/CLI but is not enabled in the browser.

The waterfall spans the decoder's full **100–4000 audio Hz** passband, with
frequency labels and a 60-second age scale. Newest spectra enter at the top at
10 rows/second; brighter colors indicate stronger signals. It uses a 16,384-point
Hann-windowed RustFFT of browser time-domain snapshots (about 2.69 Hz/bin at
44.1 kHz or 2.93 Hz/bin at 48 kHz). Display bins pool peaks so narrow signals
remain visible. The optional input waveform is under the waterfall.

**Auto level** starts enabled: a smoothed median of the passband estimates the
background, keeping ordinary noise dark rather than saturating the waterfall.
The palette uses cool navy/blue/cyan/white. Turn Auto level off to adjust
**New-row floor** manually. The color scale spans 50 dB above that floor and applies to newly drawn rows;
existing history retains its colors. Levels are coherent-gain-normalized FFT
bin amplitudes in dBFS, not decoder SNR or calibrated RF power. A bin-centered
full-scale sine reads 0 dBFS; off-bin signals have normal window scalloping.
The display is visualization-only, separate from continuous decoder PCM.
Skipped animation intervals are left blank rather than fabricated, Stop freezes
the display, and a new capture clears its history.

Decoded FT8 signals get outlined boxes over their nominal 50 Hz bandwidth and
12.64-second transmission duration. Hover a box or focus it with the keyboard
to see the message, SNR, audio frequency and UTC times. Boxes follow the audio
clock and the FFT window center, not decode arrival time or subsequent changes
to the computer clock. They clip to visible capture history/passband edges,
age off after 60 seconds, and remain visible for final results after Stop.

The page shows its computer clock in UTC, the first and last processed sample
UTC times, processing lag, and up to 500 recent decodes. Each row includes UTC
transmission start/end, audio frequency (lowest tone), SNR and message; hover a
row for its original 77-bit payload. Transmission end is the decoded start plus
FT8's 79 × 160 ms = 12.64 seconds, not a measured squelch transition.

Timing is anchored once using the computer's wall clock and
`AudioContext.currentTime`; the AudioWorklet's first sample frame establishes
the capture origin. Subsequent times use sample counts, independent of message
arrival, decode time, UI animation timing or later wall-clock adjustments.
Keep your computer clock synchronized. UTC estimates include uncalibrated input
latency and audio-clock drift; this is not a precision time source. The page
stops capture if the audio clock suspends or the input disappears, rather than
joining disconnected audio under misleading timestamps. Keep the tab active;
browser throttling, device changes and host load can interrupt live operation.

An AudioWorklet copies consecutive mono PCM into one-second chunks, with a
partial chunk flushed on Stop. Transport-only JavaScript connects it directly
to the worker using a MessageChannel. Rust's shared stateful windowed-sinc
resampler retains history and fractional phase across chunks and normalizes the
actual device rate to 12 kHz. Analyser snapshots remain visualization-only.
The worker preserves the common decoder envelope and adds UTC in the browser
session layer; the processing libraries remain independent of browser APIs.

The worklet allows at most 60 seconds of PCM in transit, returning credits only
after the worker processes a chunk. On overrun, it stops capture, reports the
error and finishes the contiguous audio already delivered. It never silently
skips a chunk or lets the queue grow without bound. Decode speed varies by
browser/device; sustainable real-time performance is not guaranteed on all
hardware. Unmounting cancels the session without a final decode.

Build static assets with `trunk build --release` from `crates/web` (output:
`crates/web/dist`). Serve all output assets together. File hashing is disabled
because the worker imports the same generated WASM module by name; configure
hosting to revalidate JS/WASM assets on deployment rather than cache them
immutably. No cross-origin isolation headers or WASM threads are required.
Native workspace checks do not compile browser-only code; also run:

```sh
cargo check -p wsjt-web --target wasm32-unknown-unknown
cargo clippy -p wsjt-web --target wasm32-unknown-unknown -- -D warnings
node --test crates/web/tests/capture-worklet.cjs
```

Tests cover streaming resampling equivalence at 12/44.1/48 kHz, bounded filter
history, UTC conversion, rejected discontinuities, FT8 fixture payload/message
preservation and emission before Stop, partial transport flushes, sustained
credit return and explicit overflow/gap failures. Spectrum tests cover tone
frequency/level at device rates, silence, out-of-band rejection and automatic
background leveling. Annotation tests cover audio-clock alignment, scrolling,
partial captures and history/passband clipping. Browser verification uses a
simulated microphone; physical-device latency is not calibrated.

## Shared decoder framework

For future implementation work, read [AGENTS.md](AGENTS.md) for the code map,
shared contracts and decoder-integration checklist.

`crates/decoder/src/decoder.rs` provides:

- `AudioDecoder`: a stateful protocol plugin with minimum, window and hop sizes,
  plus `decode(&mut self, &[f32]) -> Vec<Decode>`. Audio is normalized mono f32
  at 12 kHz. Implement this trait and register an instance with `AudioStream`
  to add another decoding algorithm.
- `Ft8Decoder` and `WsprDecoder`: adapters over the ported algorithms. Each keeps
  its own learned callsign hashes across successive windows.
- `AudioStream::push`: accept arbitrarily sized audio chunks, fan them out to
  independent protocol buffers, and return newly decoded transmissions.
- `AudioStream::finish`: flush remaining audio once. `push` after `finish` is an
  error. Overlapping windows are deduplicated by payload, frequency and start
  time; repeated transmissions in later periods remain distinct.

Audio-history buffers are bounded independently of stream length. FT8 uses
30-second windows with a 15-second hop; WSPR uses 132-second windows with a
16-second hop. Their different latencies are deliberate: one protocol never
has to use another's frame length. UTC alignment is not required. Initial live
results are available when a window fills, with subsequent attempts every hop;
EOF allows an earlier final attempt. The CLI also reports FT8 transmission
phase modulo 15 seconds and an approximate slot boundary on stderr. Callsign hashes use context learned by the
time of emission; already emitted JSON objects are not retroactively changed.
Results from different protocol buffers can arrive out of timestamp order.

```rust
use wsjt_rusty::decoder::{AudioStream, Ft8Decoder, Mode, Settings, WsprDecoder};
fn main() -> Result<(), Box<dyn std::error::Error>> {
let mut stream = AudioStream::new(vec![
    Box::new(Ft8Decoder::new(Settings::defaults(Mode::Ft8))?),
    Box::new(WsprDecoder::new(Settings::defaults(Mode::Wspr))?),
])?;
let samples = vec![0.0_f32; 4096]; // replace with normalized 12 kHz mono audio
let ready = stream.push(&samples)?;
let final_results = stream.finish();
Ok(())
}
```

The result envelope includes `mode`, `start_seconds`, `frequency_hz`,
`frequency_reference`, `snr_db`, `message` and raw `payload` bits (77 for FT8,
50 for WSPR). WSPR also includes structured callsign, locator, transmitted
power in dBm, message type, and estimated total frequency drift in Hz.

Start times are relative to the input's first sample, **not UTC**; a negative
start indicates a partially captured leading frame. Frequencies are audio Hz,
**not RF frequencies**. `frequency_reference` is `lowest_tone` for FT8 and
`center` for WSPR. SNR estimates use a 2500 Hz reference bandwidth. WSPR has no
CRC: its sequential code/tail checks and message validation are its acceptance
criteria. Unresolved callsign hashes are represented as `<...>`.

## Validation and provenance

```sh
cargo test --workspace --release
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

To select the CLI package explicitly, use `cargo run -p wsjt-rusty-cli --release -- ...`.
To test one library, use e.g. `cargo test -p wsjt-ft8 --release`.

The WSPR fixture is the public WSJT project sample
[`150426_0918.wav`](https://sourceforge.net/projects/wsjt/files/samples/WSPR/150426_0918.wav/download).
Its SHA-256 is
`0f1e41e6a94fefd3df9efd8234dd6342dfd7e6675700f70e79e69eec955537da`,
matching the upstream checkout and the published download hash.

- WSPR: **9 transmissions**, matching all nine messages from an independently
  run native WSJT-X `wsprd` on that recording.
- FT8: **121 transmissions** from the original two-minute recording through
  the shared file/stream framework.
- Tests cover upstream coding vectors and weak-signal soft symbols, all three
  WSPR message types, an arbitrarily offset recording, simultaneous FT8/WSPR
  audio, chunk boundaries, deduplication, finite buffers, EOF, silence/noise,
  resampling, and the original FT8 regressions.

Readable results, JSON lines, logs and the native WSPR comparison are in
`results/`. Implementation mappings, algorithm adaptations and limits are in
[the FT8 notes](docs/ft8.md) and [the WSPR notes](docs/wspr.md).
Source is the sibling WSJT-X checkout at revision
`967c85a6119110788b0df3782cf796485b2cee14`; exact working-tree source hashes are
recorded in `UPSTREAM-SHA256.txt`. This is a source port, with RustFFT supplying
the FFT primitive, `libm` the error function and `hound` WAV I/O. Native WSJT-X
was used only as an independent development/test oracle.

## License

GPL-3.0-or-later, matching WSJT-X. Derived from work by Joe Taylor, K1JT,
Steven Franke, K9AN, and the WSJT development team. The Fano decoder derives
from Phil Karn, KA9Q (1994), with Taylor's modifications. The hash implementation
derives from Bob Jenkins' public-domain lookup3 algorithm through WSJT-X.
See `LICENSE`; dependencies retain their own licenses.
