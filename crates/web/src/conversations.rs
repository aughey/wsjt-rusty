//! Receive-only FT8 conversation estimates. Never synthesizes decoded messages.
use crate::message_kind::{MessageKind, classify};

pub const SLOT_MS: f64 = 15_000.0;
pub const STALE_MS: f64 = 4.0 * SLOT_MS;
pub const RETAIN_MS: f64 = 3.0 * SLOT_MS;
const MAX_CONVERSATIONS: usize = 200;
const MAX_MESSAGES: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Stage {
    Calling,
    Exchanging,
    Reports,
    Acknowledged,
    Signoff,
}

impl Stage {
    pub fn label(self) -> &'static str {
        match self {
            Self::Calling => "Calling",
            Self::Exchanging => "Exchanging locators",
            Self::Reports => "Exchanging reports",
            Self::Acknowledged => "Acknowledged",
            Self::Signoff => "Sign-off heard",
        }
    }
}

/// Logical stages, not fixed UTC periods: retries may occupy several periods.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExchangeStep {
    Cq,
    Callback,
    Report,
    RogerReport,
    Acknowledgment,
    Farewell,
}

impl ExchangeStep {
    pub const ALL: [Self; 6] = [
        Self::Cq,
        Self::Callback,
        Self::Report,
        Self::RogerReport,
        Self::Acknowledgment,
        Self::Farewell,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cq => "CQ / initial call",
            Self::Callback => "Callback / locator",
            Self::Report => "Response / report",
            Self::RogerReport => "Roger + report",
            Self::Acknowledgment => "Acknowledgment / RRR or RR73",
            Self::Farewell => "Closing 73",
        }
    }

    pub fn short_label(self) -> &'static str {
        match self {
            Self::Cq => "CQ",
            Self::Callback => "Reply",
            Self::Report => "Report",
            Self::RogerReport => "R-report",
            Self::Acknowledgment => "RRR / RR73",
            Self::Farewell => "73",
        }
    }

    fn from_message(kind: MessageKind, text: &str) -> Option<Self> {
        Some(match kind {
            MessageKind::Cq => Self::Cq,
            MessageKind::Reply => Self::Callback,
            MessageKind::Report => Self::Report,
            MessageKind::RogerReport => Self::RogerReport,
            MessageKind::Acknowledgment => Self::Acknowledgment,
            MessageKind::Signoff if text.split_whitespace().last() == Some("RR73") => {
                Self::Acknowledgment
            }
            MessageKind::Signoff => Self::Farewell,
            MessageKind::Other => return None,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SlotState {
    Received,
    Missing,
    Expected,
    Upcoming,
    Optional,
    NotHeard,
}

impl SlotState {
    pub fn label(self) -> &'static str {
        match self {
            Self::Received => "Received",
            Self::Missing => "Not received",
            Self::Expected => "Expected next",
            Self::Upcoming => "Upcoming",
            Self::Optional => "Optional",
            Self::NotHeard => "Not heard",
        }
    }
    pub fn class(self) -> &'static str {
        match self {
            Self::Received => "received",
            Self::Missing => "missing",
            Self::Expected => "expected",
            Self::Upcoming => "upcoming",
            Self::Optional => "optional",
            Self::NotHeard => "not-heard",
        }
    }
}

#[derive(Clone, Debug)]
pub struct StepEvidence {
    pub latest: Heard,
    pub count: usize,
}

#[derive(Clone, Debug)]
pub struct ExchangeSlot {
    pub step: ExchangeStep,
    pub state: SlotState,
    pub current: bool,
    pub direction: String,
    pub evidence: Option<StepEvidence>,
}

#[derive(Clone, Debug)]
pub struct Heard {
    pub start_ms: f64,
    pub end_ms: f64,
    pub snr_db: f32,
    pub message: String,
    pub sender: String,
    pub stage: Stage,
}

#[derive(Clone, Debug)]
pub struct Conversation {
    pub id: u64,
    /// Stable grid-cell index, reused only after its conversation leaves the tracker.
    pub display_slot: usize,
    pub caller: String,
    pub partner: Option<String>,
    pub stage: Stage,
    pub messages: Vec<Heard>,
    pub last_ms: f64,
    pub inferred: bool,
    heard_from: Vec<String>,
    first_ms: f64,
    first_stage: Stage,
    calling_station: Option<(u8, f64, String)>,
    steps: [Option<StepEvidence>; 6],
}

impl Conversation {
    pub fn label(&self) -> String {
        format!(
            "{} ↔ {}",
            self.caller,
            self.partner.as_deref().unwrap_or("CQ")
        )
    }

    pub fn status(&self, now_ms: f64) -> &'static str {
        if self.stage == Stage::Signoff {
            self.stage.label()
        } else if now_ms >= self.last_ms + STALE_MS {
            "Stale"
        } else {
            self.stage.label()
        }
    }

    /// Conversation progress shares the waterfall message palette; stale overrides progress.
    pub fn color(&self, now_ms: f64) -> &'static str {
        let kind = if self.status(now_ms) == "Stale" {
            MessageKind::Other
        } else if self.stage == Stage::Signoff {
            MessageKind::Signoff
        } else {
            match self.steps.iter().rposition(Option::is_some).unwrap_or(0) {
                0 => MessageKind::Cq,
                1 => MessageKind::Reply,
                2 => MessageKind::Report,
                3 => MessageKind::RogerReport,
                4 => MessageKind::Acknowledgment,
                _ => MessageKind::Signoff,
            }
        };
        kind.color()
    }

    pub fn inference_note(&self) -> String {
        let Some(partner) = &self.partner else {
            return String::new();
        };
        let unheard = [&self.caller, partner]
            .into_iter()
            .filter(|call| !self.heard_from.contains(call))
            .cloned()
            .collect::<Vec<_>>();
        if unheard.is_empty() {
            "Inferred earlier progress · first heard after the opening exchange.".into()
        } else {
            format!(
                "Inferred participant: {} · addressed in received messages, but not heard transmitting. Missing transmissions were not decoded.",
                unheard.join(", ")
            )
        }
    }

    pub fn exchange_slots(&self, now_ms: f64) -> Vec<ExchangeSlot> {
        let highest = self.steps.iter().rposition(Option::is_some).unwrap_or(0);
        let terminal = self.stage == Stage::Signoff;
        let stale = self.status(now_ms) == "Stale";
        let caller = self
            .calling_station
            .as_ref()
            .map(|(_, _, call)| call.as_str());
        let responder = caller.and_then(|caller| {
            if caller == self.caller {
                self.partner.as_deref()
            } else {
                Some(self.caller.as_str())
            }
        });
        let caller = caller.unwrap_or("Calling station");
        let responder = responder.unwrap_or("Responding station");
        ExchangeStep::ALL
            .into_iter()
            .enumerate()
            .map(|(index, step)| {
                let evidence = self.steps[index].clone();
                let state = if evidence.is_some() {
                    SlotState::Received
                } else if index < highest {
                    SlotState::Missing
                } else if terminal {
                    SlotState::Optional
                } else if stale {
                    SlotState::NotHeard
                } else if index == highest + 1 {
                    SlotState::Expected
                } else {
                    SlotState::Upcoming
                };
                let direction = match step {
                    ExchangeStep::Cq => format!("{caller} → anyone"),
                    ExchangeStep::Callback | ExchangeStep::RogerReport => {
                        format!("{responder} → {caller}")
                    }
                    ExchangeStep::Report | ExchangeStep::Acknowledgment => {
                        format!("{caller} → {responder}")
                    }
                    ExchangeStep::Farewell => "Either station · optional after RR73".into(),
                };
                ExchangeSlot {
                    step,
                    state,
                    current: index == highest,
                    direction,
                    evidence,
                }
            })
            .collect()
    }

    pub fn expectation(&self, now_ms: f64) -> String {
        if self.status(now_ms) == "Stale" {
            return "No recent progress · waiting for renewed activity".into();
        }
        if self.stage == Stage::Signoff {
            return if self.steps[ExchangeStep::Farewell as usize].is_some() {
                "Closing 73 heard · exchange ended as far as received".into()
            } else {
                "RR73 heard · a closing 73 may follow".into()
            };
        }
        self.exchange_slots(now_ms)
            .into_iter()
            .find(|slot| slot.state == SlotState::Expected)
            .map(|slot| format!("Next expected: {} · {}", slot.step.label(), slot.direction))
            .unwrap_or_default()
    }

    fn expires_ms(&self) -> f64 {
        self.last_ms
            + RETAIN_MS
            + if self.stage == Stage::Signoff {
                0.0
            } else {
                STALE_MS
            }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Conversations {
    pub items: Vec<Conversation>,
    pub now_ms: f64,
    next_id: u64,
}

impl Conversations {
    /// Ingest the entire batch before advancing time: recovery decodes can arrive late.
    pub fn update<'a>(
        &mut self,
        now_ms: f64,
        messages: impl IntoIterator<Item = (f64, f64, &'a str, &'a str, f32)>,
    ) {
        let mut messages: Vec<_> = messages.into_iter().collect();
        messages.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (start, end, text, payload, snr_db) in messages {
            if start.is_finite() && end.is_finite() && end >= start {
                self.hear(start, end, text, payload, snr_db);
            }
        }
        if now_ms.is_finite() {
            self.now_ms = self.now_ms.max(now_ms);
        }
        self.items.retain(|c| self.now_ms < c.expires_ms());
        self.items.sort_by_key(|c| c.display_slot);
    }

    fn hear(&mut self, start: f64, end: f64, text: &str, payload: &str, snr_db: f32) {
        let kind = classify(text, payload);
        let words: Vec<_> = text.split_whitespace().collect();
        let stage = match kind {
            MessageKind::Cq => Stage::Calling,
            MessageKind::Reply => Stage::Exchanging,
            MessageKind::Report => Stage::Reports,
            MessageKind::RogerReport | MessageKind::Acknowledgment => Stage::Acknowledged,
            MessageKind::Signoff => Stage::Signoff,
            MessageKind::Other => return,
        };
        let (sender, recipient) = if kind == MessageKind::Cq {
            // CQ [DX/direction] CALL [grid]; calls always contain a letter and digit.
            let Some(sender) = words.iter().rev().find_map(|word| call(word)) else {
                return;
            };
            (sender, None)
        } else {
            let (Some(to), Some(from)) = (call(words[0]), call(words[1])) else {
                return;
            };
            if to == from {
                return;
            }
            (from, Some(to))
        };
        let matches = |c: &Conversation| {
            let pair = match &recipient {
                Some(to) => {
                    (c.caller == sender && c.partner.as_ref() == Some(to))
                        || (c.caller == *to && c.partner.as_ref() == Some(&sender))
                }
                None => {
                    (c.caller == sender && c.partner.is_none())
                        || ((c.caller == sender || c.partner.as_ref() == Some(&sender))
                            && start < c.first_ms)
                }
            };
            pair && start <= c.expires_ms() && end + STALE_MS + RETAIN_MS >= c.last_ms
                // A fresh locator/report exchange after a sign-off is a new contact.
                && !(c.stage == Stage::Signoff && stage < Stage::Acknowledged && start > c.last_ms)
        };
        // Prefer the contact whose observed interval is nearest the transmission,
        // so a late sign-off cannot complete a newer contact with the same pair.
        let distance = |c: &Conversation| (c.first_ms - start).max(start - c.last_ms).max(0.0);
        let index = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, c)| matches(c))
            .min_by(|(_, a), (_, b)| distance(a).total_cmp(&distance(b)))
            .map(|(index, _)| index);
        // A directed exchange can adopt an existing CQ by either participant.
        let index = index.or_else(|| {
            recipient.as_ref().and_then(|to| {
                self.items.iter().position(|c| {
                    c.partner.is_none()
                        && (c.caller == *to || c.caller == sender)
                        && start <= c.expires_ms()
                        && end >= c.last_ms - SLOT_MS
                })
            })
        });
        let index = index.unwrap_or_else(|| {
            if self.items.len() == MAX_CONVERSATIONS {
                let oldest = self
                    .items
                    .iter()
                    .enumerate()
                    .min_by(|(_, a), (_, b)| a.last_ms.total_cmp(&b.last_ms))
                    .map(|(index, _)| index)
                    .unwrap();
                self.items.remove(oldest);
            }
            let display_slot = (0..MAX_CONVERSATIONS)
                .find(|slot| !self.items.iter().any(|c| c.display_slot == *slot))
                .unwrap();
            self.next_id += 1;
            self.items.push(Conversation {
                id: self.next_id,
                display_slot,
                caller: sender.clone(),
                partner: recipient.clone(),
                stage,
                messages: Vec::new(),
                last_ms: end,
                inferred: false,
                heard_from: Vec::new(),
                first_ms: start,
                first_stage: stage,
                calling_station: None,
                steps: std::array::from_fn(|_| None),
            });
            self.items.len() - 1
        });
        let role = match kind {
            MessageKind::Cq => Some((0, sender.clone())),
            MessageKind::Reply => recipient.clone().map(|to| (1, to)),
            MessageKind::Report => Some((2, sender.clone())),
            MessageKind::RogerReport => recipient.clone().map(|to| (3, to)),
            MessageKind::Acknowledgment => Some((4, sender.clone())),
            MessageKind::Signoff if words.last() == Some(&"RR73") => Some((4, sender.clone())),
            _ => None,
        };
        let c = &mut self.items[index];
        if c.partner.is_none()
            && let Some(to) = recipient
        {
            c.partner = Some(if c.caller == sender {
                to
            } else {
                sender.clone()
            });
        }
        if c.messages
            .iter()
            .any(|m| m.message == text && (m.start_ms - start).abs() < 1_000.0)
        {
            return;
        }
        if let Some((rank, call)) = role
            && c.calling_station
                .as_ref()
                .is_none_or(|(old_rank, old_start, _)| {
                    rank < *old_rank || (rank == *old_rank && start < *old_start)
                })
        {
            c.calling_station = Some((rank, start, call));
        }
        if !c.heard_from.contains(&sender) {
            c.heard_from.push(sender.clone());
        }
        if start < c.first_ms {
            c.first_ms = start;
            c.first_stage = stage;
        }
        c.stage = c.stage.max(stage);
        c.last_ms = c.last_ms.max(end);
        let heard = Heard {
            start_ms: start,
            end_ms: end,
            snr_db,
            message: text.into(),
            sender,
            stage,
        };
        if let Some(step) = ExchangeStep::from_message(kind, text) {
            let evidence = &mut c.steps[step as usize];
            if let Some(evidence) = evidence {
                evidence.count += 1;
                if start >= evidence.latest.start_ms {
                    evidence.latest = heard.clone();
                }
            } else {
                *evidence = Some(StepEvidence {
                    latest: heard.clone(),
                    count: 1,
                });
            }
        }
        c.messages.push(heard);
        c.messages.sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
        let both_heard = c.partner.as_ref().is_some_and(|partner| {
            c.heard_from.contains(partner) && c.heard_from.contains(&c.caller)
        });
        c.inferred = c.partner.is_some() && (!both_heard || c.first_stage > Stage::Exchanging);
        if c.messages.len() > MAX_MESSAGES {
            c.messages.drain(..c.messages.len() - MAX_MESSAGES);
        }
    }
}

fn call(word: &str) -> Option<String> {
    let word = word.trim_matches(['<', '>']);
    let valid = word.len() >= 3
        && word
            .bytes()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == b'/')
        && word.bytes().any(|c| c.is_ascii_uppercase())
        && word.bytes().any(|c| c.is_ascii_digit());
    // Four-character Maidenhead locators are not callsigns.
    let grid = word.len() == 4
        && word.as_bytes()[..2]
            .iter()
            .all(|c| (b'A'..=b'R').contains(c))
        && word.as_bytes()[2..].iter().all(u8::is_ascii_digit);
    (valid && !grid).then(|| word.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn feed(t: &mut Conversations, slot: u32, text: &str) {
        let start = f64::from(slot) * SLOT_MS;
        t.update(
            start + 13_000.0,
            [(
                start,
                start + 12_640.0,
                text,
                format!("{}001", "0".repeat(74)).as_str(),
                -12.0,
            )],
        );
    }
    #[test]
    fn cq_and_reversed_pair_progress_without_regressing_on_retries() {
        let mut t = Conversations::default();
        for (slot, msg) in [
            (0, "CQ DX K1ABC FN42"),
            (1, "K1ABC W9XYZ EN37"),
            (2, "W9XYZ K1ABC -12"),
            (3, "K1ABC W9XYZ R-10"),
            (4, "W9XYZ K1ABC -12"),
            (5, "W9XYZ K1ABC RR73"),
        ] {
            feed(&mut t, slot, msg);
        }
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].stage, Stage::Signoff);
        assert!(!t.items[0].inferred);
        assert_eq!(t.items[0].messages.len(), 6);
        feed(&mut t, 7, "K1ABC W9XYZ EN37");
        assert_eq!(t.items.len(), 2);
        let new_id = t
            .items
            .iter()
            .find(|c| c.stage == Stage::Exchanging)
            .unwrap()
            .id;
        feed(&mut t, 5, "W9XYZ K1ABC RR73");
        let new_contact = t.items.iter().find(|c| c.id == new_id).unwrap();
        assert_eq!(new_contact.stage, Stage::Exchanging);
    }
    #[test]
    fn one_sided_late_entry_is_explicit_and_terminal_retention_is_bounded() {
        let mut t = Conversations::default();
        feed(&mut t, 0, "<K1ABC> W9XYZ R-12");
        assert!(t.items[0].inferred);
        assert_eq!(t.items[0].stage, Stage::Acknowledged);
        t.update(12_640.0 + STALE_MS, []);
        assert_eq!(t.items[0].status(t.now_ms), "Stale");
        t.update(12_640.0 + STALE_MS + RETAIN_MS, []);
        assert!(t.items.is_empty());
        feed(&mut t, 10, "K1ABC W9XYZ 73");
        t.update(10.0 * SLOT_MS + 12_640.0 + RETAIN_MS, []);
        assert!(t.items.is_empty());
    }
    #[test]
    fn delayed_messages_merge_and_unresolved_or_nonexchange_payloads_do_not() {
        let mut t = Conversations::default();
        feed(&mut t, 2, "W9XYZ K1ABC -12");
        feed(&mut t, 1, "K1ABC W9XYZ EN37");
        feed(&mut t, 3, "K1ABC W9XYZ R-10");
        feed(&mut t, 3, "K1ABC W9XYZ R-10");
        feed(&mut t, 3, "<...> W9XYZ R-10");
        t.update(t.now_ms, [(0.0, 12_640.0, "K1ABC W9XYZ 73", "000", -12.0)]);
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].messages.len(), 3);
        assert!(!t.items[0].inferred);
    }
    #[test]
    fn late_cq_attaches_to_existing_pair_and_silence_does_not_refresh_it() {
        let mut t = Conversations::default();
        feed(&mut t, 1, "K1ABC W9XYZ EN37");
        feed(&mut t, 0, "CQ K1ABC FN42");
        assert_eq!(t.items.len(), 1);
        assert!(!t.items[0].inferred);
        let end = t.items[0].last_ms;
        t.update(end + STALE_MS, []);
        assert_eq!(t.items[0].status(t.now_ms), "Stale");
        feed(&mut t, 6, "W9XYZ K1ABC -12");
        assert_ne!(t.items[0].status(t.now_ms), "Stale");
        t.update(20.0 * SLOT_MS, []);
        feed(&mut t, 20, "W9XYZ K1ABC -12");
        assert_eq!(t.items.len(), 1);
        assert_eq!(t.items[0].messages.len(), 1);
    }

    #[test]
    fn histories_and_contacts_remain_bounded_and_invalid_times_are_ignored() {
        let mut t = Conversations::default();
        feed(&mut t, 0, "K1ABC W9XYZ EN37");
        feed(&mut t, 1, "W9XYZ K1ABC -12");
        for slot in 2..100 {
            feed(&mut t, slot, "K1ABC W9XYZ -12");
        }
        assert!(
            !t.items[0].inferred,
            "trimming evidence preserves earlier observations"
        );
        assert_eq!(t.items[0].messages.len(), MAX_MESSAGES);
        let payload = format!("{}001", "0".repeat(74));
        t.update(
            t.now_ms,
            [
                (f64::NAN, 0.0, "K1ABC W9XYZ 73", payload.as_str(), -12.0),
                (0.0, -1.0, "K1ABC W9XYZ 73", payload.as_str(), -12.0),
            ],
        );
        assert_eq!(t.items[0].stage, Stage::Reports);
        let start = t.now_ms;
        let texts: Vec<_> = (0..300).map(|i| format!("K1ABC W{i}XYZ -12")).collect();
        t.update(
            start + 13_000.0,
            texts.iter().map(|text| {
                (
                    start,
                    start + 12_640.0,
                    text.as_str(),
                    payload.as_str(),
                    -12.0,
                )
            }),
        );
        assert_eq!(t.items.len(), MAX_CONVERSATIONS);
    }

    #[test]
    fn expected_stages_fill_in_order_with_station_directions() {
        let mut t = Conversations::default();
        feed(&mut t, 0, "CQ K1ABC FN42");
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert_eq!(slots[0].state, SlotState::Received);
        assert_eq!(slots[1].state, SlotState::Expected);
        assert_eq!(slots[2].state, SlotState::Upcoming);
        for (slot, message) in [
            (1, "K1ABC W9XYZ EN37"),
            (2, "W9XYZ K1ABC -12"),
            (3, "K1ABC W9XYZ R-10"),
            (4, "W9XYZ K1ABC RRR"),
        ] {
            feed(&mut t, slot, message);
        }
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert_eq!(slots[1].direction, "W9XYZ → K1ABC");
        assert_eq!(slots[2].direction, "K1ABC → W9XYZ");
        assert_eq!(slots[3].direction, "W9XYZ → K1ABC");
        assert!(
            slots[..5]
                .iter()
                .all(|slot| slot.state == SlotState::Received)
        );
        assert_eq!(slots[5].state, SlotState::Expected);
        feed(&mut t, 5, "K1ABC W9XYZ 73");
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert!(slots.iter().all(|slot| slot.state == SlotState::Received));
        assert!(slots[5].current);
        assert!(
            t.items[0]
                .expectation(t.now_ms)
                .contains("Closing 73 heard")
        );
    }

    #[test]
    fn unheard_stages_and_optional_rr73_closing_are_distinct() {
        let mut t = Conversations::default();
        feed(&mut t, 3, "K1ABC W9XYZ R-10");
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert!(
            slots[..3]
                .iter()
                .all(|slot| slot.state == SlotState::Missing && slot.evidence.is_none())
        );
        assert!(slots[3].current);
        assert_eq!(slots[4].state, SlotState::Expected);
        assert!(t.items[0].expectation(t.now_ms).contains("K1ABC → W9XYZ"));
        t.update(t.items[0].last_ms + STALE_MS, []);
        assert_eq!(
            t.items[0].exchange_slots(t.now_ms)[4].state,
            SlotState::NotHeard
        );
        feed(&mut t, 8, "W9XYZ K1ABC RR73");
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert_eq!(slots[4].state, SlotState::Received);
        assert!(slots[4].current);
        assert_eq!(slots[5].state, SlotState::Optional);
        assert!(t.items[0].expectation(t.now_ms).contains("RR73 heard"));
    }

    #[test]
    fn delayed_evidence_fills_gaps_and_retries_stay_in_their_stage() {
        let mut t = Conversations::default();
        feed(&mut t, 3, "K1ABC W9XYZ R-10");
        feed(&mut t, 2, "W9XYZ K1ABC -12");
        feed(&mut t, 0, "CQ K1ABC FN42");
        feed(&mut t, 1, "K1ABC W9XYZ EN37");
        feed(&mut t, 4, "W9XYZ K1ABC -11");
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert!(
            slots[..4]
                .iter()
                .all(|slot| slot.state == SlotState::Received)
        );
        assert!(
            slots[3].current,
            "a repeated earlier report does not regress progress"
        );
        assert_eq!(slots[2].evidence.as_ref().unwrap().count, 2);
        assert_eq!(
            slots[2].evidence.as_ref().unwrap().latest.message,
            "W9XYZ K1ABC -11"
        );
        assert_eq!(slots[1].direction, "W9XYZ → K1ABC");
        for slot in 5..80 {
            feed(&mut t, slot, "K1ABC W9XYZ R-10");
        }
        let slots = t.items[0].exchange_slots(t.now_ms);
        assert_eq!(
            slots[0].state,
            SlotState::Received,
            "stage summaries survive evidence trimming"
        );
        assert_eq!(slots[3].evidence.as_ref().unwrap().count, 76);
        assert_eq!(slots[1].direction, "W9XYZ → K1ABC");
    }

    #[test]
    fn display_cells_survive_progress_and_expiry_and_vacancies_are_reused() {
        let mut t = Conversations::default();
        feed(&mut t, 0, "CQ K1AAA FN42");
        feed(&mut t, 1, "CQ K2BBB FN42");
        feed(&mut t, 2, "CQ K3CCC FN42");
        let identities: Vec<_> = t.items.iter().map(|c| (c.id, c.display_slot)).collect();
        feed(&mut t, 3, "K2BBB W9XYZ EN37");
        assert_eq!(
            t.items
                .iter()
                .map(|c| (c.id, c.display_slot))
                .collect::<Vec<_>>(),
            identities
        );
        t.update(118_000.0, []);
        assert_eq!(
            t.items
                .iter()
                .map(|c| (c.id, c.display_slot))
                .collect::<Vec<_>>(),
            identities[1..]
        );
        feed(&mut t, 8, "CQ K4DDD FN42");
        assert_eq!(t.items[0].display_slot, 0);
        assert_eq!(t.items[0].caller, "K4DDD");
        assert_eq!(
            t.items
                .iter()
                .skip(1)
                .map(|c| (c.id, c.display_slot))
                .collect::<Vec<_>>(),
            identities[1..]
        );
    }

    #[test]
    fn station_can_have_multiple_partners_and_cq_is_not_a_pair() {
        let mut t = Conversations::default();
        feed(&mut t, 0, "CQ K1ABC FN42");
        feed(&mut t, 1, "K1ABC W9XYZ EN37");
        feed(&mut t, 1, "K1ABC N0CALL EM00");
        assert_eq!(t.items.len(), 2);
        feed(&mut t, 2, "CQ K1ABC FN42");
        assert_eq!(t.items.len(), 3);
    }
}
