//! Contact exchange suggestions; received evidence and locally played audio stay separate.
use crate::message_kind::{MessageKind, classify};

#[derive(Clone, Debug)]
pub struct Contact {
    pub target: String,
    pub opened_ms: f64,
    pub latest_ms: f64,
    pub receive_slot: i64,
    pub step: usize,
    pub heard: Vec<String>,
}
impl Contact {
    pub fn new(target: String, start_ms: f64) -> Self {
        Self {
            target,
            opened_ms: start_ms,
            latest_ms: start_ms - 1.0,
            receive_slot: (start_ms / 15000.0).round() as i64,
            step: 0,
            heard: Vec::new(),
        }
    }
    pub fn hear(&mut self, mine: &str, start_ms: f64, text: &str, payload: &str) {
        if start_ms < self.opened_ms || start_ms <= self.latest_ms {
            return;
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let kind = classify(text, payload);
        let cq = kind == MessageKind::Cq && words.iter().any(|&w| w == self.target);
        let directed = words.len() == 3 && words[0] == mine && words[1] == self.target;
        if !cq && !directed {
            return;
        }
        let next = match kind {
            MessageKind::Cq => 0,
            MessageKind::Reply => 1,
            MessageKind::Report => 2,
            MessageKind::RogerReport => 3,
            MessageKind::Acknowledgment => 4,
            MessageKind::Signoff if words.last() == Some(&"RR73") => 4,
            MessageKind::Signoff => 5,
            _ => return,
        };
        self.latest_ms = start_ms;
        self.receive_slot = (start_ms / 15000.0).round() as i64;
        self.step = self.step.max(next);
        self.heard.push(text.into());
        if self.heard.len() > 12 {
            self.heard.remove(0);
        }
    }
    pub fn message(&self, mine: &str, grid: &str, report: i32) -> String {
        let tail = match self.step {
            0 => grid.to_owned(),
            1 => format!("{report:+03}"),
            2 => format!("R{report:+03}"),
            3 => "RR73".into(),
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
        ][self.step.min(5)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_partner_addressing_us_advances_and_late_retries_do_not_regress() {
        let mut c = Contact::new("K1ABC".into(), 0.0);
        let p = format!("{}001", "0".repeat(74));
        c.hear("W9XYZ", 0.0, "CQ K1ABC FN42", &p);
        assert_eq!(c.message("W9XYZ", "EN37", -12), "K1ABC W9XYZ EN37");
        c.hear("W9XYZ", 30000.0, "N0CALL K1ABC -10", &p);
        assert_eq!(c.step, 0);
        c.hear("W9XYZ", 30000.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.message("W9XYZ", "EN37", -12), "K1ABC W9XYZ R-12");
        c.hear("W9XYZ", 15000.0, "W9XYZ K1ABC FN42", &p);
        assert_eq!(c.step, 2);
        c.hear("W9XYZ", 60000.0, "W9XYZ K1ABC RR73", &p);
        assert_eq!(c.step, 4);
        c.hear("W9XYZ", 90000.0, "W9XYZ K1ABC -10", &p);
        assert_eq!(c.step, 4);
        c.hear("W9XYZ", 120000.0, "W9XYZ K1ABC 73", "000");
        assert_eq!(c.step, 4);
    }
}
