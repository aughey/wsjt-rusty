# Native FT8 receiver

Run the egui desktop interface with:

```sh
cargo run -p wsjt-native --release
```

Select an OS audio input and click **Start listening**. CPAL captures native
audio, averages the device's channels, and supplies continuous normalized mono
PCM to the same Rust receiver used by Leptos. No browser, WebView, decoder
executable, or network connection is needed. Keep the computer clock synchronized.
Use release builds for live decoding.

The initial interface has three tabs:

- **Spectrum analyzer:** 100–4000 audio Hz spectrum and a 60-second waterfall,
  automatic/manual display leveling, input peak, nominal slot guides, and colored
  decode outlines with message details. FFT-bin dBFS is a display measurement,
  separate from decoded SNR and RF power.
- **Decoded messages:** the latest 500 transmissions, with UTC start/end, SNR,
  audio frequency, payload-aware category, and original payload on hover.
- **Conversations:** the shared receive-only tracker, stable grid positions,
  six received/expected/missing stages, evidence details, signal/state/distance
  sorting, and an optional local grid for approximate distances. Unheard stages
  and inferred participants are not decoded evidence or QSO confirmation.

**Stop listening** immediately releases the input and drains/flushes decodes from
both workers. The waterfall and conversations freeze after the final updates.
Start is disabled during that flush. Restart clears receive observations. Closing
the window releases capture; it does not wait for final decode results to display.
No configuration is persisted yet.

For reproducible device-independent playback, expand **Recorded input** and enter
a WAV path before starting, or pass the path when launching:

```sh
cargo run -p wsjt-native --release -- tests/fixtures/ft8-210703-133430.wav
```

The file plays at capture speed and stops/flushes at EOF. WAV loading uses
`wsjt-audio`; playback UTC is anchored to the time Start is clicked, not the
recording's historical date. Clear the path to return to live input. Live input
supports the device's default format at 12 kHz or higher (f32/f64/i16/i32/u16);
unsupported formats fail explicitly. Windows uses WASAPI, macOS CoreAudio, and
Linux ALSA through CPAL. Linux builds require ALSA development headers and the
usual egui graphics/window-system libraries (for example `libasound2-dev`,
`libxkbcommon-dev`, and `libwayland-dev` on Debian/Ubuntu). Only Windows was
exercised during this implementation.

## Shared boundaries

`wsjt-receiver` contains the extracted receive-session scheduler, continuous
resampler integration, UTC mapping, incremental result callbacks, FFT/level/color
processing, annotation geometry, message categories, geographic presentation
helpers, and conversation adapter. It has no device or UI framework dependencies.
The browser's original module paths re-export these implementations, preserving
its worker bindings and callers. `ft8-conversations` remains a standalone
zero-dependency interpreter. Protocol DSP, LDPC/CRC, message unpacking, subtraction,
and waveform processing remain in `wsjt-ft8`, with stream/window contracts in
`wsjt-rusty`. No upstream algorithm or source provenance changed.

`wsjt-native` owns only the native device stream, bounded transport, worker
threads, and egui rendering. Independent slot and recovery threads retain the
same full-band two-pass receiver settings: latest-16-second attempts at UTC slot
13.5 s after the boundary and regular 30-second recovery windows with 15-second hops. Both flush at
Stop. Validated incremental results appear during searches; cross-lane/final
duplicates preserve the first rendered text. The shared `History` model keeps
500 decodes and feeds the bounded conversation tracker. The browser retains its
existing transport-specific row/log state while using the same extracted models.

PCM packets share a single 60-second sample credit ledger. Credits return after
all consumers finish with a packet; capture callbacks never wait for decoding.
Queue failure, stream errors, and timestamp gaps stop input explicitly instead
of silently dropping samples or joining audio across a discontinuity. UTC is
anchored once using the first native capture timestamp and then derived from
sample counts. Device latency and drift are not calibrated. Display FFT work
uses a separate bounded consumer and never modifies decoder PCM.

This is the receive-only first increment. Transmit audio, rig control, station
queue, map, contact log, persistence, and WSPR GUI support are not implemented.
It does not claim complete browser feature parity, WSJT-X sensitivity parity,
or a real-time deadline guarantee.

## Validation

The shared receiver keeps the original browser timing, fixture, and DSP tests.
Native transport regressions verify aggregate credit ownership/overrun and run
the independent WSJT-X FT8 recording through both threads with arbitrary chunks,
checking live events, both final flushes, payload/time/frequency preservation,
deduplication, conversations, and exact FFT center times. GUI checks use paced
fixture playback and inspect all three tabs, Stop/restart, frozen history, and
native input release. Simulated/virtual-device checks do not establish physical
sound-card timing or sensitivity.
