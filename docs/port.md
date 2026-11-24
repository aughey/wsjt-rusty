# Rust port and decoder development

The decoder is an incremental Rust source port of WSJT-X. FT8 runs locally in the
browser as WebAssembly; **FT8 and WSPR-2** are also available through the command
line and reusable headless Rust libraries. No native decoder executable or
C++/Fortran build is required. This is a work in progress, with algorithm coverage
and sensitivity limits documented in the [FT8](ft8.md) and
[WSPR](wspr.md) notes.

This guide covers workspace structure, the headless library API, validation,
and source provenance. Run commands from the repository root. For application
usage, see the [README](../README.md) and [browser guide](browser.md).

## Workspace

The root is a Cargo workspace. All dependency versions and local crate paths
are declared in `[workspace.dependencies]`; members use `workspace = true`.
Package version, Rust edition and license are inherited from `[workspace.package]`.

| Package | Location | Responsibility |
| --- | --- | --- |
| `wsjt-rig` | `crates/rig` | Rust IC-718 CI-V frequency/mode/RF power control, optional native serial and RTS/DTR PTT. |
| `wsjt-dsp` | `crates/dsp` | General windowed-sinc downsampling, independent of audio formats and protocols. |
| `wsjt-audio` | `crates/audio` | WAV loading, channel averaging and conversion to 12 kHz using `wsjt-dsp`. |
| `wsjt-ft8` | `crates/ft8` | FT8 receive algorithms, LDPC/CRC, 77-bit messages and waveforms. |
| `wsjt-wspr` | `crates/wspr` | WSPR receive algorithms, Fano coding and 50-bit messages. |
| `wsjt-rusty` | `crates/decoder` | Shared result/settings contracts, protocol adapters and bounded streaming. |
| `wsjt-rusty-cli` | `crates/cli` | Standalone `wsjt-rusty` binary: arguments, input and text/JSONL output. |
| `ft8-conversations` | `crates/conversations` | Standalone FT8 conversation interpretation; standard library only. |
| `wsjt-web` | `crates/web` | Browser FT8 UI, capture, waterfall/map, contact queue, transmit audio, radio controls and local log. |

The protocol crates can be used directly without the CLI or WAV loader. The
framework depends on both protocol crates and re-exports the original
`wsjt_rusty::{audio, ft8, ldpc, message, waveform, wspr, decoder}` library paths.
Framework integration tests live in `crates/decoder/tests`, audio input tests
in `crates/audio/tests`, and algorithm unit tests in their owning crates.
Unchanged recordings and golden vectors stay in the root `tests/fixtures`
directory.

See [the conversation crate](../crates/conversations/README.md) for its independent
build/test commands and decoded-message input contract.

## Shared decoder framework

For future implementation work, read [AGENTS.md](../AGENTS.md) for the code map,
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
[the FT8 notes](ft8.md) and [the WSPR notes](wspr.md).
Source is the sibling WSJT-X checkout at revision
`967c85a6119110788b0df3782cf796485b2cee14`; exact working-tree source hashes are
recorded in [UPSTREAM-SHA256.txt](../UPSTREAM-SHA256.txt). This is a source port, with RustFFT supplying
the FFT primitive, `libm` the error function and `hound` WAV I/O. Native WSJT-X
was used only as an independent development/test oracle.
