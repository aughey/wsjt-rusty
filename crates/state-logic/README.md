# Async QSO logic

The procedure selects a conversation, notifies the adapter, and loops:
read `last_reply`, choose its response, send, wait for receive, then read again.
`Reply` includes CQ, locator, report, roger-report, acknowledgment and farewell.
A newer earlier-stage reply can rewind the response; missing intermediate messages
need no invented stages. Completed audio stays separate from received evidence.

`contact_stations(&mut control)` keeps the selected handle and suspended attempts
on its async stack. `set_current_conversation(Some(handle))` and `None` are
notifications for display/logging. There is no current-conversation getter.
`exchange(control, station)` creates its own accounting-only budget from elapsed
time and completed audio receipts. It has no closing/established flags and takes
no budget argument. Replacement returns `Replaced`; the outer loop selects again.
Resumed handles supply their prior completed receipts so limits are not reset.
Selection, replacement/resumption and limits remain in the state logic. Initial calls allow
four transmissions, other stages three, with eight total and 180 seconds. Even
the final allowed send gets a receive turn. Received 73 needs no response;
successful closing-73 audio also completes the exchange. Completion is a property
of that response, not remembered closing state. Superseded audio rereads the latest
reply without spending a try.

`Conversation` supplies a live shared handle to received facts. `Control` supplies
a priority-ordered list, time waits, action execution and notifications. Transmit
and receive waits receive the remaining time budget from the procedure; they
must return when it expires. False from `transmit` means no completed audio, so
the loop rereads the reply without spending a try.

The Leptos adapter is `crates/web/src/transmit_ui/control.rs`. Shared `Rc` handles
carry received observations and stable log IDs. The browser queue is a passive
list/display projection: waits never call its planner or obtain a selected target
from it. `BrowserConversations` retains received/log data across Stop for late
updates; it has no selected-conversation property. Native callers retain their
legacy executor until migrated separately. `web-time::Instant` supplies the same
clock API on native and WASM.

Dropping the current transmit future revokes pending audio and arranges unkeying,
even with a pending key acknowledgment. The log service keeps pending revisions
until a durable commit; canceling the procedure never acknowledges a write.

Run `cargo test -p state-logic --release` for exchange, rewind, selection, retry
and cancellation regressions.
