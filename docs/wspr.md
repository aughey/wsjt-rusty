# WSPR port notes

This implementation ports the WSPR-2 receive path from the sibling WSJT-X
`lib/wsprd` directory. Its four FSK tones are spaced 12000/8192 = 1.46484375 Hz;
162 symbols of 8192 input samples make a 110.592-second frame. Frames normally
occupy two-minute UTC periods. The decoder searches audio time
rather than assuming the WAV starts at a period boundary.

The port lives in the independent `wsjt-wspr` workspace crate. The shared
adapters in `crates/decoder` depend on it, and `wsjt-rusty::wspr` re-exports its
API. The workspace split changes no protocol algorithms or upstream source
bytes; the root `UPSTREAM-SHA256.txt` remains the source record.

## Source mapping

| Rust source | WSJT-X source |
| --- | --- |
| `crates/wspr/src/lib.rs`: downconversion | `wsprd.c`, `readwavfile` FFT extraction to 375 complex samples/second |
| Spectrum and candidate search | `wsprd.c` main loop: 512-point sine-window FFT, 128-sample step, seven-bin smoothing, 30th-percentile noise estimate, synchronization/time/frequency/drift search |
| Fine synchronization and soft symbols | `sync_and_demodulate`, `noncoherent_sequence_detection` |
| Signal subtraction | `subtract_signal2`, phase-continuous reference and 360-point sine LPF with edge correction |
| `crates/wspr/src/fano.rs` | Phil Karn's `fano.c` and `fano.h`: K=32, rate-1/2 convolutional code, Fano sequential search, 31 zero tail bits |
| `crates/wspr/src/tables.rs` | `wsprd.c` 162-bit synchronization sequence and `metric_tables.c` simulated metric table 2 |
| `crates/wspr/src/message.rs` | `wsprd_utils.c`: unpack50, unpackcall, unpackgrid, unpackpfx, unpk; `nhash.c` lookup3 hash |
| Coding-vector tests | `wsprsim_utils.c`, `get_wspr_channel_symbols`, and native Fano soft-symbol output |

The convolutional polynomials are `0xf2d05351` and `0xe4613c47`. The interleaver
uses eight-bit bit reversal, discarding indices ≥162. The 50-bit payload is
followed by 31 zero tail bits, producing 162 coded bits. Each transmitted tone
is `sync_bit + 2 * interleaved_code_bit`. WSPR has **no payload CRC**.

All three message forms are supported: callsign/grid4/power; extended
callsign/power; and hashed callsign/grid6/power. Callsigns learned from types 1
and 2 resolve type 3 hashes in the same decoder context. The hash uses WSJT-X's
seed 146 and 15-bit result. Power is transmitted dBm, not a received level.

## Adaptations for files and streams

- Downconversion keeps all available window audio, unlike native `readwavfile`,
  which reads the first 114 seconds of an aligned two-minute recording. The
  long FFT remains a 32:1 channel-extraction/downsampling operation, with a
  padded smooth FFT length. Normalize by FFT length rather than the native
  arbitrary amplitude factor; the soft-symbol normalization removes this gain.
- Search the full usable time range of each window, retaining the native
  allowance for a leading partial frame. Missing samples contribute zero.
  Search one selected WSPR subband up to 300 Hz wide, rather than the FT8
  decoder's broad passband. Spectrum peaks receive time, frequency and drift
  estimates, refined with the source's synchronization metric.
- Search integer total drifts from −4 through +4 Hz on the early passes and
  refine by ±0.5 Hz. The final default pass searches zero drift, as native
  `wsprd` does, for a lower-variance frequency estimate on weak signals.
- Try single-symbol, two-symbol and three-symbol block demodulation, plus
  bit-normalized single-symbol metrics, with the native ±64-sample timing
  jitter pattern. These block metrics are available on each pass.
- Fano uses the source bias 0.45, threshold step 60, metric table, tail
  constraints and bounded search. Early passes permit 10,000 cycles per bit;
  the third/fourth pass permits 50,000. Native `wsprd` defaults to 10,000 and
  exposes the budget through `-C`. This port's larger final budget recovers
  the weak G8VDQ sample despite small FFT/soft-symbol numerical differences.
- Generate phase-continuous subtraction references with double-precision phase
  accumulation. Use the ported direct LPF and subtract accepted signals during
  the pass, allowing subsequent candidates to benefit immediately.
- Retain payload bits and structured message fields in the common result.
  WSPR's reported audio frequency is the four-tone **center**, unlike FT8's
  lowest-tone convention. The common envelope identifies that distinction.

The implementation has no Jelinek stack-decoder alternative, optional Fortran
OSD path, WSPR-15 mode, `.c2` I/Q-file reader/writer, persistent on-disk hash file,
spot upload, rig-frequency correction, sound-card driver, or GUI. Success on
one public recording establishes a regression, not full native sensitivity
parity across all channel conditions. Severely truncated frames and signals
outside the selected band can remain undecodable.

## Public recording and native comparison

Source: [WSJT project WSPR sample](https://sourceforge.net/projects/wsjt/files/samples/WSPR/150426_0918.wav/download).
The fixture `tests/fixtures/wspr-150426-0918.wav` is copied byte-for-byte from
the sibling checkout and matches the SHA-256 published at that URL:

```text
0f1e41e6a94fefd3df9efd8234dd6342dfd7e6675700f70e79e69eec955537da
```

It contains 120 seconds of 12 kHz, mono, 16-bit PCM. Both this Rust implementation
and independently executed native WSJT-X `wsprd` recover these nine messages:

```text
ND6P DM04 30
W5BIT EL09 17
G8VDQ IO91 37
WD4LHT EL89 30
NM7J DM26 30
KI7CI DM09 37
DJ6OL JO52 37
W3HH EL89 30
W3BI FN20 30
```

`results/wspr-150426-0918-native.txt` records the native output. Native frequency
there is MHz relative to an unset (zero) dial frequency; multiply by 1,000,000
to compare audio Hz. Native `dt_print` is sample shift /375 −1 second, so add
one second to compare this program's first-symbol start times. The comparison
asserts the message set; it does not assert identical rounded SNR, DT or drift.
The separate common-framework FT8 regression still recovers 121 transmissions.

For this development comparison only, native C sources were compiled in
`/tmp/wsjt-wspr-reference` against the installed FFTW library. Native default
OSD is disabled (`ndepth=-1`), so the unavailable optional Fortran OSD symbol
was linked to an aborting stub: it would fail immediately if called. All
executed native decode algorithms were the unmodified WSJT-X C routines.
No native reference binary or helper is part of the Rust build or tests.

The four golden coding vectors in `tests/fixtures/wspr-code-vectors.txt` were
produced directly by native `get_wspr_channel_symbols` for the literal messages
in that file. `wspr-g8vdq-soft.bin` contains the native third-pass single-symbol
soft decisions for G8VDQ, using frequency offset −34.806252 Hz, sample shift
1200 at 375 Hz, and zero drift. Both are consumed as fixed independent Rust
regression inputs. They require no compiler or native decoder at test time.
