# IC-718 radio control

`wsjt-rig` provides transport-independent Rust CI-V commands and bounded reply
parsing. It has no decoder, audio, UI, network or native-library dependency by
default. The optional `native` feature adds `serialport` with default features
disabled (no libudev dependency). Browser serial ownership lives in `wsjt-web`;
all CI-V command encoding, BCD and response matching run in Rust.

## Source mapping and provenance

The sibling WSJT-X checkout's `Transceiver/Transceiver.hpp` defines current-VFO
frequency, operating mode and PTT operations; `PollingTransceiver.cpp` schedules
state polling. `HamlibTransceiver.cpp` configures serial transport, reads current
frequency/mode in `do_poll`, updates frequency/mode through Hamlib, and delegates
keying through `do_ptt`. `TransceiverFactory.hpp` defines serial/PTT settings.
The exact inspected source bytes are recorded in `UPSTREAM-SHA256.txt`, relative
to WSJT-X base revision `967c85a6119110788b0df3782cf796485b2cee14`.

This is a limited Rust adaptation of those operations, not a port of every
WSJT-X rig backend. WSJT-X does not contain IC-718 wire routines: those are
provided by its external Hamlib dependency. No Hamlib library, FFI or subprocess
is used here. The wire implementation is newly written against Icom's manuals:

- [IC-718 instruction manual, control commands, printed pp. 57–58](https://www.icomeurope.com/wp-content/uploads/2019/07/IC-718_ENG_IM_11.pdf).
- [IC-718 #82/#83 advanced manual, section 5](https://www.icomeurope.com/wp-content/uploads/2025/02/IC-718_ENG_AM_0.pdf).
- [Web Serial API](https://developer.mozilla.org/en-US/docs/Web/API/Serial)
  defines the browser transport and permission requirements.

Mode codes and transmitter level encoding were checked against the newer manual.
The inspected Hamlib `rigs/icom/ic718.c` also identifies legacy PTT as
`RIG_PTT_NONE`; it is not copied into this project. That file credits
Copyright (c) 2000–2010 Stephane Fillod, with capabilities submitted by
Chuck Gilkes WD0FCL/4, under LGPL-2.1-or-later. Rust project additions retain
GPL-3.0-or-later; the existing WSJT-X attribution and license remain intact.

| Operation | Rust command / transport |
| --- | --- |
| Read/set current dial frequency | `ReadFrequency` / `SetFrequency`, CI-V 03 / 05 |
| Read/set operating mode | `ReadMode` / `SetMode`, 04 / 06; omit filter byte |
| Read/set RF power setting | `ReadPower` / `SetPower`, 14 0A; 0–255, packed BCD |
| Legacy IC-718 PTT | Active-high RTS or DTR through a suitable PTT interface |
| IC-718 #82/#83 CAT PTT | `SetTransmit`, 1C **01**, per that revision's manual |

Frequency uses five little-endian packed BCD bytes. Default radio/controller
addresses are 5E/E0; custom radio addresses 01–7F are accepted. Five-byte frequency
requires **CI-V 731 mode OFF**. Range is 30,000–29,999,999 Hz. Modes are LSB, USB,
AM, CW, RTTY and their supported reverse variants. Digital receive uses USB;
there is no fabricated USB-D mode or automatic filter change.

## Browser setup

Baud, CI-V address, PTT method, requested dial frequency/FT8 preset, mode and RF
power entry persist per site in browser localStorage and restore after reload.
Invalid saved values fall back to defaults; storage failures are displayed.
Restoring form settings does not connect a port, apply settings to the radio or
restore TX. Connect explicitly and use the Set buttons to apply requested values;
readbacks continue to reflect the connected radio. Serial port permission and
connection are managed separately by the browser.

Use a Web Serial-capable browser on HTTPS or localhost (for example desktop
Chrome/Edge). Connect a USB CI-V interface to the radio's REMOTE jack and route
receive/transmit audio through the appropriate audio interface. A CI-V-only
cable does not provide audio or legacy PTT. Match a fixed CI-V baud setting
(300, 1200, 4800, 9600 or 19200) and the radio's CI-V address.

For an interface combining CI-V and PTT on one serial port, select **RTS · same
CI-V port** or **DTR · same CI-V port** before connecting. Choose the line specified
by the interface manufacturer; the other line may power the interface. The app
immediately requests receive on the selected line when the port opens, then
uses that same port for CI-V transactions and keying. Serial drivers can pulse
modem lines during opening; physical behavior depends on the interface.

For a separate keying adapter, select a separate RTS/DTR option and connect its
port with **Connect PTT interface**. Keying requires a suitable interface circuit
connected to ACC SEND or the microphone PTT input; modem lines are not wired
directly to the CI-V bus. Select CAT only for revisions explicitly documenting
1C 01. Legacy radios do not implement this command.

**Connect radio** asks permission and reads the dial/mode; thereafter it polls
every two seconds while receiving. It does not change VFO, mode, frequency or
power on connection. Background polling has its own in-flight state and does not
disable operator buttons or change contact readiness. Polls and operator/PTT
requests share an ordered per-port transaction queue, so a click during a poll
is handled after the current transaction without interleaved replies. Command
failures close the session before subsequent queued work can use it.
FT8 presets fill the Hz field; **Set frequency** applies
it. Choose USB and apply mode separately. **Read RF power** reports the raw
radio setting, not measured watts or a calibrated percentage.

**Transmit (30 s max)** requests PTT and starts a timer. **Receive / unkey**
requests release and disconnects both ports; reconnect for subsequent operation.
Disconnect, errors, the timer, unmount and page exit attempt release. Keying
status is the requested state, not evidence of actual RF output. Page/process
termination, suspended tabs or USB loss can prevent software timers or release
commands; verify receive at the radio.

The FT8 Stations to contact queue can use this same radio connection to coordinate
PTT with its selected audio output. It reserves the connection while waiting,
requests keying 500 ms before the +0.5 s UTC audio start, and requires keying to
complete before audio starts. Receive is requested 200 ms after audio ends;
normal completion keeps the ports open. Cancel, late keying, missing audio output
or disconnection cancels playback and releases the reservation/PTT. Release waits
for any pending keying request before sending unkey, with the existing watchdog
as a fallback. Manual **Receive / unkey** retains its immediate disconnect
behavior. No radio frequency, mode or power change is made by the station queue.
Existing FT8 microphone capture remains independent of radio serial transport.

## Native API

```rust,no_run
use wsjt_rig::{Command, DEFAULT_ADDRESS, Mode};
use wsjt_rig::native::{Radio, PttLine};

let mut radio = Radio::open_with_ptt("COM3", 19200, DEFAULT_ADDRESS, PttLine::Rts)?;
radio.execute(Command::SetFrequency(14_074_000))?;
radio.execute(Command::SetMode(Mode::Usb))?;
radio.execute(Command::SetPower(25))?; // raw setting, not watts
let frequency = radio.execute(Command::ReadFrequency)?;
radio.set_line_ptt(PttLine::Rts, true)?;
// Supply transmit audio and manage timing in the calling application.
radio.set_line_ptt(PttLine::Rts, false)?;
# Ok::<(), Box<dyn std::error::Error + Send + Sync>>(())
```

Enable `wsjt-rig`'s `native` feature. `native::Ptt` controls a separate RTS/DTR
port. `Radio::execute(Command::SetTransmit(..))` is available for newer CAT
revisions. Native callers own transmit duration and must explicitly unkey;
dropping a control object makes a best-effort release. The browser's 30-second
watchdog is a browser policy, not part of the native API.

A runnable read/tune example (no transmission) is included:

```sh
cargo run -p wsjt-rig --features native --example ic718 -- COM3 19200
cargo run -p wsjt-rig --features native --example ic718 -- /dev/ttyUSB0 19200 14074000 USB
```

Other transports use `Transaction::new`, send `request()`, and feed arbitrary
chunks into `feed()`. Serialize transactions and impose a two-second deadline.
Echoes, broadcasts and replies to/from other addresses are ignored; matching
NG, collisions and malformed replies fail. Buffers are bounded. Since CI-V ACKs
have no transaction identifier, timeout/error closes the native/browser session;
reconnect instead of allowing a late ACK to acknowledge a different request.
Never automatically retry a keying command. No TCP server is implemented.

## Validation and limits

Rust tests cover independently specified command bytes, fragmented replies,
echo/noise/address filtering, malformed BCD, bounds, NG and bus collisions.
Unix pseudo-terminal tests exercise actual native serial request/reply handling,
negative ACK disconnect and best-effort CAT unkey on drop. Browser transport tests
cover permission/open cancellation, exact bytes, same-port keying, timeout release
and the TX watchdog. The Playwright test emulates a CI-V/PTT serial device and
checks the release WASM UI, tuning, RF level, key/unkey and reconnect. These are
simulations; no physical IC-718 or interface has been tested.

```sh
cargo test -p wsjt-rig --features native --release
cargo clippy -p wsjt-rig --features native --all-targets -- -D warnings
node --test crates/web/tests/serial-runtime.cjs
# Serve target/web-preview, with Playwright available on NODE_PATH:
node crates/web/tests/radio-browser.cjs http://127.0.0.1:8082
```

Split/VFO switching, memories, RF meters, tuner control, VOX configuration,
waveform generation and full assisted/contest QSO automation remain outside the
radio-control crate; the browser station queue coordinates ordinary FT8 exchanges
using the independent FT8 waveform module.
RF control is separate from the headless decoding libraries and does not alter
audio-frequency or UTC decode contracts.
