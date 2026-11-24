# Native FT8 station

Run the egui desktop interface with:

```sh
cargo run -p wsjt-native --release
```

Select an OS audio input and click **Start listening**. CPAL captures native
audio, averages the device's channels, and supplies continuous normalized mono
PCM to the same Rust receiver used by Leptos. No browser, WebView, decoder
executable, or network connection is needed. Keep the computer clock synchronized.
Use release builds for live decoding.

## Downloads and Raspberry Pi 4

The [native development release](https://github.com/aughey/wsjt-rusty/releases/tag/native-latest)
contains Windows x86-64, Linux x86-64, macOS Apple Silicon/Intel, and Raspberry Pi 4
ARM64 archives plus `SHA256SUMS`. Each successful `main` build updates this
prerelease; `v*` tags publish versioned releases. Pull requests build/test the same
matrix and retain downloads as Actions artifacts. Archives include the license,
setup guides and the exact source revision in `BUILD.json`.

For Raspberry Pi 4 use **64-bit Raspberry Pi OS Bookworm or newer** with a desktop
session. The `raspberry-pi4-aarch64` binary is built natively on an ARM64 runner
inside Debian Bookworm with generic ARMv8 instructions and glibc 2.36. It does
not run on 32-bit Raspberry Pi OS. CI runs the Rust regressions on ARM64; this
does not establish real-time decode speed or physical audio/PTT timing on a Pi.

Install the runtime libraries, extract the archive, and run its `wsjt-native`:

```sh
sudo apt update
sudo apt install libasound2 libudev1 libsqlite3-0 libxkbcommon0 libxkbcommon-x11-0 libwayland-client0 libgl1 libx11-6 libxcursor1 libxi6 libxrandr2
tar -xzf wsjt-rusty-raspberry-pi4-aarch64.tar.gz
./wsjt-rusty-raspberry-pi4-aarch64/wsjt-native
```

Linux x86-64 uses the same Bookworm baseline and needs equivalent runtime
libraries. Hardware OpenGL or a software OpenGL driver is required for egui.
For local builds install `pkg-config`, ALSA, udev, SQLite, X11/Wayland and OpenGL
development packages; `scripts/ci-native-linux.sh` lists the build dependencies.

Windows archives contain `wsjt-native.exe` (system WinSQLite3; Windows 10/11).
macOS archives contain an application bundle with microphone permission text;
allow the input when prompted. Downloads are unsigned and macOS bundles are not
notarized. Choose the archive matching your CPU. CI does not start audio streams
or serial ports while building or testing.

## Interface

The interface has seven tabs:

- **Spectrum analyzer:** 100–4000 audio Hz spectrum and a 60-second waterfall,
  automatic/manual display leveling, input peak, nominal slot guides, and colored
  decode outlines with message details. FFT-bin dBFS is a display measurement,
  separate from decoded SNR and RF power.
- **Decoded messages:** the latest 500 transmissions, with UTC start/end, SNR,
  audio frequency, payload-aware category, and original payload on hover.
- **Conversations:** the shared receive-only tracker, stable grid positions,
  six received/expected/missing stages, evidence details, signal/state/distance
  sorting, explicit grid square and latest RX SNR, and an optional local grid for approximate distances. Unheard stages
  and inferred participants are not decoded evidence or QSO confirmation. Click
  or keyboard-activate a card to queue its caller.
- **Stations to contact:** a bounded 100-station CQ queue, bulk/automatic CQ
  additions, front/back/remove controls, slot-based priority sorting, CQ age and
  original decode delay, six receive stages, and successful local sent-audio counts.
  Transmitting cards and the current outgoing stage pulse amber; sent audio stays
  cyan. Received CQ/reply/report/R-report/acknowledgment/sign-off stages retain
  their own colors, labels, missing-evidence hatching and current-stage markers.
  During audio playback an amber TX banner and window-edge glow remain visible
  on every page; the corresponding conversation card also glows.
- **Contact configuration:** standard base callsign, four-character grid, exact
  audio-output selection, attenuation, optional spectrum-based frequency placement,
  waiting-station timeout and saved settings.
- **Radio control:** IC-718 CI-V serial settings, frequency/band presets, mode,
  raw RF power, manual PTT with a 30-second watchdog, same-port or separate RTS/DTR,
  and CAT PTT for supported newer revisions. See [radio setup](rig.md).
- **Log:** persistent SQLite Contacts/Evidence pages, UTC timestamps, received
  SNR/grids, successful local audio and ADIF export. See [logging](contact-log.md).

## Contact stations

1. Start live input. In **Conversations**, select a station, or use **Add all CQ
   stations** in **Stations to contact**. Selection alone never starts transmit.
2. Fill **Contact configuration** and explicitly select the radio's audio output.
   No output is selected or substituted automatically. Set attenuation as needed.
3. For radio PTT, configure and explicitly connect **Radio control**. For audio
   only (for example, external VOX), explicitly uncheck **Use connected radio PTT**.
4. Click **Start contacting**. This prepares the selected output and enables the
   shared CQ-response/exchange policy. **Stop contacting** disarms and releases it.

Fresh CQ responses use the immediately following UTC slot. The common queue
retains the partner receive turn, four unanswered initial calls, three later-stage
transmissions, eight transmissions/180 seconds per attempt, fresh-CQ replacement
before a report establishes a QSO, and eligible resumption from late reports.
Only received ordinary addressed payloads advance partner evidence. Completed
software exchanges leave the queue. Local audio does not establish RF delivery
or mutual QSO confirmation. Outgoing tone/report follow the latest validated RX;
optional open-frequency placement estimates a quiet 400–2500 Hz space and holds
it for that attempt. It cannot guarantee a clear channel.

Waiting stations expire from their last CQ frame end (180 seconds by default)
while capture runs, including on other pages and while contacting is paused.
Successful initial-call audio counts reset on capture restart. Remove or move an
active station to the back cancels it; Remove all disarms and clears selections.
Automatic and bulk additions use the full persistent completion history, with
saved exclusion choices Always, a number of days, or Off. Pending completions
also exclude repeats. Manual selection overrides exclusion. Logging loads before
contacting or automatic additions are enabled. Map remains browser-only.

Stop/input failure/restart, minimizing the window, a clock jump, a stalled UI,
output/serial failures and closing the app disarm. Audio/PTT authorization uses a
250 ms UI heartbeat; native worker watchdogs continue without repainting. The
selected output remains open through successful frames while armed. Canceling a
reserved frame closes/resets the selected stream before reuse to discard queued
audio. PTT is requested 500 ms before audio, must acknowledge before the start,
and is released 200 ms after the playback clock reports completion. Queued late
keying is never retried. Device latency, physical PTT timing and clock drift are
not calibrated; hardware disconnection or process termination can prevent release.

**Stop listening** immediately releases the input and drains/flushes decodes from
both workers. The waterfall and conversations freeze after the final updates.
Start is disabled during that flush. Restart clears receive observations. Closing
the window releases capture; it does not wait for final decode results to display.
**Save preferences** stores station/output, queue settings and radio form values
in `%LOCALAPPDATA%/wsjt-rusty/native-settings.json` on Windows, or
`$XDG_CONFIG_HOME/wsjt-rusty/native-settings.json` (falling back to
`~/.config/wsjt-rusty/native-settings.json`) on Unix. Invalid restored values reset
with a visible explanation; read/write failures are displayed. Restoration never
connects a radio, opens an audio stream, restores a queue or authorizes transmit.

For reproducible device-independent playback, expand **Recorded input** and enter
a WAV path before starting, or pass the path when launching:

```sh
cargo run -p wsjt-native --release -- tests/fixtures/ft8-210703-133430.wav
```

The file plays at capture speed and stops/flushes at EOF. WAV loading uses
`wsjt-audio`; playback UTC is anchored to the time Start is clicked, not the
recording's historical date. WAV playback is receive-only. Clear the path to return to live input. Live input
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
helpers, conversation adapter, station queue/executor model, bounded attempt evidence
and spectrum occupancy estimator. Browser modules re-export all of these models. It has no device or UI framework dependencies.
The browser's original module paths re-export these implementations, preserving
its worker bindings and callers. `ft8-conversations` remains a standalone
zero-dependency interpreter. Protocol DSP, LDPC/CRC, message unpacking, subtraction,
and waveform processing remain in `wsjt-ft8`, with stream/window contracts in
`wsjt-rusty`. No upstream algorithm or source provenance changed.

`wsjt-native` owns native device streams, bounded transports, serial/output worker
threads, persistence of form preferences, TX authorization and egui rendering. Independent slot and recovery threads retain the
same full-band two-pass receiver settings: latest-16-second attempts at UTC slot
+13.5 seconds, and regular 30-second recovery windows with 15-second hops. Both
flush at Stop. Queued frequencies update both lanes without restricting the passband. Validated incremental results appear during searches; cross-lane/final
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

Map and WSPR GUI support are not implemented.
It does not claim complete browser feature parity, WSJT-X sensitivity parity,
or a real-time deadline guarantee.

## Validation

The shared receiver keeps the original browser timing, fixture, and DSP tests.
Native transport regressions verify aggregate credit ownership/overrun and run
the independent WSJT-X FT8 recording through both threads with arbitrary chunks,
checking live events, both final flushes, payload/time/frequency preservation,
deduplication, conversations, and exact FFT center times. GUI checks use paced
fixture playback to inspect receive views, native radio/configuration forms and
conversation-to-queue selection. Native hardware tests use a simulated backend
for key/ACK/play/unkey ordering, cancellation during keying, deadlines, failed
session reuse, startup cancellation and independent watchdogs. A full simulated
CQ/report/RR73/73 exchange exercises the shared queue and native executor.
Interpolation tests check duration, amplitude and image rejection at device rates.
No physical radio, transmit output, or PTT timing was exercised. Simulated-device
checks do not establish physical sound-card timing or sensitivity.

A device-free appearance preview exercises the same egui rendering without
loading native audio/serial ownership or TX authorization:

```sh
cargo run -p wsjt-native --release --example station-states
```
