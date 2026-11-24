# Shared contact records and ADIF logging

`wsjt-log` owns the headless record/evidence model, validation, UTC formatting,
ADIF writer, schema, revision handling and SQL transactions. Its `ContactStore`
trait describes log requests; `Database` supplies bound SQL execution and rows.
Both interfaces run the same `SqlStore<D>` implementation. Existing receiver and
browser contact-log modules retain compatibility re-exports.

`wsjt-log-sqlite` implements `Database` with rusqlite and system SQLite. Native
logging runs on one resource-owner thread, separate from egui, audio and radio.
Windows uses the operating system's WinSQLite3; no bundled SQLite C build is
added. Other platforms need system SQLite development/link libraries. Windows
stores the database in `%LOCALAPPDATA%/wsjt-rusty/contacts.sqlite3`.

Native **Log** provides Contacts/Evidence pages and ADIF export to a new file.
Export refuses to overwrite an existing file. Saves acknowledge only committed
revisions; failures retain pending records and expose Retry. Normal window close
disarms contacting, stops input and waits for pending saves. A save failure offers
retry, keeping the app open, or explicit close without saving. Forced process
termination can lose updates that have not committed. History never restores
transmit authorization. Map remains browser-only.

Both adapters use schema version 1, including existing browser JSON bodies and
the separate evidence table. Their SQLite images share a schema, but browser
site storage and the native file remain separate histories; no automatic
migration or synchronization is performed.

The log records operator attempts when the armed queue schedules them. Selecting
a card alone does not create a QSO record. `contact_log.rs` separates timestamped
received decodes (payload, SNR, audio frequency) from successful local TX audio.
Canceled/failed playback is not recorded as sent. Software completion automatically
removes the station from the queue; it does not establish RF or mutual QSO success.
Completed callsigns have gray, faded, hatched conversation cards with a Contacted label.

`contact-log-worker.js` uses the shared Rust engine through WASM and the vendored
MIT-licensed sql.js 1.17.0 SQLite WASM distribution, outside the receive/UI workers.
JavaScript owns IndexedDB and connection bindings, not ADIF or record rules. SQLite
tables hold contact metadata and evidence. Schema version is `user_version=1`;
newer databases are rejected rather than overwritten.

The SQLite image is committed in IndexedDB `wsjt-rusty-contact-log`, object store
`sqlite`, key `database-v1`. Each operation reads the latest image inside its own
transaction; read/write transactions serialize tabs. Writes acknowledge only
after IndexedDB commit. Parameterized SQL and per-record revisions prevent stale
updates from replacing newer evidence. The complete historical database persists
per browser/site; this snapshot approach rewrites its image on each save and is
intended for a personal log rather than very large multi-user datasets.

Errors retain pending updates in the tab and expose Retry saving / reload log.
At 200 pending attempts new exchanges are blocked until writes succeed. Up to
200 recent archived attempts can also receive late recovery updates in the current
capture; each attempt holds at most 256 events. Reaching the active evidence cap
stops the attempt explicitly. An ended attempt can also be marked Completed by a
later ordinary RR73 or 73 addressed from its partner to our callsign, in the
partner's slot parity and within three minutes of successful local R-report or
RR73 audio. This requires received partner report evidence, belongs to the newest
attempt for that pair, and retains the sign-off in the same log record. It removes
the finished waiting station without interrupting another contact or sending an
extra 73; RR73 makes that closing reply optional. Free text, other recipients,
failed audio and older attempts cannot qualify. This reconciliation works while
contacting is paused and does not arm transmit. Restart clears the recent capture
cache, preserving
unsaved writes and committed history. Closing a tab with unsaved records can lose
them; clearing site data removes the database. No server synchronization exists.
The Contacts/Evidence tabs paginate 25 attempts and show full UTC timestamps.
Remove all cancels/disarms/clears the queue without deleting historical records.

CQ replacement retains one resumable attempt per queued station (at most 100)
in the current capture. Ordinary partner evidence within the original three-minute
attempt lifetime continues updating the original archived log, including decode
arrival times, even if another QSO prevents a reply. A fresh addressed report can
resume that attempt in its next transmit turn, preserving its log ID, successful
audio, counters and chosen transmit tone. Existing established QSOs keep priority;
otherwise competing resumable reports follow queue order ahead of new CQ calls.
Freshness uses the received frame, not decode arrival. Stop, removal or move to
back clears resumption eligibility; reload never restores transmit authorization.

ADIF follows the [3.1.7 specification](https://www.adif.org/317/ADIF_317.htm).
Exports include dates across UTC midnight, callsigns, FT8 mode, known grids and
directed sent/received reports. Application fields preserve record ID, outcome,
and ASCII-escaped JSON timestamped evidence. `QSO_COMPLETE` is `?` for software
completion and `N` otherwise. RF `FREQ` uses a CI-V ReadFrequency result in MHz
(FT8 dial convention). A completed exchange requests a new dial readback when
the connected radio is idle, with at least three seconds before the next slot.
The result updates the archived contact by ID, including after its initial save.
The query UTC is retained in the record and an ADIF application field. Attempts
also retain their first fresh background readback (at most five seconds old) as
a fallback. Disconnected or failed queries never substitute the editable form
or audio Hz; missing readbacks remain unknown. BAND is left for importers to
derive from FREQ. QSL confirmation is not fabricated.
Only completed records export by default; incomplete attempts are opt-in.

With Playwright and fake-indexeddb installed on NODE_PATH:

```sh
node crates/web/tests/build-log-engine.cjs
node --test crates/web/tests/contact-log.cjs
node crates/web/tests/contact-log-browser.cjs http://127.0.0.1:8088
node crates/web/tests/late-signoff-browser.cjs http://127.0.0.1:8088
node crates/web/tests/replaced-report-browser.cjs http://127.0.0.1:8088
node crates/web/tests/late-selection-browser.cjs http://127.0.0.1:8088
node crates/web/tests/recontact-browser.cjs http://127.0.0.1:8088
node crates/web/tests/contact-map-browser.cjs http://127.0.0.1:8088
```

The build helper generates Node bindings for the actual release Rust WASM with
the Cargo.lock-matched wasm-bindgen CLI (PATH, Trunk cache, or `WSJT_WASM_BINDGEN`).
Native tests run with `cargo test -p wsjt-log-sqlite -p wsjt-native --release`.
`cargo run -p wsjt-native --release --example logbook` previews the real log UI
with isolated temporary sample data and no audio or serial devices.

Tests use the actual shipped SQLite WASM and check worker reloads, simultaneous
tabs, revision ordering, transaction rollback/quota failure, ADIF lengths/dates/
evidence, real simulated audio playback, automatic queue removal, persistence,
contacted styling, map paths, keyboard tabs and mobile overflow. Browser tests do
not validate physical radio timing or RF delivery.

Conversations offers **Add all CQ stations** and **Automatically add new CQ stations**.
Both exclude previously completed contacts, our own station, queued stations and
stale or ongoing exchanges. Auto-add applies only to newly decoded CQs, starts
off on every page load and is never saved. Adding stations does not arm transmit.

The Conversations page saves **Exclude logged completed contacts** per browser/site:
**Always** (the existing default), **For a number of days** (initially 7), or **Off**.
The interval is measured in elapsed 24-hour days from the latest completed log
record's end time; a station becomes eligible exactly when that interval expires.
The database summary queries all completed records, independent of log pagination,
and is loaded before bulk or automatic additions are allowed, including after reload.
Incomplete attempts and the bounded recent-session cache do not establish a prior
contact. Pending completed log writes also exclude the station until saved, so a
storage failure cannot immediately cause a duplicate. Restoring preferences never
arms transmit. Invalid settings and storage failures are reported on Conversations.
The Contacted card treatment follows the selected interval; manually selecting a
card explicitly overrides exclusion. Changing the interval does not remove stations
already queued or cancel an active exchange.
