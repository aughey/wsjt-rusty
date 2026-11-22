# Browser contact records

The log records operator attempts when the armed queue schedules them. Selecting
a card alone does not create a QSO record. `contact_log.rs` separates timestamped
received decodes (payload, SNR, audio frequency) from successful local TX audio.
Canceled/failed playback is not recorded as sent. Software completion automatically
removes the station from the queue; it does not establish RF or mutual QSO success.
Completed callsigns have gray, faded, hatched conversation cards with a Contacted label.

`contact-log-worker.js` uses the vendored MIT-licensed sql.js 1.17.0 SQLite WASM
distribution, outside the receive/UI workers. No native build is added. SQLite
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
stops the attempt explicitly. Restart clears the recent capture cache, preserving
unsaved writes and committed history. Closing a tab with unsaved records can lose
them; clearing site data removes the database. No server synchronization exists.
The Contacts/Evidence tabs paginate 25 attempts and show full UTC timestamps.
Remove all cancels/disarms/clears the queue without deleting historical records.

ADIF follows the [3.1.7 specification](https://www.adif.org/317/ADIF_317.htm).
Exports include dates across UTC midnight, callsigns, FT8 mode, known grids and
directed sent/received reports. Application fields preserve record ID, outcome,
and ASCII-escaped JSON timestamped evidence. `QSO_COMPLETE` is `?` for software
completion and `N` otherwise. RF `FREQ` uses a fresh CI-V ReadFrequency result from the radio's serialized
background queries, in MHz (FT8 dial convention). The query UTC is retained with
the record and as an ADIF application field. It is sampled when an attempt first
has a readback no older than five seconds, without adding a transaction ahead of
timed PTT. Disconnected/stale/missing radio readbacks remain unknown. The editable
frequency form and audio Hz are never substituted; BAND is left for importers to
derive from FREQ. QSL confirmation is not fabricated.
Only completed records export by default; incomplete attempts are opt-in.

With Playwright and fake-indexeddb installed on NODE_PATH:

```sh
node --test crates/web/tests/contact-log.cjs
node crates/web/tests/contact-log-browser.cjs http://127.0.0.1:8088
node crates/web/tests/contact-map-browser.cjs http://127.0.0.1:8088
```

Tests use the actual shipped SQLite WASM and check worker reloads, simultaneous
tabs, revision ordering, transaction rollback/quota failure, ADIF lengths/dates/
evidence, real simulated audio playback, automatic queue removal, persistence,
contacted styling, map paths, keyboard tabs and mobile overflow. Browser tests do
not validate physical radio timing or RF delivery.

Conversations offers **Add all CQ stations** and **Automatically add new CQ stations**.
Both exclude previously completed contacts, our own station, queued stations and
stale or ongoing exchanges. Auto-add applies only to newly decoded CQs, starts
off on every page load and is never saved. Adding stations does not arm transmit.
