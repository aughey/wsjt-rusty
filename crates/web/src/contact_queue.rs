//! Operator-selected stations and bounded CQ-triggered exchanges. No device I/O.
use crate::{
    conversations::{Conversations, ExchangeSlot, ExchangeStep, SlotState},
    message_kind::{MessageKind, classify},
    transmit::Contact,
};
const SLOT: f64 = 15_000.0;
const MAX_STATIONS: usize = 100;

#[derive(Clone, Debug)]
pub struct Station {
    pub call: String,
    pub latest_ms: f64,
    pub last_cq_ms: Option<f64>,
    pub last_cq_arrival_ms: Option<f64>,
    pub latest_text: String,
    pub kind: MessageKind,
    pub snr_db: f32,
    pub frequency_hz: f32,
    pub evidence: Vec<String>,
    pub note: String,
    pub completed: bool,
    /// Latest successfully completed local audio per outgoing step, not RX evidence.
    pub sent_messages: [Option<String>; 5],
    attempted_cq_ms: f64,
}
impl Station {
    pub fn state(&self, now_ms: f64) -> &str {
        if self.completed {
            &self.note
        } else if wsjt_rusty::ft8::transmit::pack_call(&self.call).is_err() {
            "Transmit requires a standard base callsign"
        } else if !self.frequency_hz.is_finite() || !(100.0..=4000.0).contains(&self.frequency_hz) {
            "Station frequency is outside the transmit audio passband"
        } else if self.kind == MessageKind::Cq
            && now_ms <= ((self.latest_ms / SLOT).round() + 1.0) * SLOT + 300.0
        {
            "Fresh CQ heard · response opportunity"
        } else {
            "Waiting for a fresh CQ · station may be in another contact"
        }
    }
}
#[derive(Clone, Debug)]
pub struct Exchange {
    pub contact: Contact,
    pub started_ms: f64,
    pub last_tx_ms: Option<f64>,
    pub transmissions: usize,
    pub retries: usize,
    pub sent_step: Option<usize>,
}
#[derive(Clone, Debug)]
pub struct Plan {
    pub id: u64,
    pub target: String,
    pub message: String,
    pub start_ms: f64,
    pub receive_slot: i64,
    pub frequency_hz: f32,
    pub step: usize,
}
#[derive(Clone, Debug, Default)]
pub struct StationQueue {
    pub items: Vec<Station>,
    pub enabled: bool,
    pub active: Option<Exchange>,
    pub pending: Option<Plan>,
    next_id: u64,
    observations: Conversations,
}
impl StationQueue {
    /// Retain received stage summaries while queued, independently of board expiry.
    pub fn exchange_slots(&self, call: &str, now_ms: f64) -> Vec<ExchangeSlot> {
        self.observations
            .items
            .iter()
            .filter(|c| c.caller == call || c.partner.as_deref() == Some(call))
            .max_by(|a, b| a.last_ms.total_cmp(&b.last_ms))
            .map(|c| c.exchange_slots(now_ms))
            .unwrap_or_else(|| {
                ExchangeStep::ALL
                    .into_iter()
                    .map(|step| ExchangeSlot {
                        step,
                        state: SlotState::NotHeard,
                        current: false,
                        direction: String::new(),
                        evidence: None,
                    })
                    .collect()
            })
    }
    pub fn contains(&self, call: &str) -> bool {
        self.items.iter().any(|s| s.call == call)
    }
    pub fn priorities(&self) -> Vec<f32> {
        let mut frequencies = Vec::new();
        for s in &self.items {
            if !s.completed
                && !s.evidence.is_empty()
                && s.frequency_hz.is_finite()
                && !frequencies
                    .iter()
                    .any(|&f: &f32| (f - s.frequency_hz).abs() < 4.0)
            {
                frequencies.push(s.frequency_hz);
            }
        }
        frequencies
    }
    pub fn record_arrival(&mut self, start_ms: f64, text: &str, received_ms: f64) {
        for s in &mut self.items {
            if s.last_cq_ms == Some(start_ms)
                && s.latest_text == text
                && s.last_cq_arrival_ms.is_none()
                && received_ms.is_finite()
                && received_ms > 0.0
            {
                s.last_cq_arrival_ms = Some(received_ms);
            }
        }
    }
    pub fn add(&mut self, call: String) {
        if self.contains(&call) || self.items.len() >= MAX_STATIONS {
            return;
        }
        self.items.push(Station {
            call,
            latest_ms: f64::NEG_INFINITY,
            last_cq_ms: None,
            last_cq_arrival_ms: None,
            latest_text: String::new(),
            kind: MessageKind::Other,
            snr_db: -12.0,
            frequency_hz: 1500.0,
            evidence: Vec::new(),
            note: String::new(),
            completed: false,
            sent_messages: Default::default(),
            attempted_cq_ms: f64::NEG_INFINITY,
        });
    }
    pub fn cancel_active(&mut self) {
        self.active = None;
        self.pending = None;
    }
    pub fn stop(&mut self) {
        self.enabled = false;
        self.cancel_active();
    }
    pub fn to_back(&mut self, call: &str) {
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.contact.target == call)
        {
            self.cancel_active();
        }
        if let Some(i) = self.items.iter().position(|s| s.call == call) {
            let s = self.items.remove(i);
            self.items.push(s);
        }
    }
    pub fn again(&mut self, call: &str) {
        if let Some(s) = self.items.iter_mut().find(|s| s.call == call) {
            s.completed = false;
            s.note.clear();
            s.attempted_cq_ms = s.latest_ms;
        }
    }
    pub fn remove(&mut self, call: &str) {
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.contact.target == call)
        {
            self.cancel_active();
        }
        self.items.retain(|s| s.call != call);
        if self.items.is_empty() {
            self.stop();
        }
    }
    pub fn reset_observations(&mut self) {
        self.stop();
        self.observations = Conversations::default();
        for s in &mut self.items {
            s.latest_ms = f64::NEG_INFINITY;
            s.kind = MessageKind::Other;
            s.latest_text.clear();
            s.evidence.clear();
            s.last_cq_ms = None;
            s.last_cq_arrival_ms = None;
            s.sent_messages.fill(None);
        }
    }
    pub fn hear(
        &mut self,
        mine: &str,
        start_ms: f64,
        text: &str,
        payload: &str,
        snr_db: f32,
        frequency_hz: f32,
    ) {
        if !start_ms.is_finite() {
            return;
        }
        let kind = classify(text, payload);
        if kind == MessageKind::Other {
            return;
        }
        let words: Vec<_> = text.split_whitespace().collect();
        if self
            .items
            .iter()
            .any(|s| words.iter().any(|&w| w == s.call))
        {
            // Both directions and delayed decodes fill stages, even when they cannot
            // advance the active transmit exchange. Retention is bounded by the tracker.
            self.observations.update(
                0.0,
                [(start_ms, start_ms + 12_640.0, text, payload, snr_db)],
            );
        }
        let index = self.items.iter().position(|s| {
            if kind == MessageKind::Cq {
                words.iter().any(|&w| w == s.call)
            } else {
                words.get(1).is_some_and(|&w| w == s.call)
            }
        });
        let Some(index) = index else {
            return;
        };
        let s = &mut self.items[index];
        if start_ms <= s.latest_ms {
            return;
        }
        s.latest_ms = start_ms;
        s.latest_text = text.into();
        s.kind = kind;
        if kind == MessageKind::Cq {
            s.last_cq_ms = Some(start_ms);
            s.last_cq_arrival_ms = None;
        }
        s.snr_db = snr_db;
        s.frequency_hz = frequency_hz;
        s.evidence.push(text.into());
        if s.evidence.len() > 12 {
            s.evidence.remove(0);
        }
        if let Some(active) = &mut self.active
            && active.contact.target == s.call
            && start_ms > active.started_ms
        {
            if kind != MessageKind::Cq && words.first() != Some(&mine) {
                s.note = "Station answered someone else · waiting for a new CQ".into();
                self.cancel_active();
                return;
            }
            if kind == MessageKind::Cq && active.contact.step > 0 {
                self.cancel_active();
                return;
            }
            let old_step = active.contact.step;
            active.contact.hear(mine, start_ms, text, payload);
            if active.contact.step > old_step {
                active.retries = 0;
            }
            if active.contact.step == 5 {
                s.completed = true;
                s.note = "Closing 73 received · exchange ended".into();
                self.cancel_active();
            }
        }
    }
    /// Called near the response boundary; never roll a stale CQ into a later period.
    pub fn plan(&mut self, now_ms: f64, mine: &str, grid: &str) -> Option<Plan> {
        if !self.enabled || self.pending.is_some() || !now_ms.is_finite() {
            return None;
        }
        if self.active.as_ref().is_some_and(|a| {
            now_ms - a.started_ms > 180_000.0 || a.transmissions >= 8 || a.retries >= 3
        }) {
            let target = self.active.as_ref().map(|a| a.contact.target.clone());
            if let Some(target) = target {
                if let Some(s) = self.items.iter_mut().find(|s| s.call == target) {
                    s.note = "No response / attempt limit · waiting for a new CQ".into();
                    s.attempted_cq_ms = s.latest_ms;
                }
                self.to_back(&target);
            }
        }
        if self.active.is_none() {
            let station = self.items.iter_mut().find(|s| {
                let due = ((s.latest_ms / SLOT).round() + 1.0) * SLOT + 500.0;
                !s.completed
                    && s.kind == MessageKind::Cq
                    && s.latest_ms > s.attempted_cq_ms
                    && wsjt_rusty::ft8::transmit::pack_call(&s.call).is_ok()
                    && s.frequency_hz.is_finite()
                    && (100.0..=4000.0).contains(&s.frequency_hz)
                    && now_ms >= due - 1800.0
                    && now_ms <= due - 200.0
            })?;
            station.attempted_cq_ms = station.latest_ms;
            self.active = Some(Exchange {
                contact: Contact::new(station.call.clone(), station.latest_ms),
                started_ms: station.latest_ms,
                last_tx_ms: None,
                transmissions: 0,
                retries: 0,
                sent_step: None,
            });
        }
        let a = self.active.as_ref()?;
        let mut due = (a.contact.receive_slot + 1) as f64 * SLOT + 500.0;
        if let Some(last) = a.last_tx_ms {
            due = due.max(last + 2.0 * SLOT);
            while due < now_ms + 200.0 {
                due += 2.0 * SLOT;
            }
        }
        if now_ms > due - 200.0 {
            self.cancel_active();
            return None;
        }
        if now_ms < due - 1800.0 {
            return None;
        }
        let s = self.items.iter().find(|s| s.call == a.contact.target)?;
        let report = s.snr_db.round().clamp(-50.0, 49.0) as i32;
        let message = a.contact.message(mine, grid, report);
        if wsjt_rusty::ft8::transmit::pack(&message).is_err() {
            return None;
        }
        self.next_id += 1;
        let p = Plan {
            id: self.next_id,
            target: s.call.clone(),
            message,
            start_ms: due,
            receive_slot: a.contact.receive_slot,
            frequency_hz: s.frequency_hz,
            step: a.contact.step,
        };
        self.pending = Some(p.clone());
        Some(p)
    }
    pub fn complete(&mut self, id: u64, success: bool) {
        let Some(p) = self.pending.as_ref().filter(|p| p.id == id).cloned() else {
            return;
        };
        self.pending = None;
        if !success {
            self.stop();
            return;
        }
        if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target)
            && let Some(message) = s.sent_messages.get_mut(p.step)
        {
            *message = Some(p.message.clone());
        }
        if let Some(a) = &mut self.active {
            a.transmissions += 1;
            if a.sent_step == Some(p.step) {
                a.retries += 1;
            } else {
                a.retries = 1;
            }
            a.sent_step = Some(p.step);
            a.last_tx_ms = Some(p.start_ms);
            if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target) {
                s.note = format!("Sent {} · awaiting response", p.message);
            }
        }
        if p.step == 4 {
            if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target) {
                s.completed = true;
                s.note = "Closing 73 sent · exchange ended (RF not confirmed)".into();
            }
            self.cancel_active();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hear(q: &mut StationQueue, t: f64, text: &str) {
        q.hear(
            "W9XYZ",
            t,
            text,
            &format!("{}001", "0".repeat(74)),
            -11.0,
            1200.0,
        );
    }
    fn queue() -> StationQueue {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        q.add("N0XYZ".into());
        q.enabled = true;
        q
    }
    #[test]
    fn queued_stages_retain_both_directions_and_delayed_evidence_without_local_tx() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        assert_eq!(
            q.items[0].sent_messages[0].as_deref(),
            Some("K1ABC W9XYZ EN37")
        );
        let slots = q.exchange_slots("K1ABC", 16000.0);
        assert_eq!(slots.len(), 6);
        assert_eq!(slots[0].state, SlotState::Received);
        assert_eq!(
            slots[1].state,
            SlotState::Expected,
            "local playback is not received evidence"
        );
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        hear(&mut q, 15500.0, "K1ABC W9XYZ EN37");
        let slots = q.exchange_slots("K1ABC", 45000.0);
        assert!(slots[..3].iter().all(|s| s.state == SlotState::Received));
        assert_eq!(
            slots[1].evidence.as_ref().unwrap().latest.message,
            "K1ABC W9XYZ EN37"
        );
        assert_eq!(slots[3].state, SlotState::Expected);
        q.to_back("K1ABC");
        let slots = q.exchange_slots("K1ABC", 900000.0);
        assert_eq!(
            slots[2].state,
            SlotState::Received,
            "queue retains history after board expiry"
        );
        assert_eq!(slots[3].state, SlotState::NotHeard);
        q.reset_observations();
        assert!(q.items[0].sent_messages.iter().all(Option::is_none));
        assert!(
            q.exchange_slots("K1ABC", 900000.0)
                .iter()
                .all(|s| s.evidence.is_none())
        );
    }
    #[test]
    fn cq_priority_freshness_and_busy_stations() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        hear(&mut q, 500.0, "CQ N0XYZ EM00");
        hear(&mut q, 600.0, "K2OTHER K1ABC -10");
        assert_eq!(q.plan(14000.0, "W9XYZ", "EN37").unwrap().target, "N0XYZ");
        q.to_back("N0XYZ");
        assert!(q.pending.is_none());
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        hear(&mut q, 500.0, "CQ N0XYZ EM00");
        q.to_back("K1ABC");
        assert_eq!(q.plan(14000.0, "W9XYZ", "EN37").unwrap().target, "N0XYZ");
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        assert!(q.plan(30000.0, "W9XYZ", "EN37").is_none());
    }
    #[test]
    fn exchange_progress_retry_bound_and_cancel_on_other_partner() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ EN37");
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        let p = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ R-11");
        assert_eq!(p.frequency_hz, 1200.0);
        q.complete(p.id, true);
        hear(&mut q, 60500.0, "W9XYZ K1ABC RR73");
        let p = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ 73");
        q.complete(p.id, true);
        assert!(q.items[0].completed);
        assert!(q.active.is_none());
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "K2OTHER K1ABC -09");
        assert!(q.active.is_none());
        assert!(q.plan(44000.0, "W9XYZ", "EN37").is_none());
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        for now in [14000.0, 44000.0, 74000.0] {
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        assert!(q.plan(104000.0, "W9XYZ", "EN37").is_none());
        assert!(q.active.is_none());
    }
    #[test]
    fn bounds_removal_noise_and_delayed_evidence() {
        let mut q = queue();
        hear(&mut q, 30500.0, "K2OTHER K1ABC -10");
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        assert_eq!(q.items[0].kind, MessageKind::Report);
        q.hear("W9XYZ", 60500.0, "CQ K1ABC FN42", "000", 0.0, 0.0);
        assert!(q.plan(74000.0, "W9XYZ", "EN37").is_none());
        hear(&mut q, 90500.0, "CQ K1ABC FN42");
        let p = q.plan(104000.0, "W9XYZ", "EN37").unwrap();
        q.remove("K1ABC");
        q.complete(p.id, true);
        assert!(q.pending.is_none());
        assert!(!q.contains("K1ABC"));
        for i in 0..200 {
            q.add(format!("K{i}ABC"));
        }
        assert_eq!(q.items.len(), MAX_STATIONS);
    }
    #[test]
    fn canceled_or_failed_playback_does_not_leave_sent_history() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, false);
        assert!(q.items[0].sent_messages.iter().all(Option::is_none));
        q.enabled = true;
        hear(&mut q, 30500.0, "CQ K1ABC FN42");
        let p = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        q.to_back("K1ABC");
        q.complete(p.id, true);
        assert!(
            q.items
                .iter()
                .all(|s| s.sent_messages.iter().all(Option::is_none))
        );
    }
}

#[cfg(test)]
mod observation_tests {
    use super::*;
    #[test]
    fn cq_clock_and_arrival_survive_retries_and_later_reports() {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        let payload = format!("{}001", "0".repeat(74));
        q.hear("W9XYZ", 500.0, "CQ K1ABC FN42", &payload, -7.4, 1234.375);
        q.record_arrival(500.0, "CQ K1ABC FN42", 14000.0);
        q.record_arrival(500.0, "CQ K1ABC FN42", 19000.0);
        q.enabled = true;
        let plan = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(plan.frequency_hz, 1234.375);
        q.complete(plan.id, true);
        q.hear(
            "W9XYZ",
            30500.0,
            "W9XYZ K1ABC -12",
            &payload,
            -7.4,
            1234.625,
        );
        assert_eq!(q.items[0].last_cq_ms, Some(500.0));
        assert_eq!(q.items[0].last_cq_arrival_ms, Some(14000.0));
        assert_eq!(q.priorities(), [1234.625]);
        let plan = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(plan.frequency_hz, 1234.625);
        assert_eq!(
            plan.message, "K1ABC W9XYZ R-07",
            "report describes received SNR, not the partner's report of us"
        );
    }
}
