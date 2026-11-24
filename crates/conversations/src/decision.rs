//! Pure, station-independent decisions from received conversation evidence.
//!
//! This module does not know our callsign, send audio, advance time, authorize a
//! transmission, or count local playback as received evidence. Call
//! [`Conversation::decision`] after updating the tracker. The application compares
//! the recommended sender with its own callsign, then applies its slot, freshness,
//! retry and operator controls. Repeated calls without new evidence are identical.
use crate::{Conversation, ExchangeStep, Stage};

/// Checkboxes describe actual received evidence, including retained stage summaries.
/// Missing earlier stages remain false even when later evidence establishes progress.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReceivedStages {
    pub cq: bool,
    pub reply: bool,
    pub report: bool,
    pub roger_report: bool,
    /// Either RRR or RR73 was heard.
    pub acknowledgment: bool,
    /// A standalone closing 73 was heard; RR73 does not check this box.
    pub farewell: bool,
}

impl ReceivedStages {
    pub fn received(self, step: ExchangeStep) -> bool {
        match step {
            ExchangeStep::Cq => self.cq,
            ExchangeStep::Callback => self.reply,
            ExchangeStep::Report => self.report,
            ExchangeStep::RogerReport => self.roger_report,
            ExchangeStep::Acknowledgment => self.acknowledgment,
            ExchangeStep::Farewell => self.farewell,
        }
    }
}

/// An expected transmission, not permission to key a transmitter.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextAction {
    /// Message category to send; reports, grids and waveform packing are external.
    pub step: ExchangeStep,
    /// None after a lone CQ: no responding callsign has been observed yet.
    pub sender: Option<String>,
    pub recipient: String,
    /// Closing 73 is optional after RR73, but expected after RRR.
    pub optional: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Recommendation {
    Next(NextAction),
    /// Closing 73 was received. This does not assert mutual QSO confirmation.
    Complete,
    /// No ordinary exchange evidence is available.
    InsufficientEvidence,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConversationDecision {
    pub received: ReceivedStages,
    pub recommendation: Recommendation,
}

impl Conversation {
    /// Inspect received stages and recommend the next exchange action.
    ///
    /// Progress follows the furthest received stage, rather than restarting at a
    /// missed earlier stage. The next sender is the other member of the observed
    /// pair, and the recipient is the sender of that furthest-stage evidence.
    /// This also handles late entry without assuming `caller` means CQ initiator.
    /// Retries of earlier stages cannot roll progress back. No times or local
    /// station identity enter this decision; even stale evidence yields the same
    /// logical expectation. Scheduling and expiry policy belong to the caller.
    ///
    /// ```
    /// use ft8_conversations::{Conversations, ExchangeStep, Recommendation};
    /// let mut tracker = Conversations::default();
    /// let payload = format!("{}001", "0".repeat(74));
    /// tracker.update(28_000.0, [
    ///     (15_000.0, 27_640.0, "K1ABC W9XYZ EN37", payload.as_str(), -12.0),
    /// ]);
    /// let decision = tracker.items[0].decision();
    /// assert!(!decision.received.cq); // Missed CQ stays unchecked.
    /// assert!(decision.received.reply);
    /// let Recommendation::Next(action) = decision.recommendation else { panic!() };
    /// assert_eq!(action.step, ExchangeStep::Report);
    /// assert_eq!(action.sender.as_deref(), Some("K1ABC"));
    /// assert_eq!(action.recipient, "W9XYZ");
    /// ```
    pub fn decision(&self) -> ConversationDecision {
        let has = |step: ExchangeStep| self.steps[step as usize].is_some();
        let received = ReceivedStages {
            cq: has(ExchangeStep::Cq),
            reply: has(ExchangeStep::Callback),
            report: has(ExchangeStep::Report),
            roger_report: has(ExchangeStep::RogerReport),
            acknowledgment: has(ExchangeStep::Acknowledgment),
            farewell: has(ExchangeStep::Farewell),
        };
        let recommendation = if received.farewell {
            Recommendation::Complete
        } else if let Some(index) = self.steps.iter().rposition(Option::is_some) {
            let heard = &self.steps[index].as_ref().unwrap().latest;
            let sender = if heard.sender == self.caller {
                self.partner.clone()
            } else if self.partner.as_ref() == Some(&heard.sender) {
                Some(self.caller.clone())
            } else {
                None
            };
            Recommendation::Next(NextAction {
                step: ExchangeStep::ALL[index + 1],
                sender,
                recipient: heard.sender.clone(),
                optional: received.acknowledgment && self.stage == Stage::Signoff,
            })
        } else {
            Recommendation::InsufficientEvidence
        };
        ConversationDecision {
            received,
            recommendation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Conversations, SLOT_MS};

    fn feed(tracker: &mut Conversations, slot: u32, text: &str) {
        let start = f64::from(slot) * SLOT_MS;
        let payload = format!("{}001", "0".repeat(74));
        tracker.update(
            start + 13_000.0,
            [(start, start + 12_640.0, text, payload.as_str(), -12.0)],
        );
    }

    fn expect_next(
        decision: &ConversationDecision,
        step: ExchangeStep,
        sender: Option<&str>,
        recipient: &str,
        optional: bool,
    ) {
        assert_eq!(
            decision.recommendation,
            Recommendation::Next(NextAction {
                step,
                sender: sender.map(str::to_owned),
                recipient: recipient.into(),
                optional,
            })
        );
    }

    #[test]
    fn every_stage_checks_only_received_evidence_and_selects_the_next_actor() {
        let mut tracker = Conversations::default();
        let messages = [
            "CQ K1ABC FN42",
            "K1ABC W9XYZ EN37",
            "W9XYZ K1ABC -12",
            "K1ABC W9XYZ R-10",
            "W9XYZ K1ABC RRR",
            "K1ABC W9XYZ 73",
        ];
        for (index, message) in messages.into_iter().enumerate() {
            feed(&mut tracker, index as u32, message);
            let decision = tracker.items[0].decision();
            for (stage, step) in ExchangeStep::ALL.into_iter().enumerate() {
                assert_eq!(decision.received.received(step), stage <= index);
            }
            if index == 5 {
                assert_eq!(decision.recommendation, Recommendation::Complete);
            } else {
                let (sender, recipient) = match index {
                    0 => (None, "K1ABC"),
                    1 | 3 => (Some("K1ABC"), "W9XYZ"),
                    _ => (Some("W9XYZ"), "K1ABC"),
                };
                expect_next(
                    &decision,
                    ExchangeStep::ALL[index + 1],
                    sender,
                    recipient,
                    false,
                );
            }
        }
    }

    #[test]
    fn late_entry_skips_missing_history_without_inventing_checkmarks_or_roles() {
        for (message, received_step, next_step, sender, recipient) in [
            (
                "K1ABC W9XYZ EN37",
                ExchangeStep::Callback,
                ExchangeStep::Report,
                "K1ABC",
                "W9XYZ",
            ),
            (
                "W9XYZ K1ABC -12",
                ExchangeStep::Report,
                ExchangeStep::RogerReport,
                "W9XYZ",
                "K1ABC",
            ),
            (
                "K1ABC W9XYZ R-10",
                ExchangeStep::RogerReport,
                ExchangeStep::Acknowledgment,
                "K1ABC",
                "W9XYZ",
            ),
            (
                "W9XYZ K1ABC RRR",
                ExchangeStep::Acknowledgment,
                ExchangeStep::Farewell,
                "W9XYZ",
                "K1ABC",
            ),
        ] {
            let mut tracker = Conversations::default();
            feed(&mut tracker, 0, message);
            let decision = tracker.items[0].decision();
            for step in ExchangeStep::ALL {
                assert_eq!(decision.received.received(step), step == received_step);
            }
            expect_next(&decision, next_step, Some(sender), recipient, false);
        }
    }

    #[test]
    fn rr73_makes_closing_optional_but_does_not_count_as_a_received_73() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 0, "W9XYZ K1ABC RR73");
        let decision = tracker.items[0].decision();
        assert!(decision.received.acknowledgment);
        assert!(!decision.received.farewell);
        expect_next(
            &decision,
            ExchangeStep::Farewell,
            Some("W9XYZ"),
            "K1ABC",
            true,
        );
        feed(&mut tracker, 1, "K1ABC W9XYZ 73");
        assert_eq!(
            tracker.items[0].decision().recommendation,
            Recommendation::Complete
        );
    }

    #[test]
    fn repeated_rrr_cannot_undo_a_previously_heard_rr73() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 0, "W9XYZ K1ABC RR73");
        feed(&mut tracker, 1, "W9XYZ K1ABC RRR");
        expect_next(
            &tracker.items[0].decision(),
            ExchangeStep::Farewell,
            Some("W9XYZ"),
            "K1ABC",
            true,
        );
    }

    #[test]
    fn delayed_evidence_fills_checkmarks_and_retries_never_regress_the_action() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 3, "K1ABC W9XYZ R-10");
        let before = tracker.items[0].decision();
        feed(&mut tracker, 0, "CQ K1ABC FN42");
        feed(&mut tracker, 1, "K1ABC W9XYZ EN37");
        feed(&mut tracker, 2, "W9XYZ K1ABC -12");
        feed(&mut tracker, 4, "W9XYZ K1ABC -11");
        feed(&mut tracker, 4, "W9XYZ K1ABC -11");
        let after = tracker.items[0].decision();
        assert_eq!(before.recommendation, after.recommendation);
        assert!(after.received.cq && after.received.reply && after.received.report);
        assert!(!after.received.acknowledgment);
    }

    #[test]
    fn decision_is_pure_and_survives_trimmed_history_and_silence() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 0, "CQ K1ABC FN42");
        feed(&mut tracker, 1, "K1ABC W9XYZ EN37");
        feed(&mut tracker, 2, "W9XYZ K1ABC -12");
        feed(&mut tracker, 3, "K1ABC W9XYZ R-10");
        let before = tracker.items[0].decision();
        for slot in 4..80 {
            feed(&mut tracker, slot, "K1ABC W9XYZ R-10");
        }
        assert_eq!(tracker.items[0].messages.len(), 64);
        assert!(
            !tracker.items[0]
                .messages
                .iter()
                .any(|h| h.message.starts_with("CQ"))
        );
        let count = tracker.items[0].messages.len();
        tracker.update(tracker.items[0].last_ms + crate::STALE_MS, []);
        assert_eq!(tracker.items[0].decision(), before);
        assert_eq!(tracker.items[0].decision(), before);
        assert_eq!(tracker.items[0].messages.len(), count);
    }

    #[test]
    fn closing_73_is_terminal_even_with_missing_stages_or_late_evidence() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 5, "K1ABC W9XYZ 73");
        feed(&mut tracker, 3, "K1ABC W9XYZ R-10");
        let decision = tracker.items[0].decision();
        assert!(decision.received.farewell);
        assert!(!decision.received.acknowledgment);
        assert_eq!(decision.recommendation, Recommendation::Complete);
    }

    #[test]
    fn unknown_hashes_and_nonordinary_payloads_cannot_advance_a_decision() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 0, "CQ K1ABC FN42");
        let before = tracker.items[0].decision();
        feed(&mut tracker, 1, "<...> K1ABC -12");
        tracker.update(
            28_000.0,
            [(15_000.0, 27_640.0, "K1ABC W9XYZ 73", "000", -12.0)],
        );
        assert_eq!(tracker.items.len(), 1);
        assert_eq!(tracker.items[0].decision(), before);
    }

    #[test]
    fn no_evidence_has_no_recommendation() {
        let mut tracker = Conversations::default();
        feed(&mut tracker, 0, "CQ K1ABC FN42");
        tracker.items[0].steps.fill(None);
        let decision = tracker.items[0].decision();
        assert_eq!(decision.received, ReceivedStages::default());
        assert_eq!(
            decision.recommendation,
            Recommendation::InsufficientEvidence
        );
    }
}
