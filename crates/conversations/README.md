# ft8-conversations

Standalone FT8 conversation interpretation and contact policy using only Rust's standard
library. There are no dependencies or dev-dependencies, including on other
workspace crates. Build and test from the repository root:

```sh
cargo build -p ft8-conversations
cargo test -p ft8-conversations --release
cargo clippy -p ft8-conversations --all-targets -- -D warnings
```

`Conversations::default()` creates the tracker. `update(now_ms, messages)` accepts
an iterator of `(start_ms, end_ms, text, payload, snr_db)` tuples. Payload is the
original 77-bit FT8 payload encoded as ASCII `0`/`1`, not fabricated from text.
Use frame times and processed-audio time from one consistent millisecond epoch;
UTC is optional. An empty batch advances aging. Stop advancing time to freeze;
replace the tracker with `Default` to clear it.

Inspect `items` for callsign pairs, progress, inferred participants, received
messages and per-sender grid references. `exchange_slots(now_ms)` distinguishes
received, missing, expected, upcoming, optional and unheard stages. The six stages
cover CQ, callback, report, roger-report, acknowledgment and farewell. RR73 is an
acknowledgment with optional closing 73. `message_kind(now_ms)` exposes semantic
progress without assigning colors. `sort_by` lets callers apply their own order.

The tracker retains at most 200 contacts and 64 messages per contact, plus bounded
stage summaries and latest decoded grids. It handles retries, reversed callsign
pairs, delayed evidence and duplicate decodes. Unknown hashes do not form contacts.
Nonordinary payload types are ignored. Silence marks contacts stale after four
slots; stale contacts remain for three more slots, and sign-offs for three slots.
Missing messages are never synthesized. A heard sign-off does not prove mutual
completion or RF delivery. Interpretation behavior is preserved from the browser
tracker; extraction does not establish broader protocol coverage.

`src/lib.rs` owns the tracker and its behavior tests; `src/message.rs` owns
payload-aware classification and its tests. Browser coordinates, distances,
colors, transport, transmit state and UI stay in `wsjt-web`. GPL-3.0-or-later.

## Conversation decisions

`Conversation::decision()` in [`src/decision.rs`](src/decision.rs) is the pure,
station-independent decision point. It returns `ConversationDecision`:

- `received`: named boolean checkboxes for CQ, reply, report, roger-report,
  acknowledgment and farewell. These mean received evidence, never inferred or
  locally sent messages.
- `recommendation`: `Next(NextAction)`, `Complete`, or `InsufficientEvidence`.
  A next action includes its `ExchangeStep`, sender callsign (if known), recipient,
  and whether it is optional.

For example, after `K1ABC W9XYZ EN37`, the next action is a Report from K1ABC to
W9XYZ. After `W9XYZ K1ABC -12`, it is a RogerReport from W9XYZ to K1ABC. After a
lone `CQ K1ABC FN42`, the next action is a Callback addressed to K1ABC, with an
unknown sender. The application may choose its own station as that responder.
RRR expects a closing 73; RR73 recommends an optional closing 73. A received
standalone 73 ends the recommendation, even if earlier stages were missed.

Later evidence skips missing historical steps without checking their boxes.
Retries of earlier stages cannot regress the recommendation; delayed decodes can
fill its checkboxes. Summaries preserve these decisions after message trimming.
Repeated calls do not mutate the conversation. No clock or local station identity
is needed. Stale evidence retains its logical expectation: the caller must apply
freshness, UTC slot/parity, retry limits and operator authorization before acting.
The browser active-contact policy consumes this decision through `session::Contact`.

Run just its decision tests with:

```sh
cargo test -p ft8-conversations --release decision::tests
```

## Selecting an active conversation

`select_active_conversation(&candidates, transmit_slot_start_ms)` in
[`src/manager.rs`](src/manager.rs) returns `Option<&Conversation>`. Supply candidates
in priority order and call it when there is no active exchange. It picks the
first eligible candidate and preserves its identity without sorting or cloning.

Eligibility requires a calling conversation with no partner, whose latest CQ is
from the immediately preceding 15-second slot and whose frame ended within five
seconds before the intended transmit boundary (inclusive). Freshness uses decoded
frame time, never arrival time. Normal FT8 frames last 12.64 seconds, so they end
about two seconds before the next boundary. Frame starts are associated with the
nearest nominal slot boundary to tolerate fine-sync offsets. Both times must
share an epoch; pass the intended boundary rather than the current callback time.
An older CQ cannot be reused in a later transmit slot.

No eligible candidate returns `None`. Retaining/releasing an active conversation,
excluding prior contacts and transmission authorization remain application
policy. The complete manager below owns our-call filtering and attempt budgets.
The selector does not consume a CQ; identical inputs return the same reference.
The browser contact policy uses this selector through `manager::decide`.
Test this policy alone with:

```sh
cargo test -p ft8-conversations --release manager::tests
```

## Executing contact policy

[`manager::decide`](src/manager.rs) is the browser queue's policy entry point.
It takes an optional `Attempt` snapshot, priority-ordered `Candidate` references,
current clock time, local callsign/grid and the latest measured report. It returns:

- `Listen(reason)` while waiting for our slot, the receive worker, or a fresh CQ.
- `Start { ... }` to select an idle queue's first eligible caller.
- `Start { ... }` can also replace an attempt that has sent audio but has not
  received a partner report. Fresh CQs compete in priority order; stale CQs cannot
  take over. Selecting the same target retains its counters and evidence.
- `Transmit { ... }` with the outgoing message, stage and scheduled audio time.
- `End(reason)` for completed, abandoned or exhausted attempts.

[`session::Contact`](src/session.rs) validates addressed partner evidence and uses
`Conversation::decision()` to progress the exchange. Local playback and self echoes
cannot advance RX progress. Late evidence can advance it without regressing prior
progress. Slot parity prevents another station's turn from becoming ours.
A validated partner CQ in the current receive slot also permits an initial-call
retry before the full slot search completes. Self echoes, unrelated callers and
older CQs cannot authorize that retry, and renewed CQ retains the attempt budget.
A repeated addressed partner message in the current receive slot similarly permits
retrying the same response before the full search completes. Successful local
audio does not establish that the partner received it. Retain received history
and retry budgets; the executor supplies the latest measured SNR for each report.
`Attempt::end_reason` applies four unanswered initial calls, three transmissions
per later stage, eight total transmissions and a 180-second attempt cap. It keeps
the last permitted transmission's receive window open. Renewed CQ does not reset
the initial-call budget. After RRR/RR73 the owed closing 73 survives partner CQ or
traffic to another station. `after_transmission` updates successful-audio counters
and reports completion; failed or canceled playback must never call it.

The executor calls this policy near a slot boundary, reserves audio/PTT, and
revalidates the same message/stage/time before keying and after PTT acknowledgment.
A changed recommendation cancels unstarted audio, releases PTT and replans. Once
audio has started, it finishes without fabricating receive evidence. The web
adapter owns devices, packing/synthesis, settings, queue sorting, persistence,
frequency measurements and explicit Start/Stop, interruption and failure controls.

Selected conversations transfer their bounded history with `Conversations::remember`.
`Conversation::received_history()` includes up to 64 retained messages plus six
stage-summary messages, preserving heard stages after trimming. Snapshot merges
keep the larger summary count when trimmed overlap cannot be reconstructed.
The tracker does not retain an unlimited recording or all historical exchanges.

All interpretation, selection, session and policy tests run without a browser,
radio, decoder, external crate or dev-dependency:

```sh
cargo test -p ft8-conversations --release
```
