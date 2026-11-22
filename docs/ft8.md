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
for browser integration. There is no transmitter, native sound-card driver or
network server.

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
