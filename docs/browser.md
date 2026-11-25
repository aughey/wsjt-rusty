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
station cards in a responsive grid that stays within the viewport. The waterfall
fills its plot; the Stations panel scrolls its controls and cards together without
an additional page or card-list scrollbar. Callsign, grid, audio output, PTT
selection lives in Contact configuration; TX audio attenuation is on Stations to contact. Start listening and
Stop remain available above every tab. Arrow keys, Home and End navigate tabs.

Receive decoding, conversation tracking, queue scheduling, sorting, expiry and
CQ auto-add continue across application pages. The waterfall keeps its bounded
history and open-frequency estimates live even when hidden, so returning does
not create a display gap. Switching pages does not disarm contacting or cancel
audio/PTT. Explicit Stop, browser-tab hiding and input interruption still do.
The map is disposed when hidden, and radio readback polling pauses outside Radio
control. Manual radio TX is released on leaving Radio control. Settings and
selections survive page changes.
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

While attempting a contact or in an active QSO, a solid amber band marks the
other station’s latest received 50 Hz footprint, labeled **RX**, callsign and
audio frequency. It follows new decodes from that station and disappears when
the attempt ends or contacting stops. The dashed cyan **TX** band shows your
transmit frequency; labels sit on separate rows so both remain readable when
the frequencies coincide. These markers indicate the selected exchange, not
confirmed RF delivery.

**Input peak** above the waterfall shows the maximum absolute input sample in
each quarter-second of continuous capture, before filtering or resampling. The
meter runs from 0 to 1; keep the peak below 1. If capture peak notifications are
unavailable, it uses the same live input snapshots as the waterfall (short peaks
between snapshots may be missed). A full-scale sample shows
**Clipping — reduce input gain**. This is a browser input level, independent of
waterfall leveling, decoder SNR and RF power. Stop retains the last reading;
Start clears it until fresh audio arrives.

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

Horizontal solid lines mark nominal FT8 audio starts at UTC slot +0.50 seconds;
dashed lines mark ends at +13.14 seconds (12.64 seconds of audio). They share
the decoded boxes' FFT-window-center clock, scroll with history, and freeze on
Stop. Hover or focus a decode to see its observed start offset from the nearest
nominal start: positive means later. That offset includes remote/local clock
error, propagation and audio buffering; it is not a propagation measurement.
These guides do not shift samples or transmit timing. Compare several stations
before attributing a common offset to local equipment. FFT windowing also blurs
visible signal edges.

Transmit already schedules audio at slot +0.50 seconds, matching the standard
FT8 delay in upstream `Modulator/Modulator.cpp`; the 500 ms PTT lead starts at
the slot boundary, rather than adding another half-second to that audio start.
Normal release is requested 200 ms after audio completion. Receive audio seen
after returning to RX can include device-buffered samples or late stations;
that alone does not establish an early local transmission. Physical audio
latency and radio switching remain uncalibrated.

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

On **Stations to contact**, **TX audio attenuation** reduces output from 0 to 60 dB. Move the slider right
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
The local Trunk server rebuilds changed files without automatically reloading the
page: a reload resets the receive session and requires Start listening again.
Stop listening/contacting, then refresh explicitly to use a new build. Unexpected
capture errors stay visible in a wrapping
alert on every application page until the next Start listening, rather than only
in the truncated session status. Real audio gaps still stop and disarm contacting.

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

Queued-frequency sync ordering uses a bounded per-start-second FT8 FFT cache.
Each lane can retain up to 32 spectra (about 47 MiB) during a subtraction pass,
so crowded queues do not repeatedly rebuild the same large FFTs as candidate
times jump around. This preserves priority order, full-band decoding, both
passes and the aggregate 60-second PCM limit. Sustained hardware overload still
stops explicitly; the fix does not enlarge the backlog or discard recovery work.

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
transmit audio frequency within 250–2500 Hz for each new attempt from up to 60 seconds of available
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

### Contact state and reply scheduling

After output preparation, the Leptos shell runs
`state_logic::contact_stations` in one cancelable local task. The selected
conversation and suspended attempts live on that future's stack. The procedure
notifies its adapter through `set_current_conversation`; there is no getter.
Its exchange loop reads the latest validated partner reply (including CQ), sends
the corresponding response, waits through receive, and reads again. A newer
locator or CQ can rewind the owed response; total/initial/stage budgets still
bound the attempt. Successful closing-73 audio completes the exchange.

The browser queue is a passive station list/display projection. Its legacy
planner, clock transitions, completion policy and transmit authorization are
inert in this mode. The adapter executes only actions for the supplied shared
conversation handle, checks current received evidence before key/audio, and
publishes status/PCM completion/log updates. Shared received contexts and stable
log IDs remain available for late RX updates without designating a current QSO.
Stop/input interruption/unmount abort the task; dropping a transmit future
arranges release through the JS/PTT owner even with a key acknowledgment pending.
The interval handles display, list sorting/expiry and frequency readbacks.
Pending log revisions survive cancellation until durable commit is acknowledged.

The queue displays whether contacting is paused, waiting for a fresh CQ, or in
an active QSO, including the required outgoing action and audio reservation.
Only one station owns the active exchange. Once partner report evidence
establishes a QSO, sorting and new CQs cannot replace it; completion, cancellation,
another partner, or an attempt limit releases it. A newer CQ from the partner
requests another locator response within the existing budget. A new attempt requires a
fresh, unused CQ in its immediately following response window.

The headless receive models retain furthest-heard evidence for cards and context.
The procedure's next send comes from the latest validated partner frame instead:
CQ selects locator, locator selects report, report selects R-report, R-report
selects RR73, RRR/RR73 selects 73, and received 73 closes the exchange. Newer
locators or reports can rewind the response without erasing received stages.
Missing intermediate frames require no invented evidence. Frame-time ordering
keeps a delayed older recovery decode from rewinding a newer received frame.
Messages must belong to the partner's alternating UTC slots after the opening
CQ; wrong recipients, other senders, invalid payload types and nonfinite times
cannot control the exchange. Local playback never becomes a received reply.

Playback completion updates sent history and retry counts, never RX progress.
Before audio starts, the scheduler revokes a reserved waveform if new evidence
requires another reply. Cancellation releases PTT before another plan can run;
a canceled reservation's completion callback cannot count as sent. Evidence
arriving after audio starts applies to the next frame. Retry exhaustion leaves
the final reply window open before returning the station to the waiting queue.
These rules have exhaustive state/event tests and queue timing regressions;
they are not a claim of formally verified software or physical radio delivery.

The fourth unanswered initial call is followed by its final receive window,
shown as “waiting for final reply (4/4 calls)”. When that window closes, the
procedure ends the attempt, moves the station to the back, and applies
the three-minute sorting penalty even while radio readiness or a frequency
query blocks new transmissions. Repeated CQs do not reset the attempt's call
budget. Cards show initial calls this attempt out of four separately from total initial
calls while queued, which accumulate across attempts until capture restarts.
The penalty changes sorting priority; a penalized station can still be selected
on a new CQ if no unpenalized caller is eligible.

RRR/RR73 requests closing 73. Successful closing audio completes the exchange;
no response is needed to a received 73. Pending audio is revalidated against the
latest reply, including when a closing response was superseded before it started.
The loop then recomputes the response without remembering a closing flag.
Explicit Stop, capture/output/radio failures and attempt limits still cancel it.
Before acknowledgment, abandoning an exchange requires a newer
partner receive slot; another decode's fine timing within the same slot cannot
cancel the accepted response. Queued cards prefer our station/partner pair's
received stage evidence over the partner's other conversations. Ended attempts
replace the last “sent … awaiting response” note with their termination reason;
capture interruptions retain their receive status on the card and in the log.
Capture still stops on discontinuity, suspension or device loss rather than
joining audio across a gap or automatically rearming transmit.

### Receive readiness and final transmit authorization

New-station selection opens only 200 ms before the nominal UTC transmit boundary.
After successful audio, the current partner's entire receive turn is preserved:
a CQ on the opposite parity cannot replace the call before its reply window.
The final station choice uses the decodes available at that boundary. Audio still
starts at slot +500 ms, with radio keying scheduled 500 ms before audio; received
updates continue to revoke obsolete reservations before playback. Reply synthesis
for the same active station may begin earlier, without committing another target.

If a fresh CQ replaced an unanswered call, a subsequent addressed partner report
can resume that earlier attempt when no established QSO owns the turn. Competing
fresh reports follow queue priority. This preserves the original contact record,
successful-audio counters and chosen TX tone. Replies also fill the replaced
attempt's log when a reply cannot be scheduled. Only reports from the immediately
preceding partner slot can resume TX; delayed recovery does not refresh them.
Explicit Stop, removal and move to back clear resumption eligibility.

The slot worker reports the last UTC receive slot whose early search completed.
Capture progress and recovery-lane completion do not supply this authorization.
Until a partner report establishes a QSO, a retry waits for receive-slot completion
or a new validated partner message. Once established, silence or an unfinished
slot search repeats the owed response in our next alternating transmit slot,
within the existing stage/attempt limits. A late decoded reply still advances or
revokes reserved audio before it starts; partner CQ or traffic to another station
still ends the exchange before acknowledgment. An owed closing 73 remains eligible.
Decoder passes, candidate
caps, full-band processing and recovery work are unchanged.

Received state changes revoke obsolete reserved audio immediately through a
reactive callback, including changes to the outgoing report or audio frequency
within the same QSO phase. The transport revalidates the plan before keying and
after PTT acknowledgment, immediately before scheduling its source. Superseding
cancels/unkeys without disarming or counting canceled audio as sent. Raw decodes
of our own audio remain in evidence but cannot control the partner state or
replace its SNR/frequency. Evidence now includes Decode available UTC, the first
arrival time at the app, separately from the estimated transmission time;
older logs have that column blank.

The `contact-scheduler-browser.cjs` regression injects delayed reports and
self-echoes through the running browser receiver, tests cancellation 50 ms before
start without allowing an interval tick, and decodes the PCM that reached its
simulated output using the release-WASM receiver. It covers the W4VPK report /
R-report / RR73 sequence and subsequent QSO continuation. Simulated output does
not establish physical sound-card or radio timing.

### Conversation policy in Stations to contact

The **Next action** panel identifies the active conversation and shows whether the
policy is listening or preparing a specific message. Cards distinguish **Active
conversation** from **Waiting conversation**, show the station-independent expected
sender/recipient and stage, and provide **Received conversation history**. History
is bounded to 64 messages plus six latest stage summaries; previously heard stages
survive display-history trimming. Local sent audio remains separate.

The zero-dependency `ft8-conversations` crate now owns the contact decisions in
`manager.rs` and partner-evidence progression in `session.rs`. The browser passes
priority-ordered candidates and current attempt facts to `manager::decide`, then
executes its **Listen**, **Start**, **Transmit** or **End** result. When idle, only a
CQ from the immediately preceding slot, ending within five seconds before the
intended transmit boundary, is eligible. Decode arrival does not refresh CQ age.
The active exchange keeps priority until completion, abandonment, explicit Stop or
its attempt limit, including a final receive window after the fourth unanswered
call. The next reply is checked again before keying and after the PTT acknowledgment;
late valid partner evidence can revoke unstarted audio. Started frames finish.
Device lifecycle, radio/output readiness, waveform generation, saved settings,
sorting and contact logs remain browser responsibilities.
