//! Selection policy for starting a conversation from a caller-prioritized list.
use crate::{Conversation, ExchangeStep, SLOT_MS, Stage};

/// Maximum age of the CQ frame's end at the intended transmit-slot boundary.
pub const CQ_FRESHNESS_MS: f64 = 5_000.0;

/// Select the first eligible CQ conversation, preserving the slice's priority order.
///
/// Call this only when the application has no active conversation. This function
/// does not replace an active exchange, sort candidates, consume a CQ, or authorize
/// transmission. The caller owns those policies and any exclusion of its own call.
/// The returned reference borrows the original candidate, with no clone or mutation.
///
/// `transmit_slot_start_ms` is the intended 15-second transmit-slot boundary, not
/// decode-arrival time or the time this function happens to run. It must use the
/// same clock epoch as the conversations. The epoch need not itself be slot aligned.
///
/// A candidate must still be calling with no partner. Its latest CQ must belong
/// to the immediately preceding slot (frame starts are assigned to the nearest
/// nominal slot boundary), have ended by the transmit boundary, and have ended
/// no more than five seconds earlier. Both checks are needed: late recovery
/// arrival never makes an older CQ fresh, nor can an unusually long old frame
/// satisfy selection merely because its end is recent. The five-second boundary
/// is inclusive. Invalid times and an empty/ineligible list return None.
///
/// ```
/// use ft8_conversations::{Conversations, select_active_conversation};
/// let mut tracker = Conversations::default();
/// let payload = format!("{}001", "0".repeat(74));
/// tracker.update(14_000.0, [
///     (500.0, 13_140.0, "CQ K1ABC FN42", payload.as_str(), -12.0),
/// ]);
/// let selected = select_active_conversation(&tracker.items, 15_000.0).unwrap();
/// assert_eq!(selected.caller, "K1ABC");
/// assert!(select_active_conversation(&tracker.items, 30_000.0).is_none());
/// ```
pub fn select_active_conversation(
    conversations: &[Conversation],
    transmit_slot_start_ms: f64,
) -> Option<&Conversation> {
    if !transmit_slot_start_ms.is_finite() {
        return None;
    }
    conversations.iter().find(|conversation| {
        if conversation.stage != Stage::Calling || conversation.partner.is_some() {
            return false;
        }
        let Some(cq) = &conversation.steps[ExchangeStep::Cq as usize] else {
            return false;
        };
        let cq = &cq.latest;
        let age_ms = transmit_slot_start_ms - cq.end_ms;
        cq.start_ms.is_finite()
            && cq.end_ms.is_finite()
            && cq.end_ms >= cq.start_ms
            && ((cq.start_ms - transmit_slot_start_ms) / SLOT_MS).round() == -1.0
            && (0.0..=CQ_FRESHNESS_MS).contains(&age_ms)
    })
}

/// Policy outcome for ending the current attempt. Transport failures remain external.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Completed,
    Unanswered,
    AttemptLimit,
    MissedWindow,
    PartnerAnsweredAnother,
    PartnerResumedCq,
}
impl EndReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "Completed",
            Self::Unanswered => "No response after 4 tries",
            Self::AttemptLimit => "Attempt limit",
            Self::MissedWindow => "Interrupted",
            Self::PartnerAnsweredAnother => "Partner answered another station",
            Self::PartnerResumedCq => "Partner resumed CQ",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListenReason {
    NoFreshCq,
    OutsideTransmitWindow,
    ReceiveIncomplete,
    FinalReplyWindow,
    InvalidTime,
}
impl ListenReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::NoFreshCq => "Listening · waiting for a fresh CQ",
            Self::OutsideTransmitWindow => "Listening · waiting for our transmit slot",
            Self::ReceiveIncomplete => "Listening · receive slot still decoding",
            Self::FinalReplyWindow => "Listening · waiting for the final reply window",
            Self::InvalidTime => "Listening · invalid clock time",
        }
    }
}

/// Retained attempt facts supplied by the executor. Only successfully completed
/// audio increments the counters; reservations and canceled audio do not count.
#[derive(Clone, Copy, Debug)]
pub struct Attempt<'a> {
    pub contact: &'a crate::Contact,
    pub started_ms: f64,
    pub last_tx_ms: Option<f64>,
    pub transmissions: usize,
    pub retries: usize,
    pub sent_step: Option<usize>,
    pub require_receive_progress: bool,
    pub completed_receive_slot: Option<i64>,
}

/// Queue eligibility facts; priority is defined solely by input order.
pub struct Candidate<'a> {
    pub conversation: &'a Conversation,
    pub attempted_cq_ms: f64,
    pub completed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum PolicyDecision {
    Listen(ListenReason),
    Start {
        conversation_id: u64,
        target: String,
        cq_start_ms: f64,
    },
    Transmit {
        start_ms: f64,
        receive_slot: i64,
        step: usize,
        message: String,
    },
    End(EndReason),
}

/// Audio starts 500 ms into the nominal slot; reservation leaves time for PTT.
pub const AUDIO_OFFSET_MS: f64 = 500.0;
pub const RESERVATION_LEAD_MS: f64 = 1_800.0;
pub const RESERVATION_DEADLINE_MS: f64 = 200.0;
pub const INITIAL_CALL_LIMIT: usize = 4;
pub const STAGE_TRANSMISSION_LIMIT: usize = 3;
pub const TOTAL_TRANSMISSION_LIMIT: usize = 8;
pub const ATTEMPT_TIMEOUT_MS: f64 = 180_000.0;
pub const UNANSWERED_PENALTY_MS: f64 = 180_000.0;

impl Attempt<'_> {
    pub fn receive_ready(&self, transmit_ms: f64) -> bool {
        if !self.require_receive_progress
            || self.last_tx_ms.is_none()
            || self.contact.state() == crate::QsoState::SendingFarewell
        {
            return true;
        }
        let receive_slot = ((transmit_ms - AUDIO_OFFSET_MS) / SLOT_MS).round() as i64 - 1;
        self.completed_receive_slot
            .is_some_and(|slot| slot >= receive_slot)
            // A validated CQ in this partner turn explicitly invites another
            // initial call, even while the rest of the slot is still decoding.
            || (self.contact.state() == crate::QsoState::Calling
                && self.contact.receive_slot == receive_slot)
            || (self.contact.last_directed_slot == Some(receive_slot)
                // Repeated partner evidence requests the same response again;
                // it need not advance our outgoing stage to authorize a retry.
                && self.contact.state() != crate::QsoState::Calling)
    }

    /// Evaluate limits independently of device readiness. Keep the final allowed
    /// transmission's reply window open before ending an unanswered attempt.
    pub fn end_reason(&self, now_ms: f64) -> Option<EndReason> {
        if !now_ms.is_finite() {
            return None;
        }
        if let Some(reason) = self.contact.abandoned() {
            return Some(reason);
        }
        if self.contact.state() == crate::QsoState::Closed {
            return Some(EndReason::Completed);
        }
        let reply_window_ended = self
            .last_tx_ms
            .is_some_and(|last| now_ms >= last + 2.0 * SLOT_MS - RESERVATION_LEAD_MS);
        if self.contact.state() == crate::QsoState::Calling
            && self.transmissions >= INITIAL_CALL_LIMIT
            && reply_window_ended
        {
            Some(EndReason::Unanswered)
        } else if now_ms - self.started_ms > ATTEMPT_TIMEOUT_MS
            || ((self.transmissions >= TOTAL_TRANSMISSION_LIMIT
                || (self.contact.state() != crate::QsoState::Calling
                    && self.retries >= STAGE_TRANSMISSION_LIMIT))
                && reply_window_ended)
        {
            Some(EndReason::AttemptLimit)
        } else {
            None
        }
    }
}

/// Updated attempt facts after successful audio. Failed/canceled transport must
/// never call this function. Sent audio is not inserted into received evidence.
#[derive(Clone, Debug, PartialEq)]
pub struct TransmissionResult {
    pub transmissions: usize,
    pub retries: usize,
    pub sent_step: Option<usize>,
    pub last_tx_ms: Option<f64>,
    pub complete: bool,
}
pub fn after_transmission(active: Attempt<'_>, step: usize, start_ms: f64) -> TransmissionResult {
    TransmissionResult {
        transmissions: active.transmissions + 1,
        retries: if step != active.contact.step() {
            0
        } else if active.sent_step == Some(step) {
            active.retries + 1
        } else {
            1
        },
        sent_step: Some(step),
        last_tx_ms: Some(start_ms),
        complete: step == crate::QsoState::SendingFarewell.step(),
    }
}

/// The contact policy's single entry point. No browser, radio, PCM or log access.
/// Select fresh CQs in priority order until partner report evidence establishes
/// a QSO. An established contact keeps its turn/parity and exclusive priority.
/// An unestablished attempt can return Start to replace its target. The
/// executor records successful audio and calls again with updated facts. Recheck
/// before keying and after PTT acknowledgment when evidence may have changed.
pub fn decide(
    active: Option<Attempt<'_>>,
    candidates: &[Candidate<'_>],
    now_ms: f64,
    mine: &str,
    grid: &str,
    report: i32,
) -> PolicyDecision {
    if !now_ms.is_finite() {
        return PolicyDecision::Listen(ListenReason::InvalidTime);
    }
    if let Some(active) = active {
        if let Some(reason) = active.end_reason(now_ms) {
            return PolicyDecision::End(reason);
        }
        if !active.contact.is_established() && active.last_tx_ms.is_some() {
            let selected = decide(None, candidates, now_ms, mine, grid, report);
            if matches!(&selected, PolicyDecision::Start { target, .. }
                if target != &active.contact.target)
            {
                return selected;
            }
        }
        let mut due = (active.contact.receive_slot + 1) as f64 * SLOT_MS + AUDIO_OFFSET_MS;
        if let Some(last) = active.last_tx_ms {
            due = due.max(last + 2.0 * SLOT_MS);
            if due < now_ms + RESERVATION_DEADLINE_MS {
                due += ((now_ms + RESERVATION_DEADLINE_MS - due) / (2.0 * SLOT_MS)).ceil()
                    * 2.0
                    * SLOT_MS;
            }
        }
        if now_ms > due - RESERVATION_DEADLINE_MS {
            return PolicyDecision::End(EndReason::MissedWindow);
        }
        if now_ms < due - RESERVATION_LEAD_MS {
            return PolicyDecision::Listen(ListenReason::OutsideTransmitWindow);
        }
        if !active.receive_ready(due) {
            return PolicyDecision::Listen(ListenReason::ReceiveIncomplete);
        }
        if (active.contact.state() == crate::QsoState::Calling
            && active.transmissions >= INITIAL_CALL_LIMIT)
            || active.transmissions >= TOTAL_TRANSMISSION_LIMIT
            || (active.contact.state() != crate::QsoState::Calling
                && active.retries >= STAGE_TRANSMISSION_LIMIT)
        {
            return PolicyDecision::Listen(ListenReason::FinalReplyWindow);
        }
        return PolicyDecision::Transmit {
            start_ms: due,
            receive_slot: active.contact.receive_slot,
            step: active.contact.step(),
            message: active.contact.message(mine, grid, report),
        };
    }
    let boundary = (now_ms / SLOT_MS).round() * SLOT_MS;
    let due = boundary + AUDIO_OFFSET_MS;
    if !(due - RESERVATION_LEAD_MS..=due - RESERVATION_DEADLINE_MS).contains(&now_ms) {
        return PolicyDecision::Listen(ListenReason::OutsideTransmitWindow);
    }
    for candidate in candidates {
        let c = candidate.conversation;
        if candidate.completed || c.caller == mine {
            continue;
        }
        if select_active_conversation(std::slice::from_ref(c), boundary).is_some() {
            let start = c.steps[ExchangeStep::Cq as usize]
                .as_ref()
                .unwrap()
                .latest
                .start_ms;
            if start > candidate.attempted_cq_ms {
                return PolicyDecision::Start {
                    conversation_id: c.id,
                    target: c.caller.clone(),
                    cq_start_ms: start,
                };
            }
        }
    }
    PolicyDecision::Listen(ListenReason::NoFreshCq)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Conversations;

    fn attempt(contact: &crate::Contact) -> Attempt<'_> {
        Attempt {
            contact,
            started_ms: contact.opened_ms,
            last_tx_ms: None,
            transmissions: 0,
            retries: 0,
            sent_step: None,
            require_receive_progress: false,
            completed_receive_slot: None,
        }
    }
    fn policy(
        active: Option<Attempt<'_>>,
        candidates: &[Candidate<'_>],
        now: f64,
    ) -> PolicyDecision {
        decide(active, candidates, now, "W9XYZ", "EN37", -12)
    }

    #[test]
    fn policy_selects_priority_cq_and_respects_consumed_completed_and_own_calls() {
        let mut tracker = Conversations::default();
        for call in ["W9XYZ", "K1DONE", "K1USED", "K1NEW"] {
            hear(&mut tracker, 500.0, 13_140.0, &format!("CQ {call} FN42"));
        }
        let candidates: Vec<_> = tracker
            .items
            .iter()
            .map(|c| Candidate {
                conversation: c,
                completed: c.caller == "K1DONE",
                attempted_cq_ms: if c.caller == "K1USED" {
                    500.0
                } else {
                    f64::NEG_INFINITY
                },
            })
            .collect();
        assert!(
            matches!(policy(None, &candidates, 14_000.0), PolicyDecision::Start { target, .. } if target == "K1NEW")
        );
        assert_eq!(
            policy(None, &candidates, 30_000.0),
            PolicyDecision::Listen(ListenReason::NoFreshCq)
        );
    }

    #[test]
    fn active_contact_keeps_priority_and_listens_outside_its_transmit_turn() {
        let contact = crate::Contact::new("K1ABC".into(), 500.0);
        let active = attempt(&contact);
        assert_eq!(
            policy(Some(active), &[], 29_000.0),
            PolicyDecision::End(EndReason::MissedWindow)
        );
        assert_eq!(
            policy(Some(active), &[], 12_000.0),
            PolicyDecision::Listen(ListenReason::OutsideTransmitWindow)
        );
        assert!(
            matches!(policy(Some(active), &[], 14_000.0), PolicyDecision::Transmit { ref message, start_ms: 15_500.0, .. } if message == "K1ABC W9XYZ EN37")
        );
        let mut waiting = Conversations::default();
        hear(&mut waiting, 500.0, 13_140.0, "CQ K1NEW FN42");
        let candidates = [Candidate {
            conversation: &waiting.items[0],
            completed: false,
            attempted_cq_ms: f64::NEG_INFINITY,
        }];
        assert!(
            matches!(policy(Some(active), &candidates, 14_000.0), PolicyDecision::Transmit { message, .. } if message.starts_with("K1ABC "))
        );
    }

    #[test]
    fn fresh_cq_can_replace_an_attempt_but_not_a_report_established_qso() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, 60_500.0, 73_140.0, "CQ N5IH EM20");
        hear(&mut tracker, 60_500.0, 73_140.0, "CQ N7IH EM10");
        let candidates: Vec<_> = tracker
            .items
            .iter()
            .map(|c| Candidate {
                conversation: c,
                attempted_cq_ms: f64::NEG_INFINITY,
                completed: false,
            })
            .collect();
        for (message, established) in [
            (None, false),
            (Some("W9XYZ KM4ESU FN42"), false),
            (Some("W9XYZ KM4ESU -10"), true),
            (Some("W9XYZ KM4ESU R-10"), true),
        ] {
            let mut contact = crate::Contact::new("KM4ESU".into(), 500.0);
            if let Some(message) = message {
                contact.hear(
                    "W9XYZ",
                    30_500.0,
                    message,
                    &format!("{}001", "0".repeat(74)),
                );
            }
            let active = Attempt {
                last_tx_ms: Some(45_500.0),
                transmissions: 2,
                ..attempt(&contact)
            };
            let decision = policy(Some(active), &candidates, 74_000.0);
            if established {
                assert!(
                    matches!(decision, PolicyDecision::Transmit { message, .. } if message.starts_with("KM4ESU "))
                );
            } else {
                assert!(
                    matches!(decision, PolicyDecision::Start { target, .. } if target == "N5IH")
                );
                assert!(
                    !matches!(
                        policy(Some(active), &candidates, 104_000.0),
                        PolicyDecision::Start { .. }
                    ),
                    "stale CQ must not replace an attempt"
                );
            }
        }
    }

    #[test]
    fn fourth_call_keeps_final_reply_window_and_a_reply_continues_the_exchange() {
        let mut contact = crate::Contact::new("K1ABC".into(), 500.0);
        let mut active = attempt(&contact);
        for i in 0..4 {
            let start = 15_500.0 + f64::from(i) * 30_000.0;
            let result = after_transmission(active, 0, start);
            assert!(!result.complete);
            active.transmissions = result.transmissions;
            active.retries = result.retries;
            active.sent_step = result.sent_step;
            active.last_tx_ms = result.last_tx_ms;
        }
        assert_eq!(active.end_reason(133_699.0), None);
        assert_eq!(active.end_reason(133_700.0), Some(EndReason::Unanswered));
        let facts = (active.transmissions, active.last_tx_ms);
        let payload = format!("{}001", "0".repeat(74));
        contact.hear("W9XYZ", 120_500.0, "W9XYZ K1ABC -10", &payload);
        let active = Attempt {
            transmissions: facts.0,
            last_tx_ms: facts.1,
            ..attempt(&contact)
        };
        assert!(
            matches!(policy(Some(active), &[], 134_000.0), PolicyDecision::Transmit { message, .. } if message == "K1ABC W9XYZ R-12")
        );
    }

    #[test]
    fn current_partner_cq_permits_retry_but_old_and_wrong_turn_cqs_do_not() {
        let mut contact = crate::Contact::new("K1ABC".into(), 500.0);
        let payload = format!("{}001", "0".repeat(74));
        let ready = |contact: &crate::Contact, due| {
            Attempt {
                last_tx_ms: Some(15_500.0),
                transmissions: 1,
                retries: 1,
                sent_step: Some(0),
                require_receive_progress: true,
                ..attempt(contact)
            }
            .receive_ready(due)
        };
        assert!(!ready(&contact, 45_500.0));
        contact.hear("W9XYZ", 15_500.0, "CQ K1ABC FN42", &payload);
        assert!(!ready(&contact, 45_500.0), "wrong transmit parity");
        contact.hear("W9XYZ", 30_500.0, "CQ N0CALL EN37", &payload);
        assert!(!ready(&contact, 45_500.0), "unrelated caller");
        contact.hear("W9XYZ", 30_500.0, "CQ K1ABC FN42", &payload);
        assert!(ready(&contact, 45_500.0));
        assert!(
            !ready(&contact, 75_500.0),
            "CQ cannot authorize a later turn"
        );
    }

    #[test]
    fn retries_wait_for_receive_completion_but_a_new_partner_reply_can_advance() {
        let mut contact = crate::Contact::new("K1ABC".into(), 500.0);
        let active = Attempt {
            last_tx_ms: Some(15_500.0),
            transmissions: 1,
            retries: 1,
            sent_step: Some(0),
            require_receive_progress: true,
            ..attempt(&contact)
        };
        assert_eq!(
            policy(Some(active), &[], 44_000.0),
            PolicyDecision::Listen(ListenReason::ReceiveIncomplete)
        );
        assert!(matches!(
            policy(
                Some(Attempt {
                    completed_receive_slot: Some(2),
                    ..active
                }),
                &[],
                44_000.0
            ),
            PolicyDecision::Transmit { step: 0, .. }
        ));
        let payload = format!("{}001", "0".repeat(74));
        contact.hear("W9XYZ", 30_500.0, "W9XYZ K1ABC -10", &payload);
        let active = Attempt {
            last_tx_ms: Some(15_500.0),
            transmissions: 1,
            sent_step: Some(0),
            require_receive_progress: true,
            ..attempt(&contact)
        };
        assert!(matches!(
            policy(Some(active), &[], 44_000.0),
            PolicyDecision::Transmit { step: 2, .. }
        ));
    }

    #[test]
    fn repeated_directed_stages_authorize_retries_only_in_their_receive_turn() {
        fn active(contact: &crate::Contact, step: usize) -> Attempt<'_> {
            Attempt {
                last_tx_ms: Some(45_500.0),
                transmissions: 2,
                retries: 1,
                sent_step: Some(step),
                require_receive_progress: true,
                ..attempt(contact)
            }
        }
        let payload = format!("{}001", "0".repeat(74));
        for (text, step) in [
            ("W9XYZ K1ABC FN42", 1),
            ("W9XYZ K1ABC -04", 2),
            ("W9XYZ K1ABC R-04", 3),
        ] {
            let mut contact = crate::Contact::new("K1ABC".into(), 500.0);
            contact.hear("W9XYZ", 30_500.0, text, &payload);
            assert!(
                !active(&contact, step).receive_ready(75_500.0),
                "old report: {text}"
            );
            contact.hear("W9XYZ", 45_500.0, text, &payload);
            assert!(
                !active(&contact, step).receive_ready(75_500.0),
                "wrong parity: {text}"
            );
            contact.hear("W9XYZ", 60_500.0, text, &payload);
            assert!(
                matches!(policy(Some(active(&contact, step)), &[], 74_000.0),
                PolicyDecision::Transmit { step: actual, .. } if actual == step)
            );
            assert!(
                !active(&contact, step).receive_ready(105_500.0),
                "cannot reuse report: {text}"
            );
        }
    }

    #[test]
    fn later_stage_timeout_and_completion_are_policy_results() {
        let mut contact = crate::Contact::new("K1ABC".into(), 500.0);
        let payload = format!("{}001", "0".repeat(74));
        contact.hear("W9XYZ", 30_500.0, "W9XYZ K1ABC -10", &payload);
        let active = Attempt {
            retries: 3,
            transmissions: 4,
            last_tx_ms: Some(45_500.0),
            ..attempt(&contact)
        };
        assert_eq!(active.end_reason(73_699.0), None);
        assert_eq!(active.end_reason(73_700.0), Some(EndReason::AttemptLimit));
        assert_eq!(
            attempt(&contact).end_reason(180_501.0),
            Some(EndReason::AttemptLimit)
        );
        contact.hear("W9XYZ", 60_500.0, "W9XYZ K1ABC RR73", &payload);
        assert!(after_transmission(attempt(&contact), 4, 75_500.0).complete);
    }

    fn hear(tracker: &mut Conversations, start: f64, end: f64, text: &str) {
        let payload = format!("{}001", "0".repeat(74));
        tracker.update(end, [(start, end, text, payload.as_str(), -12.0)]);
    }

    #[test]
    fn first_eligible_priority_wins_even_when_a_later_candidate_has_a_newer_cq() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, 0.0, 12_640.0, "CQ K1ABC FN42");
        hear(&mut tracker, 500.0, 13_140.0, "CQ W9XYZ EN37");
        let selected = select_active_conversation(&tracker.items, 15_000.0).unwrap();
        assert!(std::ptr::eq(selected, &tracker.items[0]));
        tracker.items.reverse();
        let selected = select_active_conversation(&tracker.items, 15_000.0).unwrap();
        assert_eq!(selected.caller, "W9XYZ");
    }

    #[test]
    fn skips_old_and_engaged_conversations_before_selecting_a_fresh_cq() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, -30_000.0, -17_360.0, "CQ K1OLD FN42");
        hear(&mut tracker, 0.0, 12_640.0, "CQ K1BUSY FN42");
        hear(&mut tracker, 500.0, 13_140.0, "K1BUSY W9XYZ EN37");
        hear(&mut tracker, 0.0, 12_640.0, "CQ K1NEW FN42");
        assert_eq!(
            select_active_conversation(&tracker.items, 15_000.0)
                .unwrap()
                .caller,
            "K1NEW"
        );
    }

    #[test]
    fn freshness_is_inclusive_and_measured_from_frame_end() {
        for (end, eligible) in [
            (9_999.0, false),
            (10_000.0, true),
            (12_640.0, true),
            (15_000.0, true),
            (15_001.0, false),
        ] {
            let mut tracker = Conversations::default();
            hear(&mut tracker, 500.0, end, "CQ K1ABC FN42");
            assert_eq!(
                select_active_conversation(&tracker.items, 15_000.0).is_some(),
                eligible,
                "end={end}"
            );
        }
    }

    #[test]
    fn previous_slot_is_required_even_when_an_old_frame_ends_recently() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, -15_000.0, 13_000.0, "CQ K1ABC FN42");
        assert!(select_active_conversation(&tracker.items, 15_000.0).is_none());
        let mut current = Conversations::default();
        hear(&mut current, 15_000.0, 15_000.0, "CQ K1ABC FN42");
        assert!(select_active_conversation(&current.items, 15_000.0).is_none());
    }

    #[test]
    fn late_arrival_never_refreshes_cq_and_old_cq_cannot_roll_to_another_slot() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, 500.0, 13_140.0, "CQ K1ABC FN42");
        tracker.update(44_000.0, []);
        assert!(select_active_conversation(&tracker.items, 45_000.0).is_none());
        hear(&mut tracker, 30_500.0, 43_140.0, "CQ K1ABC FN42");
        assert_eq!(
            select_active_conversation(&tracker.items, 45_000.0)
                .unwrap()
                .caller,
            "K1ABC"
        );
    }

    #[test]
    fn accepts_timing_offsets_and_a_non_utc_epoch() {
        for (boundary, start) in [
            (15_000.0, -500.0),
            (15_000.0, 500.0),
            (0.0, -14_500.0),
            (18_200.0, 3_700.0),
        ] {
            let mut tracker = Conversations::default();
            hear(&mut tracker, start, start + 12_640.0, "CQ K1ABC FN42");
            assert!(select_active_conversation(&tracker.items, boundary).is_some());
        }
    }

    #[test]
    fn no_cq_or_invalid_times_return_none() {
        assert!(select_active_conversation(&[], 15_000.0).is_none());
        let mut tracker = Conversations::default();
        hear(&mut tracker, 500.0, 13_140.0, "K1ABC W9XYZ -12");
        assert!(select_active_conversation(&tracker.items, 15_000.0).is_none());
        let mut cq = Conversations::default();
        hear(&mut cq, 500.0, 13_140.0, "CQ K1ABC FN42");
        for invalid in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert!(select_active_conversation(&cq.items, invalid).is_none());
        }
    }

    #[test]
    fn selection_is_pure_and_leaves_active_conversation_handling_to_the_caller() {
        let mut tracker = Conversations::default();
        hear(&mut tracker, 500.0, 13_140.0, "CQ K1ABC FN42");
        let selected = select_active_conversation(&tracker.items, 15_000.0).unwrap();
        let again = select_active_conversation(&tracker.items, 15_000.0).unwrap();
        assert!(std::ptr::eq(selected, again));
        assert_eq!(tracker.items.len(), 1);
        assert_eq!(tracker.items[0].messages.len(), 1);
    }
}
