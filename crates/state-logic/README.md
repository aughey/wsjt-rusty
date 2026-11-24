# Async QSO logic

The control flow follows the exchange directly:

1. Wait for a station sending CQ.
2. Call with our locator and wait for its reply.
3. Exchange reports and acknowledgments, skipping stages already received.
4. Send closing 73 and record the outcome.
5. Return to looking for a station.

`contact_stations(&mut control)` runs this loop. `finish_qso` holds one exchange's
retry counts and elapsed-time budget on the async stack. Initial calls allow
four transmissions, later stages three, with an overall eight-transmission /
180-second limit. The last allowed transmission still gets its reply window.

`Conversation` is a live handle to validated, monotonically advancing partner
replies. `Control` supplies the priority-ordered candidates, clock, slot/reply
waits, transmit operation and durable outcome recording. Keep payload validation,
queue replacement/resumption, radio resources and database ownership in those
adapters. No concrete FT8 types or async runtime are required here. Browser and
native integration remains separate.

Dropping the operation cancels its current await. A transmit future must own a
reservation guard that cancels output and arranges unkeying on drop, even with a
pending key acknowledgment. Log writes must also be cancel safe and acknowledge
only durable commits. These are adapter responsibilities; the procedure spawns
no tasks and needs no asynchronous cleanup after an await.

Run `cargo test -p state-logic --release` for exchange, retry and cancellation
regressions.
