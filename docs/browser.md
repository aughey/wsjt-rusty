# Browser guide

Try the public receiver at **[aughey.github.io/wsjt-rusty](https://aughey.github.io/wsjt-rusty/)**.
The [GitHub Pages workflow](../.github/workflows/pages.yml) builds and deploys the
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

The full-page app has tabs for **Stations to contact**, **Spectrum**, **Radio
control**, **Contact configuration**, **Log**, **Decoded messages**, and
**Conversations**. Stations to contact combines the waterfall, world map and
station cards in a responsive flex layout. Callsign, grid, audio output, PTT
selection and attenuation live in Contact configuration. Start listening and
Stop remain available above every tab. Arrow keys, Home and End navigate tabs.

Only the visible view renders live content: hidden spectrum views stop their
animation/FFT work, the map is disposed when leaving Stations, and radio
readback polling pauses outside Radio control. Leaving Stations stops and
disarms contacting, including pending audio/PTT; returning requires Start
contacting again. Manual radio TX is released on leaving Radio control. Queue
sorting, expiry checks and auto-add run only while Stations is visible; elapsed
CQ time is checked again on return. Settings and selections survive tab changes.
The shared receive session, bounded decode/conversation history and durable
contact-log writes continue while listening so changing views does not lose
received evidence or unsaved contact records. Hidden tabs do not render cards,
message tables or log pages. The map opens with Stations and requests
OpenStreetMap tiles only while mounted.

While a spectrum view is visible, the FFT waterfall and waveform remain live.
Two dedicated WASM workers run
the existing Rust `Ft8Decoder` through `AudioStream`: 100–4000 audio Hz and two
passes in each lane. The slot worker handles deadline-sensitive recent-audio
attempts; an independent recovery worker retains the full 30-second windows and
15-second hops. A slow recovery does not serialize ahead of the next slot attempt.
The slot attempt uses the latest 16 seconds at **13.5 seconds into each UTC slot**, shortly after the nominal
transmission ends. Quarter-second transport chunks limit delivery delay. Results
can arrive before the next slot when processing finishes in time; slower devices
may still lag, and early attempts add processing work. Arbitrary capture offsets
are supported; starting exactly on a UTC slot boundary is unnecessary. **Stop**
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
The palette moves from dark navy through green to yellow and pale yellow highlights. Turn Auto level off to adjust
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
Stations in **Stations to contact** have thicker waterfall outlines. The station
in the active exchange also has a translucent fill and brighter outline; these
markers identify the decoded sender, preserving message category colors.

Outline colors and small symbol badges distinguish CQ/calling (green ◉),
reply/locator (cyan ◇), signal report (amber +), R-report (purple R), RRR
(blue ✓), 73/RR73 (pink ✦), and other messages (gray ·). A legend appears below
the waterfall and the hover details include the category. These are presentation
labels derived from standard/nonstandard callsign payloads and message content,
not inferred QSO state. Free text, telemetry and specialized contest forms stay
Other even if their text contains CQ or 73.

The **Conversations** panel groups ordinary FT8 exchanges by callsign pair,
independent of which station is addressed or its audio frequency. CQ calls start
an entry that can develop into a contact. Progress follows calling, locator
exchange, reports, acknowledgment and sign-off, without moving backward on
retries. These are receive-only estimates: an addressed station can be inferred
without hearing it transmit, and entering after the opening is marked inferred.

The conversation board spans the browser width, with a dense grid of small
fixed-height tiles and two
columns on phones and up to ten on wide displays. Each tile shows only its
callsign pair, six vertical stage indicators and a small stale/ended label.
Received stages show their latest decoded message, including callsigns, locator,
report or acknowledgment. Long messages truncate with full text on hover.
Dates and explanatory text stay out of the tiles. Hover or keyboard
focus reveals received messages, inferred participants and the expected
next response; Escape dismisses the detail panel. Clicking/tapping a tile moves
its calling station to the station queue described below. Hatched unheard stages,
ghosted upcoming stages and the highlighted current/next stage remain visible.
Tile borders, tinted backgrounds and received stages follow conversation progress:
green CQ, cyan reply, amber report, purple R-report, blue RRR, pink sign-off and
gray stale. This reuses the waterfall message palette. RR73 uses the sign-off
color even though it fills the acknowledgment stage. Retries do not regress the
color; stale contacts regain their progress color when activity resumes.

Activity never automatically sorts contacts or changes tile size. Signal, State and Distance sort once (strongest, CQ first and nearest caller); later activity keeps the chosen cells. Unknown distances sort last. Expiry leaves a vacant grid
cell, and new contacts reuse vacancies without shifting survivors. Resizing the
viewport adjusts the column count. At the 200-contact limit, the least recently
heard contact is replaced. The board accommodates many contacts on a page;
additional rows scroll with the page when its capacity exceeds the viewport.

Each tile lays out six logical stages: **CQ**, **callback/locator**,
**response/report**, **roger-report**, **RRR or RR73**, and **closing 73**.
Received messages occupy their stage, with evidence available in the detail
panel. Empty stages are marked **Not received**, **Expected next**, or
**Upcoming** in their accessible descriptions; stale contacts show **Not heard**
for unfinished stages. Unheard blocks have diagonal hatching; upcoming and
optional blocks use ghosted outlines. The next expected response keeps an amber
highlight, with a legend explaining these visual states. The furthest received
stage is highlighted, and the detail panel names the expected next response and
station direction. Earlier stages fill in when recovery decodes arrive. These
are logical stages rather than fixed UTC slots: stations can repeat or skip
steps. RR73 occupies acknowledgment and makes a subsequent 73 optional.

The detail panel shows actual decoded transmissions; no missing message is
invented. A sign-off heard on one side does not prove mutual completion.

Click a conversation (or press Enter/Space while focused) to move its calling
station into **Stations to contact**. Cards in that section expand to show queue
priority, the latest received message, SNR/audio frequency, our exchange stage,
recent received evidence, the last CQ age, decode-arrival delay/deadline
status and the most recent local transmit status. Other
conversations keep their original grid cells. **Move to back** lowers a station's
priority and cancels its active attempt; **Move to front** prioritizes a waiting station. An active exchange moves to the front and stays there through sorting or prioritization of waiting cards. **Remove** takes it out of the queue.
If the conversation is still retained, removing a station makes it visible on
the conversation board again. The queue holds at most 100 unique stations.

**Automatically remove stations without CQ** is on by default with a configurable
**CQ timeout (seconds)** of 180 (three minutes), saved per browser/site. While
receiving, waiting stations expire from the end of their last CQ; stations not
yet heard get a full timeout. Active exchanges are retained. Each card records
successful initial-call audio as **Contact tries**. After four initial calls with
no response, the attempt ends after its final reply window and the station moves
to the back, waiting for a fresh CQ and sorting behind unpenalized stations for
three minutes. Failed/canceled audio does not count; these
counts do not confirm RF delivery. Counts reset on capture restart or removal.

Enter your callsign/grid and choose **Transmit audio output**, then connect and
configure the radio/PTT method. **Start contacting** prepares the selected audio
device from the click gesture and enables the queue. It chooses the first queued
station whose latest ordinary transmission is a fresh CQ with an upcoming
response slot. Busy stations are skipped, so a lower-priority station can be
called when a higher-priority one is already in another contact. Stale CQs,
recovery decodes arriving after the response deadline and unsupported callsigns
never initiate a call. There is no arbitrary “Transmit next slot” action.

Queued stations' last-known frequencies are sent to the worker as decode
priorities. Candidates within ±60 Hz of those frequencies are attempted first,
in sync-strength order; the full-band search, candidate caps, both subtraction
passes and all LDPC/CRC/OSD checks remain active. CRC-validated results are sent
to the UI immediately when found, before the rest of the pass/band finishes.
Late CQ results explicitly show how far they missed the response deadline.
Priority changes apply between decoder attempts; ongoing synchronous work is
not discarded. Keep using release WASM. These changes reduce time to a useful
result rather than guaranteeing a particular device's total decode time.

A CQ response uses the immediately following 15-second UTC slot at +0.5 s. One
exchange runs at a time. Replies addressed from that station to our callsign
advance locator, report, roger-report, RR73 and closing 73; subsequent transmissions
use our alternating slots. Received SNR supplies the report automatically; each transmission uses the
partner’s latest decoded lowest-tone audio frequency. Initial calls allow four tries
with no response; later unanswered stages allow three transmissions;
an exchange stops after eight transmissions or three minutes. Exhausted attempts
move to the back and wait for a new CQ. If the station answers someone else,
its attempt is canceled and it waits for another CQ. Completed entries leave the queue automatically and retain their contact log. **Remove all** clears and disarms the queue without deleting log records. Local audio
and received evidence remain separate; a sent 73 does not prove RF delivery or
mutual QSO completion.

Queued cards show the same six received exchange stages as conversations, with
messages, missing steps and expected progress. Stage summaries remain available
while queued, including after the conversation board expires its entry.
During local playback, the station card and outgoing step pulse with an amber
glow and show the exact message being transmitted. Completed audio leaves a
steady cyan glow and a **Sent audio** mark on each outgoing step, including
after stopping or completing the exchange. Scheduled, canceled and failed
attempts are not marked sent. Starting a new listening session clears these
marks. Reduced-motion settings use a steady amber glow. These local playback
indicators are separate from received evidence and do not confirm RF delivery.

**TX audio attenuation** reduces output from 0 to 60 dB. Move the slider right
to lower drive. Set it in Contact configuration before starting contacting.
For example, 20 dB attenuation gives 10% of the original audio amplitude.
Callsign, grid, audio device, attenuation and radio settings remain saved per browser/site.
**Refresh audio outputs** requests device access when necessary; temporary
microphone permission streams are released. Missing or inaccessible saved audio
outputs never fall back to the main/system output. Selecting system default is
an explicit choice. Output selection needs `AudioContext.setSinkId` support.
The output context is resumed after the device switch completes, and reused
while contacting remains enabled. Start contacting must be clicked again after
an output interruption or browser autoplay block.

Radio PTT is requested 500 ms before audio and must succeed before the start
deadline. Receive is requested 200 ms after the 12.64-second frame, preserving the
serial connection. Cancel waits for pending keying before unkey, and the existing
30-second serial watchdog remains active. Uncheck **Use connected radio PTT**
for externally managed keying. Radio frequency, mode and power remain operator
settings. Keep the tab visible. **Stop contacting**, input Stop/restart, hiding
the tab, errors and unmount disarm/cancel playback and request radio release.
Selected stations survive capture restart with old observations cleared; reload
clears the queue and never re-arms it. Timing is subject to the computer clock and
uncalibrated physical-device latency. Standard base callsigns only; the receive
decoder keeps its broader message support.

After four silent 15-second slots (60 seconds since the last transmission end),
an unfinished contact becomes **Stale**. Stale entries remain for three more
slots (45 seconds); sign-offs remain for three slots after the last transmission
end. A fresh locator/report exchange after sign-off starts a new contact.
Late recovery decodes are merged by transmission time. Aging follows processed
audio, freezes on Stop, and resets on Start. The panel keeps at most 200 contacts
and 64 recent evidence messages per contact, plus one latest-message summary
for each of the six stages. Unresolved `<...>` callsigns,
free text, telemetry and specialized contest forms are left in the raw stream
without conversation grouping. This heuristic does not infer hidden callsigns,
confirm a QSO, or separate simultaneous contacts between the same callsign pair.

A yellow decoding indicator identifies the **slot response window** and
**background recovery** separately, including when both are running. Last slot
and recovery durations and the last recovery lag are shown. Each lane still runs
both passes; the recovery lane is an independent worker, rather than a tuning
phase which must finish before the next slot can start. Capture and display
continue while either lane works. Stop waits for both final tails.

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

An AudioWorklet copies consecutive mono PCM into quarter-second chunks, with a
partial chunk flushed on Stop. Transport-only JavaScript delivers it to the slot
worker, which forwards the identical PCM to recovery through a second
MessageChannel. Each lane retains its own continuous resampler and hash context.
Rust's shared stateful windowed-sinc resampler retains history and fractional phase across chunks and normalizes the
actual device rate to 12 kHz. Analyser snapshots remain visualization-only.
Both lanes preserve the common decoder envelope and add UTC in the browser
session layer; the processing libraries remain independent of browser APIs.

The worklet allows at most 60 seconds of PCM in transit. A shared credit ledger
returns a chunk credit only after **both** workers have processed it, keeping the
same aggregate bound rather than two independent 60-second queues. On overrun,
it stops capture, reports the error and finishes the contiguous audio already delivered. It never silently
skips a chunk or lets the queue grow without bound. Decode speed varies by
browser/device; sustainable real-time performance is not guaranteed on all
hardware. Unmounting cancels the session without a final decode.

Build static assets with `trunk build --release` from `crates/web` (output:
`crates/web/dist`). Serve all output assets together. File hashing is disabled
because the worker imports the same generated WASM module by name; configure
hosting to revalidate JS/WASM assets on deployment rather than cache them
immutably. No cross-origin isolation headers or shared-memory WASM threads are required.
Native workspace checks do not compile browser-only code; also run:

```sh
cargo check -p wsjt-web --target wasm32-unknown-unknown
cargo clippy -p wsjt-web --target wasm32-unknown-unknown -- -D warnings
node --test crates/web/tests/capture-worklet.cjs crates/web/tests/capture-runtime.cjs crates/web/tests/decoder-worker.cjs crates/web/tests/transmit-runtime.cjs
```

The `crates/web/tests/tabs-browser.cjs` Playwright check covers the full-page
shell, all seven tabs, settings retained across tab changes, hidden spectrum
painting and map disposal, live decoding from the independent FT8 recording,
contact disarming, keyboard navigation, phone layout and Stop/restart input
release. `radio-browser.cjs` also verifies that hidden radio controls do not
poll and that leaving manual TX releases the port. Serve an isolated release
build, then run with Playwright on `NODE_PATH`:

```sh
node crates/web/tests/tabs-browser.cjs http://127.0.0.1:8093
node crates/web/tests/radio-browser.cjs http://127.0.0.1:8093
```

The optional `crates/web/tests/conversations-browser.cjs` Playwright check runs
an independent FT8 recording through simulated microphone capture, then checks
conversation grouping, inference, expiry, evidence controls, Stop/restart and
mobile layout with controlled worker events. Serve an isolated release build
and run it with a Playwright installation on `NODE_PATH` and the server URL as
its argument. Conversation unit tests also cover delayed decodes, retries,
multiple partners, unresolved callsigns and bounded histories.

The `decode-lanes-browser.cjs` regression blocks recovery for eight seconds and
checks that two actual WASM slot CQs still arrive during the stall. It compares
all payload/period keys with the combined receiver on the same derived input,
preserves every original fixture payload, and checks shared credits and tail
flushes. Node transport tests also cover both-worker startup cancellation,
release and the aggregate 60-second bound.
The `decode-priority-browser.cjs` check runs the independent WAV directly through
the release WASM worker and compares all 19 payloads, first-CQ latency and event
ordering with and without priorities.
The `transmit-radio-browser.cjs` check additionally verifies sink-switch suspension
recovery, CQ priority, automatic exchange replies, saved station/output settings,
timed RTS keying, cancellation and output/connection reuse with simulated hardware. The transport tests
cover missing devices, permission failure, late ACKs, interrupted startup and
serialization of cancel/unkey with pending keying.

Tests cover streaming resampling equivalence at 12/44.1/48 kHz, bounded filter
history, UTC conversion, rejected discontinuities, FT8 fixture payload/message
preservation and emission before Stop, partial transport flushes, sustained
credit return and explicit overflow/gap failures. Spectrum tests cover tone
frequency/level at device rates, silence, out-of-band rejection and automatic
background leveling. Annotation tests cover audio-clock alignment, scrolling,
partial captures and history/passband clipping. Browser verification uses a
simulated microphone; physical-device latency is not calibrated.

## IC-718 radio and transmitter control

The Leptos receiver includes an **Icom IC-718 control** panel for serial dial
frequency, mode, RF power and PTT. Baud, CI-V address, PTT method, frequency/preset,
mode and RF-power entry are saved in this browser and restored after reload.
Connect explicitly; saved values are applied only with the Set buttons.
For a combined CI-V/PTT interface, choose
**RTS · same CI-V port** or **DTR · same CI-V port** before connecting, according
to your interface documentation. Match the radio's baud/address and turn CI-V
731 mode off. Browser control needs Web Serial support on HTTPS or localhost.

Legacy IC-718s use interface RTS/DTR for PTT; separate PTT ports are supported
as well. Newer #82/#83 revisions have an explicitly selectable CAT PTT option.
Transmit requests keying only, with a browser 30-second release/disconnect timer;
transmit audio is supplied separately. Verify RX at the radio after release.
RF power values are raw 0–255 settings, not watts. The radio-control panel handles
keying; the station queue supplies FT8 audio and coordinates timed PTT through this
same connection.

The independent `wsjt-rig` crate also works outside a browser: enable its `native`
feature for native serial control, or use its transport-independent transactions
in another application. See [radio control notes](rig.md) for native usage,
source provenance, hardware setup, tests and limits.

### Contact locations and world map

Cards show decoded Maidenhead grid centers and approximate great-circle distances
in kilometers from **My grid**. Conversation hover/focus details include each
heard participant's grid and east-positive longitude / north-positive latitude.
Only ordinary CQ and locator exchanges supply locations. Locations survive
message trimming while their card is retained; input restart clears observations.
Four-character grids cover a large area, so these are approximate positions.

Queue sort checkboxes are saved in this browser and reapplied each 15-second
cycle and before selecting a new caller. Checked criteria combine in order:
CQ-first state, strongest signal, then nearest distance (requires My grid).
The active exchange always stays first. Four unanswered initial calls move a
station behind unpenalized stations for three minutes, even if it has a stronger
signal or a new CQ. This is a sorting penalty, so it can still be contacted if
no higher-priority station is eligible. Conversations retain one-shot sorting.
Selecting a conversation puts its caller at the front of the waiting queue.
Card movement animations are disabled.

The Stations to contact tab displays a pan/zoom world map using bundled OpenLayers 10.10.0 and
OpenStreetMap tiles. Great-circle lines connect known endpoints of observed exchanges; dashed lines indicate inferred context or local attempts. Colors follow received stages; a white home marker uses
My grid. The map fills its panel without a header or close button; station and
path text lists remain available to screen readers. Unknown
grids are omitted. The visible map makes external tile requests to
OpenStreetMap, with visible attribution and normal browser caching. No callsign
lookup or spot upload is performed.

### Local contact log and ADIF

The Log tab has Contacts and Evidence views, with UTC dates and times,
our callsign/grid, the partner/grid, software outcome and timestamped evidence.
Received decodes retain payload, SNR and audio frequency; successful local playback
is separately labeled TX audio. Failed/canceled playback is never recorded as sent.
Records begin when an armed queue schedules an attempt, not when a card is selected.

SQLite (vendored sql.js 1.17.0 WASM) runs in a separate worker; IndexedDB commits
its database image locally for this browser/site. Records survive reloads and
capture restarts. Storage failures show Retry saving / reload log and retain
unacknowledged records in the tab. New attempts stop at 200 pending writes until
saving succeeds. Closing a tab with unsaved records or clearing site data can
lose them; download backups. There is no server upload or synchronization.

Completed software exchanges automatically leave Stations to contact. Their
stations get gray, faded, hatched Conversations cards and a Contacted label, also after reload.
This does not establish RF delivery or mutual QSO completion.

Download ADIF exports completed software exchanges; Include incomplete attempts
also exports interrupted/in-progress attempts. UTC dates/times, callsigns, known
grids/reports and application fields for timestamped evidence are retained.
QSO_COMPLETE is unknown for software completion and incomplete otherwise.
Connected radio readbacks supply the RF dial frequency in MHz and its query time.
Only fresh CI-V ReadFrequency results are recorded, never the editable form.
Disconnected or stale readbacks stay unknown; audio Hz are never exported as RF.
Importers can derive the band from the queried RF frequency. See
[contact-log implementation notes](contact-log.md) for storage and validation.

Conversations offers **Add all CQ stations** and **Automatically add new CQ stations**.
Both exclude previously completed contacts, our own station, queued stations and
stale or ongoing exchanges. Auto-add applies only to newly decoded CQs, starts
off on every page load and is never saved. Adding stations does not arm transmit.

The saved **find an open frequency** checkbox in Contact configuration selects a
transmit audio frequency within 100–3000 Hz for each new attempt from up to 60 seconds of available
waterfall spectra. It minimizes accumulated energy above each snapshot's median
background across a 50 Hz footprint plus 10 Hz guards on each side. Missing
snapshots do not count as quiet data; display floor/color settings have no effect.
With no recent spectrum, new attempts wait for data. This is an occupancy estimate,
not a guarantee that a channel is clear. The choice stays fixed through initial
calls and the responding exchange, even if the partner changes frequency or the
checkbox changes; each new attempt chooses again. With the option off, new attempts
retain the existing received-frequency matching behavior. Cards show the chosen
TX audio Hz, and a dashed cyan band marks its footprint on the waterfall. Receive
SNR still sets outgoing reports; local playback remains separate from RX evidence.

Waterfall transmit intervals scroll as blank rows. Local TX audio and FFT snapshots
that still overlap its tail do not paint spectra or contribute to open-frequency
occupancy; received history remains visible and continues aging against audio time.
