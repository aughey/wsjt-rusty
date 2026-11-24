//! Contact exchange suggestions; received evidence and locally played audio stay separate.
use crate::manager::EndReason;
use crate::{Conversations, ExchangeStep, MessageKind, Recommendation, classify};

/// The outgoing action implied by validated partner evidence. Local playback
/// never advances this machine; it only supplies the queue's retry counters.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QsoState {
    Calling,
    SendingReport,
    SendingRogerReport,
    SendingAcknowledgment,
    SendingFarewell,
    Closed,
}

impl QsoState {
    pub fn step(self) -> usize {
        match self {
            Self::Calling => 0,
            Self::SendingReport => 1,
            Self::SendingRogerReport => 2,
            Self::SendingAcknowledgment => 3,
            Self::SendingFarewell => 4,
            Self::Closed => 5,
        }
    }
    fn advance(self, next: Self) -> Self {
        if next.step() > self.step() {
            next
        } else {
            self
        }
    }
}

#[derive(Clone, Debug)]
pub struct Contact {
    pub target: String,
    pub opened_ms: f64,
    pub latest_ms: f64,
    pub receive_slot: i64,
    pub last_directed_slot: Option<i64>,
    state: QsoState,
    pub heard: Vec<String>,
    observations: Conversations,
    abandoned: Option<EndReason>,
}
impl Contact {
    pub fn new(target: String, start_ms: f64) -> Self {
        Self {
            target,
            opened_ms: start_ms,
            latest_ms: start_ms - 1.0,
            receive_slot: (start_ms / 15000.0).round() as i64,
            last_directed_slot: None,
            state: QsoState::Calling,
            heard: Vec::new(),
            observations: Conversations::default(),
            abandoned: None,
        }
    }
    pub fn hear(&mut self, mine: &str, start_ms: f64, text: &str, payload: &str) {
        if !start_ms.is_finite()
            || start_ms < self.opened_ms
            || ((start_ms / 15000.0).round() as i64).rem_euclid(2)
                != ((self.opened_ms / 15000.0).round() as i64).rem_euclid(2)
        {
            return;
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let kind = classify(text, payload);
        let cq = kind == MessageKind::Cq && words.iter().any(|&w| w == self.target);
        let directed = words.len() == 3 && words[0] == mine && words[1] == self.target;
        let slot = (start_ms / 15000.0).round() as i64;
        if kind != MessageKind::Other
            && start_ms > self.latest_ms
            && slot > self.receive_slot
            && self.state != QsoState::SendingFarewell
            && self.state != QsoState::Closed
        {
            if !cq && words.get(1) == Some(&self.target.as_str()) && words.first() != Some(&mine) {
                self.abandoned = Some(EndReason::PartnerAnsweredAnother);
                return;
            }
            if cq && self.state != QsoState::Calling {
                self.abandoned = Some(EndReason::PartnerResumedCq);
                return;
            }
        }
        if (!cq && !directed)
            || (directed && (start_ms / 15000.0).round() <= (self.opened_ms / 15000.0).round())
        {
            return;
        }
        if kind == MessageKind::Other {
            return;
        }
        self.observations
            .update(0.0, [(start_ms, start_ms + 12_640.0, text, payload, -12.0)]);
        // This attempt's validated partner evidence drives the shared conversation
        // decision. Self echoes and other pairs stay in the receive history outside
        // this attempt and cannot authorize a transmission.
        let next = self
            .observations
            .items
            .iter()
            .filter(|c| c.caller == self.target || c.partner.as_ref() == Some(&self.target))
            .map(|c| match c.decision().recommendation {
                Recommendation::Complete => QsoState::Closed,
                Recommendation::Next(action) => match action.step {
                    ExchangeStep::Report => QsoState::SendingReport,
                    ExchangeStep::RogerReport => QsoState::SendingRogerReport,
                    ExchangeStep::Acknowledgment => QsoState::SendingAcknowledgment,
                    ExchangeStep::Farewell => QsoState::SendingFarewell,
                    _ => QsoState::Calling,
                },
                Recommendation::InsufficientEvidence => QsoState::Calling,
            })
            .max_by_key(|state| state.step())
            .unwrap_or(QsoState::Calling);
        if directed {
            let slot = (start_ms / 15000.0).round() as i64;
            self.last_directed_slot =
                Some(self.last_directed_slot.map_or(slot, |old| old.max(slot)));
        }
        let state = self.state.advance(next);
        if state != self.state || start_ms > self.latest_ms {
            self.receive_slot = self.receive_slot.max((start_ms / 15000.0).round() as i64);
            self.latest_ms = self.latest_ms.max(start_ms);
            self.state = state;
        }
        if self.heard.iter().any(|message| message == text) {
            return;
        }
        self.heard.push(text.into());
        if self.heard.len() > 12 {
            self.heard.remove(0);
        }
    }
    pub fn abandoned(&self) -> Option<EndReason> {
        self.abandoned
    }

    pub fn state(&self) -> QsoState {
        self.state
    }
    /// A locator or local audio alone does not establish a QSO. A received
    /// partner report (including later-stage evidence) earns exclusive priority.
    pub fn is_established(&self) -> bool {
        !matches!(self.state, QsoState::Calling | QsoState::SendingReport)
    }
    pub fn step(&self) -> usize {
        self.state.step()
    }
    pub fn message(&self, mine: &str, grid: &str, report: i32) -> String {
        let tail = match self.state {
            QsoState::Calling => grid.to_owned(),
            QsoState::SendingReport => format!("{report:+03}"),
            QsoState::SendingRogerReport => format!("R{report:+03}"),
            QsoState::SendingAcknowledgment => "RR73".into(),
            _ => "73".into(),
        };
        format!("{} {mine} {tail}", self.target)
    }
    pub fn label(&self) -> &'static str {
        [
            "Call with locator",
            "Send signal report",
            "Acknowledge report",
            "Send RR73",
            "Send closing 73",
            "Closing 73 received",
        ][self.step().min(5)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn complete_transition_matrix_is_monotonic_and_closed_is_terminal() {
        use QsoState::*;
        let states = [
            Calling,
            SendingReport,
            SendingRogerReport,
            SendingAcknowledgment,
            SendingFarewell,
            Closed,
        ];
        let events = states;
        let expected = [
            [0, 1, 2, 3, 4, 5],
            [1, 1, 2, 3, 4, 5],
            [2, 2, 2, 3, 4, 5],
            [3, 3, 3, 3, 4, 5],
            [4, 4, 4, 4, 4, 5],
            [5, 5, 5, 5, 5, 5],
        ];
        for (row, state) in states.into_iter().enumerate() {
            for (column, event) in events.into_iter().enumerate() {
                assert_eq!(state.advance(event), states[expected[row][column]]);
            }
        }
    }

    #[test]
    fn recovery_progresses_despite_newer_retry_and_wrong_slot_cannot_advance() {
        let mut c = Contact::new("K1ABC".into(), 500.0);
        let p = format!("{}001", "0".repeat(74));
        c.hear("W9XYZ", 60500.0, "CQ K1ABC FN42", &p);
        c.hear("W9XYZ", 30500.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.state(), QsoState::SendingRogerReport);
        assert_eq!(c.receive_slot, 4);
        for (time, text, payload) in [
            (75500.0, "W9XYZ K1ABC RR73", p.as_str()),
            (90500.0, "N0CALL K1ABC RR73", p.as_str()),
            (90500.0, "W9XYZ N0CALL RR73", p.as_str()),
            (90500.0, "W9XYZ K1ABC RR73", "000"),
            (f64::NAN, "W9XYZ K1ABC RR73", p.as_str()),
        ] {
            c.hear("W9XYZ", time, text, payload);
        }
        assert_eq!(c.state(), QsoState::SendingRogerReport);
        c.hear("W9XYZ", 90500.0, "W9XYZ K1ABC RR73", &p);
        c.hear("W9XYZ", 120500.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.state(), QsoState::SendingFarewell);
    }

    #[test]
    fn only_partner_addressing_us_advances_and_late_retries_do_not_regress() {
        let mut c = Contact::new("K1ABC".into(), 0.0);
        let p = format!("{}001", "0".repeat(74));
        c.hear("W9XYZ", 0.0, "CQ K1ABC FN42", &p);
        assert_eq!(c.message("W9XYZ", "EN37", -12), "K1ABC W9XYZ EN37");
        c.hear("W9XYZ", 30000.0, "N0CALL K1ABC -10", &p);
        assert_eq!(c.step(), 0);
        c.hear("W9XYZ", 30000.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.message("W9XYZ", "EN37", -12), "K1ABC W9XYZ R-12");
        c.hear("W9XYZ", 15000.0, "W9XYZ K1ABC FN42", &p);
        assert_eq!(c.step(), 2);
        c.hear("W9XYZ", 60000.0, "W9XYZ K1ABC RR73", &p);
        assert_eq!(c.step(), 4);
        c.hear("W9XYZ", 90000.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.step(), 4);
        c.hear("W9XYZ", 120000.0, "W9XYZ K1ABC 73", "000");
        assert_eq!(c.step(), 4);
    }
}
