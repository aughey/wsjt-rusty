//! Operator-selected stations and bounded CQ-triggered exchanges. No device I/O.
use crate::{
    contact_log::{ContactRecord, Evidence},
    conversations::{Conversations, ExchangeSlot, ExchangeStep, SlotState},
    geography::{CardSort, Location, decoded_location, distance_key},
    message_kind::{MessageKind, classify},
    transmit::Contact,
};
const SLOT: f64 = 15_000.0;
const MAX_STATIONS: usize = 100;

#[derive(Clone, Debug)]
pub struct Station {
    pub call: String,
    pub received: Vec<Evidence>,
    pub location: Option<(f64, Location)>,
    pub latest_ms: f64,
    pub last_cq_ms: Option<f64>,
    pub last_cq_arrival_ms: Option<f64>,
    waiting_since_ms: Option<f64>,
    pub latest_text: String,
    pub kind: MessageKind,
    pub snr_db: f32,
    pub frequency_hz: f32,
    pub evidence: Vec<String>,
    pub note: String,
    pub completed: bool,
    /// Latest successfully completed local audio per outgoing step, not RX evidence.
    pub sent_messages: [Option<String>; 5],
    /// Successfully completed initial-call audio while this station is queued.
    pub contact_tries: usize,
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
    pub record: ContactRecord,
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
    pub log_session: String,
    /// Block new attempts at 200 pending writes. Up to 200 recently archived
    /// attempts may also receive late recovery updates. Never discard on failure.
    pub log_pending: std::collections::BTreeMap<String, ContactRecord>,
    recent_logs: Vec<ContactRecord>,
    next_id: u64,
    observations: Conversations,
}
impl StationQueue {
    /// Expire waiting stations by CQ frame end, without interrupting an exchange.
    /// Stations not yet heard get a full timeout from the first receiving tick.
    pub fn expire_waiting(&mut self, now_ms: f64, timeout_seconds: u32) {
        if !now_ms.is_finite() || timeout_seconds == 0 {
            return;
        }
        let active = self.active.as_ref().map(|a| a.contact.target.as_str());
        self.items.retain_mut(|s| {
            let since = *s.waiting_since_ms.get_or_insert(now_ms);
            active == Some(s.call.as_str())
                || now_ms - s.last_cq_ms.map(|ms| ms + 12_640.0).unwrap_or(since)
                    < f64::from(timeout_seconds) * 1000.0
        });
    }
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
    pub fn was_contacted(&self, call: &str, contacted: &[String]) -> bool {
        contacted.iter().any(|c| c == call)
            || self
                .log_pending
                .values()
                .chain(self.recent_logs.iter())
                .any(|r| r.outcome == "Completed" && r.target == call)
    }

    /// Add current CQ callers, preserving existing queue priority and its bound.
    pub fn add_cq_stations<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a crate::conversations::Conversation>,
        now_ms: f64,
        contacted: &[String],
        mine: &str,
    ) -> usize {
        let before = self.items.len();
        for c in candidates {
            if c.stage != crate::conversations::Stage::Calling
                || c.partner.is_some()
                || c.status(now_ms) == "Stale"
                || c.caller == mine
                || self.contains(&c.caller)
                || self.was_contacted(&c.caller, contacted)
            {
                continue;
            }
            self.add(c.caller.clone());
            if let Some(s) = self.items.iter_mut().find(|s| s.call == c.caller) {
                s.location = c.locations.get(&c.caller).cloned();
            }
        }
        self.items.len() - before
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
            received: Vec::new(),
            location: None,
            latest_ms: f64::NEG_INFINITY,
            last_cq_ms: None,
            last_cq_arrival_ms: None,
            waiting_since_ms: None,
            latest_text: String::new(),
            kind: MessageKind::Other,
            snr_db: -12.0,
            frequency_hz: 1500.0,
            evidence: Vec::new(),
            note: String::new(),
            completed: false,
            sent_messages: Default::default(),
            contact_tries: 0,
            attempted_cq_ms: f64::NEG_INFINITY,
        });
    }
    /// Change priority without interrupting an exchange already in progress.
    pub fn to_front(&mut self, call: &str) {
        if let Some(i) = self.items.iter().position(|s| s.call == call) {
            let station = self.items.remove(i);
            self.items.insert(0, station);
        }
        self.pin_active();
    }
    fn pin_active(&mut self) {
        if let Some(call) = self.active.as_ref().map(|a| a.contact.target.clone())
            && let Some(i) = self.items.iter().position(|s| s.call == call)
        {
            let station = self.items.remove(i);
            self.items.insert(0, station);
        }
    }
    pub fn sort(&mut self, by: CardSort, mine: &str) {
        let states: std::collections::BTreeMap<_, _> = self
            .items
            .iter()
            .map(|s| {
                let rank = if s.completed {
                    6
                } else {
                    self.exchange_slots(&s.call, self.observations.now_ms)
                        .iter()
                        .rposition(|slot| slot.state == SlotState::Received)
                        .unwrap_or(0)
                };
                (s.call.clone(), rank)
            })
            .collect();
        self.items.sort_by(|a, b| match by {
            CardSort::Signal => {
                let snr = |s: &Station| {
                    if s.evidence.is_empty() {
                        f32::NEG_INFINITY
                    } else {
                        s.snr_db
                    }
                };
                snr(b).total_cmp(&snr(a))
            }
            CardSort::State => states[&a.call].cmp(&states[&b.call]),
            CardSort::Distance => distance_key(a.location.as_ref().map(|(_, l)| l), mine)
                .total_cmp(&distance_key(b.location.as_ref().map(|(_, l)| l), mine)),
        });
        self.pin_active();
    }
    fn checkpoint(&mut self) {
        if let Some(a) = &mut self.active {
            a.record.revision += 1;
            self.log_pending
                .insert(a.record.id.clone(), a.record.clone());
        }
    }
    fn end_active(&mut self, outcome: &str) {
        if let Some(a) = &mut self.active {
            a.record.outcome = outcome.into();
        }
        self.checkpoint();
        if let Some(a) = &self.active {
            self.recent_logs.push(a.record.clone());
            if self.recent_logs.len() > 200 {
                self.recent_logs.remove(0);
            }
        }
        self.active = None;
        self.pending = None;
    }
    pub fn cancel_active(&mut self) {
        self.end_active("Interrupted");
    }
    pub fn clear(&mut self) {
        self.stop();
        self.items.clear();
        self.observations = Conversations::default();
    }
    pub fn acknowledge_log(&mut self, records: &[ContactRecord]) {
        for record in records {
            if self
                .log_pending
                .get(&record.id)
                .is_some_and(|r| r.revision == record.revision)
            {
                self.log_pending.remove(&record.id);
            }
        }
    }
    pub fn record_frequency(&mut self, plan_id: u64, sample: Option<(u64, f64)>) {
        if !self.pending.as_ref().is_some_and(|p| p.id == plan_id) {
            return;
        }
        if let Some((hz, ms)) = sample
            && let Some(a) = &mut self.active
            && a.record.dial_hz.is_none()
        {
            a.record.dial_hz = Some(hz);
            a.record.frequency_read_ms = Some(ms);
            self.checkpoint();
        }
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
        self.recent_logs.clear();
        for s in &mut self.items {
            s.latest_ms = f64::NEG_INFINITY;
            s.location = None;
            s.kind = MessageKind::Other;
            s.latest_text.clear();
            s.evidence.clear();
            s.received.clear();
            s.last_cq_ms = None;
            s.last_cq_arrival_ms = None;
            s.waiting_since_ms = None;
            s.sent_messages.fill(None);
            s.contact_tries = 0;
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
        let event = Evidence {
            start_ms,
            end_ms: start_ms + 12_640.0,
            direction: "RX".into(),
            message: text.into(),
            payload: Some(payload.into()),
            snr_db: Some(snr_db),
            frequency_hz,
        };
        // Recovery decodes can fill an already archived attempt in this session.
        for record in &mut self.recent_logs {
            if start_ms >= record.start_ms
                && start_ms <= record.end_ms
                && words.iter().any(|&w| w == record.target)
                && record.evidence.len() < 256
                && record.hear(event.clone())
            {
                record.revision += 1;
                self.log_pending.insert(record.id.clone(), record.clone());
            }
        }
        for station in &mut self.items {
            if words.iter().any(|&w| w == station.call)
                && !station
                    .received
                    .iter()
                    .any(|e| e.message == text && (e.start_ms - start_ms).abs() < 350.0)
            {
                station.received.push(event.clone());
                station
                    .received
                    .sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
                if station.received.len() > 12 {
                    station.received.remove(0);
                }
            }
        }
        if let Some(a) = &mut self.active
            && words.iter().any(|&w| w == a.contact.target)
            && start_ms >= a.record.start_ms
        {
            if a.record.evidence.len() >= 255 {
                self.end_active("Evidence limit reached");
                self.enabled = false;
                return;
            }
            a.record.hear(event);
            self.checkpoint();
        }

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
        if let Some(location) = decoded_location(text, kind)
            && s.location.as_ref().is_none_or(|(ms, _)| start_ms >= *ms)
        {
            s.location = Some((start_ms, location));
        }
        // Recovery can deliver an older CQ after a newer non-CQ message.
        if kind == MessageKind::Cq && s.last_cq_ms.is_none_or(|ms| start_ms > ms) {
            s.last_cq_ms = Some(start_ms);
            s.last_cq_arrival_ms = None;
        }
        if start_ms <= s.latest_ms {
            return;
        }
        s.latest_ms = start_ms;
        s.latest_text = text.into();
        s.kind = kind;
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
                let target = s.call.clone();
                self.end_active("Completed");
                self.remove(&target);
            }
        }
    }
    /// Called near the response boundary; never roll a stale CQ into a later period.
    pub fn plan(&mut self, now_ms: f64, mine: &str, grid: &str) -> Option<Plan> {
        if !self.enabled || self.pending.is_some() || !now_ms.is_finite() {
            return None;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.record.evidence.len() >= 256)
        {
            self.end_active("Evidence limit reached");
            self.enabled = false;
            return None;
        }
        let unanswered = self.active.as_ref().is_some_and(|a| {
            a.contact.step == 0
                && a.retries >= 4
                && a.last_tx_ms
                    .is_some_and(|ms| now_ms >= ms + 2.0 * SLOT - 1800.0)
        });
        if unanswered
            || self.active.as_ref().is_some_and(|a| {
                now_ms - a.started_ms > 180_000.0
                    || a.transmissions >= 8
                    || (a.contact.step > 0 && a.retries >= 3)
            })
        {
            let target = self.active.as_ref().map(|a| a.contact.target.clone());
            if let Some(target) = target {
                if let Some(s) = self.items.iter_mut().find(|s| s.call == target) {
                    s.note = if unanswered {
                        "No response after 4 tries · moved to back; waiting for a new CQ"
                    } else {
                        "No response / attempt limit · waiting for a new CQ"
                    }
                    .into();
                    s.attempted_cq_ms = s.latest_ms;
                }
                self.end_active(if unanswered {
                    "No response after 4 tries"
                } else {
                    "Attempt limit"
                });
                self.to_back(&target);
            }
        }
        if self.active.is_none() {
            if self.log_pending.len() >= 200 {
                return None;
            }
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
                record: ContactRecord {
                    id: format!(
                        "{}:{}:{}:{:.0}:{}",
                        self.log_session, mine, station.call, station.latest_ms, self.next_id
                    ),
                    revision: 0,
                    station_call: mine.into(),
                    station_grid: grid.into(),
                    target: station.call.clone(),
                    grid: station.location.as_ref().map(|(_, l)| l.grid.clone()),
                    start_ms: station.latest_ms,
                    end_ms: station.latest_ms + 12_640.0,
                    outcome: "In progress".into(),
                    dial_hz: None,
                    frequency_read_ms: None,
                    evidence: station.received.clone(),
                },
                started_ms: station.latest_ms,
                last_tx_ms: None,
                transmissions: 0,
                retries: 0,
                sent_step: None,
            });
            self.checkpoint();
        }
        self.pin_active();
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
            if p.step == 0 {
                s.contact_tries += 1;
            }
        }
        if let Some(a) = &mut self.active {
            a.record.hear(Evidence {
                start_ms: p.start_ms,
                end_ms: p.start_ms + 12_640.0,
                direction: "TX audio".into(),
                message: p.message.clone(),
                payload: None,
                snr_db: None,
                frequency_hz: p.frequency_hz,
            });
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
        self.checkpoint();
        if p.step == 4 {
            if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target) {
                s.completed = true;
                s.note = "Closing 73 sent · exchange ended (RF not confirmed)".into();
            }
            self.end_active("Completed");
            self.remove(&p.target);
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
    #[test]
    fn cq_bulk_add_excludes_logged_own_busy_stale_and_duplicate_stations() {
        let mut tracker = Conversations::default();
        let payload = format!("{}001", "0".repeat(74));
        let messages = [
            "CQ K1ABC FN42",
            "CQ N0XYZ EN37",
            "CQ W9XYZ EN50",
            "CQ W6XYZ CM87",
            "K7ABC W1XYZ -10",
        ];
        tracker.update(
            13_000.0,
            messages
                .iter()
                .map(|text| (0.0, 12_640.0, *text, payload.as_str(), -12.0)),
        );
        let mut q = StationQueue::default();
        q.add("N0XYZ".into());
        let contacted = vec!["K1ABC".into()];
        assert_eq!(
            q.add_cq_stations(&tracker.items, tracker.now_ms, &contacted, "W9XYZ"),
            1
        );
        assert_eq!(
            q.items.iter().map(|s| s.call.as_str()).collect::<Vec<_>>(),
            ["N0XYZ", "W6XYZ"]
        );
        assert!(q.items[1].location.is_some());
        assert_eq!(
            q.add_cq_stations(&tracker.items, tracker.now_ms, &contacted, "W9XYZ"),
            0
        );
        q.clear();
        assert_eq!(q.add_cq_stations(&tracker.items, 80_000.0, &[], "W9XYZ"), 0);
        // Unsaved and recently acknowledged completions must also block bulk/auto add.
        let mut completed = StationQueue::default();
        completed.add("K1ABC".into());
        completed.enabled = true;
        hear(&mut completed, 0.0, "CQ K1ABC FN42");
        let plan = completed.plan(14_000.0, "W9XYZ", "EN37").unwrap();
        let mut record = completed.log_pending.values().next().unwrap().clone();
        record.outcome = "Completed".into();
        completed.log_pending.clear();
        completed
            .log_pending
            .insert(record.id.clone(), record.clone());
        assert!(completed.was_contacted("K1ABC", &[]));
        completed.recent_logs.push(record.clone());
        completed.acknowledge_log(&[record]);
        assert!(completed.was_contacted("K1ABC", &[]));
        assert_eq!(plan.target, "K1ABC");
    }

    fn queue() -> StationQueue {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        q.add("N0XYZ".into());
        q.enabled = true;
        q
    }
    #[test]
    fn clearing_queue_keeps_unacknowledged_records_and_ack_does_not_drop_new_evidence() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        let first = q.log_pending.values().cloned().collect::<Vec<_>>();
        q.record_frequency(p.id + 1, Some((21_074_000, 13_900.0)));
        assert!(q.active.as_ref().unwrap().record.dial_hz.is_none());
        q.record_frequency(p.id, Some((14_074_000, 13_900.0)));
        q.record_frequency(p.id, Some((21_074_000, 14_000.0)));
        assert_eq!(q.active.as_ref().unwrap().record.dial_hz, Some(14_074_000));
        q.complete(p.id, true);
        q.acknowledge_log(&first);
        assert_eq!(
            q.log_pending.len(),
            1,
            "older save cannot acknowledge newer local playback"
        );
        q.clear();
        assert!(q.items.is_empty());
        assert!(q.active.is_none());
        assert!(q.pending.is_none());
        assert!(!q.enabled);
        let record = q.log_pending.values().next().unwrap();
        assert_eq!(record.outcome, "Interrupted");
        assert_eq!(
            record
                .evidence
                .iter()
                .filter(|e| e.direction == "TX audio")
                .count(),
            1
        );
        let saved = q.log_pending.values().cloned().collect::<Vec<_>>();
        q.acknowledge_log(&saved);
        assert!(q.log_pending.is_empty());
        hear(&mut q, 1000.0, "W9XYZ K1ABC -09");
        assert_eq!(
            q.log_pending.len(),
            1,
            "late recovery evidence updates archived attempt"
        );
        assert!(
            q.log_pending
                .values()
                .next()
                .unwrap()
                .evidence
                .iter()
                .any(|e| e.message.ends_with("-09"))
        );
    }
    #[test]
    fn received_closing_removes_station_and_cq_state_sorts_first() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        hear(&mut q, 500.0, "CQ N0XYZ EN37");
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        q.sort(CardSort::State, "EN37");
        assert_eq!(q.items[0].call, "N0XYZ");
        q.to_front("K1ABC");
        hear(&mut q, 60500.0, "CQ K1ABC FN42");
        let p = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 90500.0, "W9XYZ K1ABC 73");
        assert!(!q.contains("K1ABC"));
        assert!(q.contains("N0XYZ"));
        let record = q
            .log_pending
            .values()
            .find(|r| r.target == "K1ABC")
            .unwrap();
        assert_eq!(record.outcome, "Completed");
        assert!(record.evidence.iter().any(|e| e.message.ends_with("73")));
    }
    #[test]
    fn active_station_moves_first_and_survives_priority_changes() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ N0XYZ EN37");
        let plan = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(q.items[0].call, "N0XYZ");
        q.to_front("K1ABC");
        assert_eq!(q.items[0].call, "N0XYZ");
        for by in [CardSort::Signal, CardSort::State, CardSort::Distance] {
            q.sort(by, "FN42");
            assert_eq!(q.items[0].call, "N0XYZ");
            assert_eq!(q.pending.as_ref().unwrap().id, plan.id);
        }
        q.complete(plan.id, true);
        assert_eq!(q.active.as_ref().unwrap().contact.target, "N0XYZ");
        q.to_back("N0XYZ");
        assert!(q.active.is_none());
        q.to_front("N0XYZ");
        assert_eq!(q.items[0].call, "N0XYZ");
    }
    #[test]
    fn queue_sorts_and_retains_only_senders_latest_grid() {
        let mut q = queue();
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        hear(&mut q, 500.0, "CQ N0XYZ EN37");
        hear(&mut q, 45500.0, "K1ABC W9XYZ EN50");
        assert_eq!(q.items[0].location.as_ref().unwrap().1.grid, "FN42");
        q.sort(CardSort::Distance, "EN37");
        assert_eq!(q.items[0].call, "N0XYZ");
        q.items[1].snr_db = 10.0;
        q.sort(CardSort::Signal, "EN37");
        assert_eq!(q.items[0].call, "K1ABC");
        q.reset_observations();
        assert!(q.items.iter().all(|s| s.location.is_none()));
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
        assert!(!q.contains("K1ABC"));
        let record = q
            .log_pending
            .values()
            .find(|r| r.target == "K1ABC")
            .unwrap();
        assert_eq!(record.outcome, "Completed");
        assert!(
            record
                .evidence
                .iter()
                .any(|e| e.direction == "RX" && e.message.ends_with("RR73"))
        );
        assert!(
            record
                .evidence
                .iter()
                .any(|e| e.direction == "TX audio" && e.message.ends_with("73"))
        );
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
        for now in [14000.0, 44000.0, 74000.0, 104000.0] {
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        assert_eq!(q.items[0].contact_tries, 4);
        assert!(q.plan(118000.0, "W9XYZ", "EN37").is_none());
        assert!(q.active.is_some(), "fourth call still gets a reply window");
        assert!(q.plan(134000.0, "W9XYZ", "EN37").is_none());
        assert!(q.active.is_none());
        assert!(q.pending.is_none());
        assert_eq!(q.items.last().unwrap().call, "K1ABC");
        assert_eq!(
            q.log_pending.values().next().unwrap().outcome,
            "No response after 4 tries"
        );
        hear(&mut q, 150500.0, "CQ N0XYZ EM00");
        assert_eq!(q.plan(164000.0, "W9XYZ", "EN37").unwrap().target, "N0XYZ");
    }
    #[test]
    fn waiting_expiry_uses_cq_end_and_preserves_active_exchange() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        q.expire_waiting(14000.0, 180);
        hear(&mut q, 150500.0, "W9XYZ K1ABC -09");
        q.expire_waiting(193139.0, 180);
        assert!(q.contains("K1ABC"));
        q.expire_waiting(193140.0, 180);
        assert!(
            !q.contains("K1ABC"),
            "non-CQ traffic does not refresh timeout"
        );
        assert!(q.contains("N0XYZ"), "unheard station gets a full timeout");
        q.expire_waiting(194000.0, 180);
        assert!(q.items.is_empty());

        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.expire_waiting(999999.0, 1);
        assert!(q.contains("K1ABC"));
        assert_eq!(q.pending.as_ref().unwrap().id, p.id);
        q.reset_observations();
        q.expire_waiting(1000000.0, 180);
        assert!(
            q.contains("K1ABC"),
            "restart grants unheard stations a new timeout"
        );
    }
    #[test]
    fn response_to_fourth_call_continues_exchange() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        for now in [14000.0, 44000.0, 74000.0, 104000.0] {
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        hear(&mut q, 120500.0, "W9XYZ K1ABC -09");
        let p = q.plan(134000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ R-11");
        assert_eq!(q.items[0].contact_tries, 4);
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
        assert_eq!(q.items[0].contact_tries, 0);
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
