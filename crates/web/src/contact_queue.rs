//! Operator-selected stations and bounded CQ-triggered exchanges. No device I/O.
use crate::{
    contact_log::{CompletedContact, ContactRecord, Evidence},
    conversations::{Conversations, ExchangeSlot, ExchangeStep, SlotState},
    geography::{CardSort, Location, decoded_location, distance_key},
    message_kind::{MessageKind, classify},
    transmit::{Contact, QsoState},
};
use ft8_conversations::manager::{self, Attempt, Candidate, EndReason, PolicyDecision};
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
    pub sort_penalty_until_ms: f64,
}
impl Station {
    pub fn state(&self, now_ms: f64) -> &str {
        if self.completed || self.note.starts_with("Attempt ended:") {
            &self.note
        } else if wsjt_rusty::ft8::transmit::pack_call(&self.call).is_err() {
            "Transmit requires a standard base callsign"
        } else if !self.frequency_hz.is_finite() || !(100.0..=4000.0).contains(&self.frequency_hz) {
            "Station frequency is outside the transmit audio passband"
        } else if self.kind == MessageKind::Cq
            && now_ms <= ((self.latest_ms / SLOT).round() + 1.0) * SLOT + 300.0
        {
            "Fresh CQ heard · response opportunity"
        } else if !self.note.is_empty() {
            &self.note
        } else {
            "Waiting for a fresh CQ"
        }
    }
}
#[derive(Clone, Debug)]
pub struct Exchange {
    /// Chosen once per attempt; RX frequency updates never move this tone.
    pub transmit_frequency_hz: Option<f32>,
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
/// Operational state is derived from the owned exchange/reservation, never
/// inferred from presentation categories or the most recently displayed text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContactingState {
    Paused,
    WaitingForCq,
    Attempting(crate::transmit::QsoState),
    InQso(crate::transmit::QsoState),
    Reserved(crate::transmit::QsoState),
}
#[derive(Clone, Debug, Default)]
pub struct StationQueue {
    pub items: Vec<Station>,
    pub enabled: bool,
    /// CQ state, strongest signal, then nearest distance; unchecked keys are skipped.
    pub sort_preferences: [bool; 3],
    pub find_open_frequency: bool,
    /// Latest placement from recent spectrum history; absent when unavailable.
    pub open_frequency_hz: Option<f32>,
    last_sort_slot: Option<i64>,
    pub active: Option<Exchange>,
    pub pending: Option<Plan>,
    pub log_session: String,
    /// Block new attempts at 200 pending writes. Up to 200 recently archived
    /// attempts may also receive late recovery updates. Never discard on failure.
    pub log_pending: std::collections::BTreeMap<String, ContactRecord>,
    recent_logs: Vec<ContactRecord>,
    next_id: u64,
    observations: Conversations,
    require_slot_progress: bool,
    completed_receive_slot: Option<i64>,
    pub policy: Option<PolicyDecision>,
}
impl StationQueue {
    /// Browser retries require the current receive slot to finish, or a newly
    /// validated partner message requesting a response or retry. Capture progress alone
    /// and recovery activity cannot authorize a blind retry.
    pub fn require_receive_progress(&mut self) {
        self.require_slot_progress = true;
        self.completed_receive_slot = None;
    }
    pub fn receive_slot_completed(&mut self, slot_utc_ms: f64) {
        if slot_utc_ms.is_finite() {
            let slot = (slot_utc_ms / SLOT).round() as i64;
            self.completed_receive_slot = Some(
                self.completed_receive_slot
                    .map_or(slot, |old| old.max(slot)),
            );
        }
    }
    fn attempt<'a>(&self, a: &'a Exchange) -> Attempt<'a> {
        Attempt {
            contact: &a.contact,
            started_ms: a.started_ms,
            last_tx_ms: a.last_tx_ms,
            transmissions: a.transmissions,
            retries: a.retries,
            sent_step: a.sent_step,
            require_receive_progress: self.require_slot_progress,
            completed_receive_slot: self.completed_receive_slot,
        }
    }

    /// The browser supplies evidence/device facts; the headless crate owns decisions.
    pub fn policy_decision(&self, now_ms: f64, mine: &str, grid: &str) -> PolicyDecision {
        let candidates: Vec<_> = self
            .items
            .iter()
            .filter_map(|station| {
                // Waveform compatibility and audio passband are executor constraints.
                if station.evidence.is_empty()
                    || !station.latest_ms.is_finite()
                    || wsjt_rusty::ft8::transmit::pack_call(&station.call).is_err()
                    || !station.frequency_hz.is_finite()
                    || !(100.0..=4000.0).contains(&station.frequency_hz)
                {
                    return None;
                }
                let conversation = self
                    .observations
                    .items
                    .iter()
                    .filter(|c| {
                        c.caller == station.call || c.partner.as_ref() == Some(&station.call)
                    })
                    .max_by(|a, b| a.last_ms.total_cmp(&b.last_ms))?;
                Some(Candidate {
                    conversation,
                    attempted_cq_ms: station.attempted_cq_ms,
                    completed: station.completed,
                })
            })
            .collect();
        let report = self
            .active
            .as_ref()
            .and_then(|a| self.items.iter().find(|s| s.call == a.contact.target))
            .map(|s| s.snr_db.round().clamp(-50.0, 49.0) as i32)
            .unwrap_or(-12);
        manager::decide(
            self.active.as_ref().map(|a| self.attempt(a)),
            &candidates,
            now_ms,
            mine,
            grid,
            report,
        )
    }

    pub fn policy_label(&self) -> String {
        if !self.enabled {
            return "Policy paused · Start contacting to enable decisions".into();
        }
        match &self.policy {
            Some(PolicyDecision::Listen(reason)) => reason.label().into(),
            Some(PolicyDecision::Start { target, .. }) => {
                format!("Selected {target} · fresh CQ in priority order")
            }
            Some(PolicyDecision::Transmit { message, .. }) => format!("Transmit next · {message}"),
            Some(PolicyDecision::End(reason)) => format!("Attempt ended · {}", reason.label()),
            None => "Listening · waiting for the next policy decision".into(),
        }
    }
    /// Authorization is checked again by the transport at the key/audio boundary.
    pub fn can_transmit(&self, id: u64) -> bool {
        let Some(p) = self.pending.as_ref().filter(|p| p.id == id) else {
            return false;
        };
        let Some(a) = self
            .active
            .as_ref()
            .filter(|a| a.contact.target == p.target)
        else {
            return false;
        };
        let Some(s) = self.items.iter().find(|s| s.call == p.target) else {
            return false;
        };
        self.enabled
            && matches!(self.policy_decision(p.start_ms - manager::RESERVATION_DEADLINE_MS,
                    &a.record.station_call, &a.record.station_grid),
                PolicyDecision::Transmit { start_ms, step, ref message, .. }
                    if start_ms == p.start_ms && step == p.step && message == &p.message)
            && p.step == a.contact.step()
            && p.message
                == a.contact.message(
                    &a.record.station_call,
                    &a.record.station_grid,
                    s.snr_db.round().clamp(-50.0, 49.0) as i32,
                )
            && p.frequency_hz == a.transmit_frequency_hz.unwrap_or(s.frequency_hz)
    }
    pub fn revoke_pending(&mut self, id: u64) {
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            self.pending = None;
        }
    }
    pub fn state(&self) -> ContactingState {
        if !self.enabled {
            ContactingState::Paused
        } else if let Some(a) = &self.active {
            if self.pending.is_some() {
                ContactingState::Reserved(a.contact.state())
            } else if !a.contact.is_established() {
                ContactingState::Attempting(a.contact.state())
            } else {
                ContactingState::InQso(a.contact.state())
            }
        } else {
            ContactingState::WaitingForCq
        }
    }
    pub fn state_label(&self) -> String {
        match self.state() {
            ContactingState::Paused => "Contacting paused".into(),
            ContactingState::WaitingForCq => "Waiting for a fresh CQ".into(),
            ContactingState::Attempting(_)
            | ContactingState::InQso(_)
            | ContactingState::Reserved(_) => {
                let a = self
                    .active
                    .as_ref()
                    .expect("active QSO state owns an exchange");
                format!(
                    "{} {} · {}{}",
                    if a.contact.is_established() {
                        "In QSO with"
                    } else {
                        "Attempting contact with"
                    },
                    a.contact.target,
                    a.contact.label(),
                    if self.pending.is_some() {
                        " · audio reserved"
                    } else if a.contact.state() == QsoState::Calling && a.transmissions >= 4 {
                        " · waiting for final reply (4/4 calls)"
                    } else {
                        " · listening"
                    }
                )
            }
        }
    }
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
    fn conversation(&self, call: &str) -> Option<&crate::conversations::Conversation> {
        let attempt = self
            .active
            .as_ref()
            .map(|a| &a.record)
            .filter(|r| r.target == call)
            .or_else(|| self.recent_logs.iter().rev().find(|r| r.target == call));
        self.observations
            .items
            .iter()
            .filter(|c| c.caller == call || c.partner.as_deref() == Some(call))
            // Other contacts involving the partner must not replace our evidence.
            .max_by(|a, b| {
                let ours = |c: &crate::conversations::Conversation| {
                    attempt.is_some_and(|r| {
                        c.caller == r.station_call
                            || c.partner.as_deref() == Some(r.station_call.as_str())
                    })
                };
                ours(a).cmp(&ours(b)).then(a.last_ms.total_cmp(&b.last_ms))
            })
    }

    pub fn received_history(&self, call: &str) -> Vec<ft8_conversations::Heard> {
        self.conversation(call)
            .map(|c| c.received_history())
            .unwrap_or_default()
    }

    pub fn expected_action(&self, call: &str) -> String {
        self.conversation(call)
            .map(|c| match c.decision().recommendation {
                ft8_conversations::Recommendation::Next(action) => format!(
                    "Expected: {} → {} · {}{}",
                    action.sender.as_deref().unwrap_or("Responding station"),
                    action.recipient,
                    action.step.label(),
                    if action.optional { " (optional)" } else { "" }
                ),
                ft8_conversations::Recommendation::Complete => {
                    "Closing 73 heard · exchange ended as received".into()
                }
                ft8_conversations::Recommendation::InsufficientEvidence => {
                    "No ordinary exchange evidence".into()
                }
            })
            .unwrap_or_else(|| "Waiting for received conversation evidence".into())
    }

    /// Retain received stage summaries while queued, independently of board expiry.
    pub fn exchange_slots(&self, call: &str, now_ms: f64) -> Vec<ExchangeSlot> {
        self.conversation(call)
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
    pub fn remember_conversation(&mut self, conversation: &ft8_conversations::Conversation) {
        self.observations.remember(conversation);
    }

    pub fn contains(&self, call: &str) -> bool {
        self.items.iter().any(|s| s.call == call)
    }
    pub fn marker_status(&self, text: &str, payload: &str) -> (bool, bool) {
        let kind = classify(text, payload);
        if kind == MessageKind::Other {
            return (false, false);
        }
        let words: Vec<_> = text.split_whitespace().collect();
        let sender = if kind == MessageKind::Cq {
            words
                .iter()
                .rev()
                .find(|w| {
                    w.chars().any(|c| c.is_ascii_digit())
                        && w.chars().any(|c| c.is_ascii_alphabetic())
                        && Location::parse(w).is_none()
                })
                .copied()
        } else {
            words.get(1).copied()
        };
        let sender = sender.unwrap_or("").trim_matches(['<', '>']);
        (
            self.contains(sender),
            self.active
                .as_ref()
                .is_some_and(|a| a.contact.target == sender),
        )
    }
    pub fn was_contacted(&self, call: &str, contacted: &[CompletedContact], since_ms: f64) -> bool {
        contacted.iter().any(|r| r.target == call && r.end_ms > since_ms)
            // Pending log writes close the commit gap, including storage failures.
            || self.log_pending.values().any(|r| {
                r.outcome == "Completed" && r.target == call && r.end_ms > since_ms
            })
    }

    /// Add current CQ callers, preserving existing queue priority and its bound.
    pub fn add_cq_stations<'a>(
        &mut self,
        candidates: impl IntoIterator<Item = &'a crate::conversations::Conversation>,
        now_ms: f64,
        contacted: &[CompletedContact],
        since_ms: f64,
        mine: &str,
    ) -> usize {
        let before = self.items.len();
        for c in candidates {
            if c.stage != crate::conversations::Stage::Calling
                || c.partner.is_some()
                || c.status(now_ms) == "Stale"
                || c.caller == mine
                || self.contains(&c.caller)
                || self.was_contacted(&c.caller, contacted, since_ms)
            {
                continue;
            }
            self.add(c.caller.clone());
            self.remember_conversation(c);
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
        if !received_ms.is_finite() || received_ms <= 0.0 {
            return;
        }
        for s in &mut self.items {
            for event in &mut s.received {
                if event.message == text
                    && (event.start_ms - start_ms).abs() < 350.0
                    && event.received_at_ms.is_none()
                {
                    event.received_at_ms = Some(received_ms);
                }
            }
            if s.last_cq_ms == Some(start_ms)
                && s.latest_text == text
                && s.last_cq_arrival_ms.is_none()
                && received_ms.is_finite()
                && received_ms > 0.0
            {
                s.last_cq_arrival_ms = Some(received_ms);
            }
        }
        if self
            .active
            .as_mut()
            .is_some_and(|a| a.record.record_arrival(start_ms, text, received_ms))
        {
            self.checkpoint();
        }
        for record in &mut self.recent_logs {
            if record.record_arrival(start_ms, text, received_ms) {
                record.revision += 1;
                self.log_pending.insert(record.id.clone(), record.clone());
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
            sort_penalty_until_ms: f64::NEG_INFINITY,
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
        if let Some(call) = self
            .active
            .as_ref()
            .filter(|a| a.contact.is_established())
            .map(|a| a.contact.target.clone())
            && let Some(i) = self.items.iter().position(|s| s.call == call)
        {
            let station = self.items.remove(i);
            self.items.insert(0, station);
        }
    }
    pub fn sort(&mut self, by: CardSort, mine: &str) {
        let keys = match by {
            CardSort::State => [true, false, false],
            CardSort::Signal => [false, true, false],
            CardSort::Distance => [false, false, true],
        };
        self.sort_selected(keys, mine, self.observations.now_ms);
    }
    pub fn sort_cycle(&mut self, now_ms: f64, mine: &str) {
        if !now_ms.is_finite() {
            return;
        }
        let slot = (now_ms / SLOT).floor() as i64;
        if self.last_sort_slot != Some(slot) {
            self.last_sort_slot = Some(slot);
            self.sort_selected(self.sort_preferences, mine, now_ms);
        }
    }
    pub fn sort_selected(&mut self, keys: [bool; 3], mine: &str, now_ms: f64) {
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
        self.items.sort_by(|a, b| {
            let snr = |s: &Station| {
                if s.evidence.is_empty() {
                    f32::NEG_INFINITY
                } else {
                    s.snr_db
                }
            };
            (a.sort_penalty_until_ms > now_ms)
                .cmp(&(b.sort_penalty_until_ms > now_ms))
                .then_with(|| {
                    if keys[0] {
                        states[&a.call].cmp(&states[&b.call])
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| {
                    if keys[1] {
                        snr(b).total_cmp(&snr(a))
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .then_with(|| {
                    if keys[2] {
                        distance_key(a.location.as_ref().map(|(_, l)| l), mine)
                            .total_cmp(&distance_key(b.location.as_ref().map(|(_, l)| l), mine))
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
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
            if outcome != "Completed"
                && let Some(station) = self.items.iter_mut().find(|s| s.call == a.contact.target)
            {
                station.note = format!("Attempt ended: {outcome} · waiting for a fresh CQ");
            }
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
    pub fn completed_record_ids(&self) -> std::collections::BTreeSet<String> {
        self.recent_logs
            .iter()
            .filter(|r| r.outcome == "Completed")
            .map(|r| r.id.clone())
            .collect()
    }

    /// Update an archived contact by stable ID, even after its initial save was acknowledged.
    pub fn record_completed_frequency(&mut self, id: &str, sample: (u64, f64)) {
        if let Some(record) = self
            .recent_logs
            .iter_mut()
            .find(|r| r.id == id && r.outcome == "Completed")
        {
            record.dial_hz = Some(sample.0);
            record.frequency_read_ms = Some(sample.1);
            record.revision += 1;
            self.log_pending.insert(record.id.clone(), record.clone());
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
        self.stop_with_reason("Interrupted");
    }
    pub fn stop_with_reason(&mut self, reason: &str) {
        self.enabled = false;
        self.policy = None;
        self.end_active(reason);
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
        self.last_sort_slot = None;
        self.completed_receive_slot = None;
        for s in &mut self.items {
            s.latest_ms = f64::NEG_INFINITY;
            s.location = None;
            s.kind = MessageKind::Other;
            s.latest_text.clear();
            s.note.clear();
            s.evidence.clear();
            s.received.clear();
            s.last_cq_ms = None;
            s.last_cq_arrival_ms = None;
            s.waiting_since_ms = None;
            s.sent_messages.fill(None);
            s.contact_tries = 0;
            s.sort_penalty_until_ms = f64::NEG_INFINITY;
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
            received_at_ms: None,
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
        if kind != MessageKind::Cq && words.get(1) == Some(&mine) {
            return;
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
        let newest = start_ms > s.latest_ms;
        if newest {
            s.latest_ms = start_ms;
            s.latest_text = text.into();
            s.kind = kind;
            s.snr_db = snr_db;
            s.frequency_hz = frequency_hz;
            s.evidence.push(text.into());
            if s.evidence.len() > 12 {
                s.evidence.remove(0);
            }
        }
        if let Some(active) = &mut self.active
            && active.contact.target == s.call
            && start_ms > active.started_ms
        {
            let old_state = active.contact.state();
            active.contact.hear(mine, start_ms, text, payload);
            if let Some(reason) = active.contact.abandoned() {
                self.policy = Some(PolicyDecision::End(reason));
                self.end_active(reason.label());
                return;
            }
            if active.contact.state() != old_state {
                active.retries = 0;
            }
            if active.contact.state() == QsoState::Closed {
                s.completed = true;
                s.note = "Closing 73 received · exchange ended".into();
                let target = s.call.clone();
                self.end_active("Completed");
                self.remove(&target);
            }
        }
    }
    /// Process clock-driven terminal transitions even when radio/output readiness
    /// prevents planning another frame. A reserved/playing frame owns its tail.
    pub fn advance_time(&mut self, now_ms: f64) {
        if !self.enabled || self.pending.is_some() || !now_ms.is_finite() {
            return;
        }
        if self
            .active
            .as_ref()
            .is_some_and(|a| a.record.evidence.len() >= 256)
        {
            self.end_active("Evidence limit reached");
            self.enabled = false;
            return;
        }
        let reason = self
            .active
            .as_ref()
            .and_then(|a| self.attempt(a).end_reason(now_ms));
        if let Some(reason) = reason {
            self.policy = Some(PolicyDecision::End(reason));
            let target = self.active.as_ref().unwrap().contact.target.clone();
            if let Some(station) = self.items.iter_mut().find(|s| s.call == target) {
                station.attempted_cq_ms = station.latest_ms;
                if reason == EndReason::Unanswered {
                    station.sort_penalty_until_ms = now_ms + manager::UNANSWERED_PENALTY_MS;
                }
            }
            self.end_active(reason.label());
            if reason == EndReason::Completed {
                self.remove(&target);
            } else {
                self.to_back(&target);
            }
        }
    }
    /// Called near the response boundary; never roll a stale CQ into a later period.
    pub fn plan(&mut self, now_ms: f64, mine: &str, grid: &str) -> Option<Plan> {
        self.advance_time(now_ms);
        if !self.enabled || self.pending.is_some() || !now_ms.is_finite() {
            return None;
        }
        if self
            .active
            .as_ref()
            .is_none_or(|a| !a.contact.is_established())
        {
            // Include decodes arriving after the slot boundary before choosing a caller.
            self.sort_selected(self.sort_preferences, grid, now_ms);
            let decision = self.policy_decision(now_ms, mine, grid);
            self.policy = Some(decision.clone());
            if let PolicyDecision::Start {
                target,
                cq_start_ms,
                ..
            } = decision
            {
                if self.log_pending.len() >= 200 {
                    return None;
                }
                let transmit_frequency_hz = if self.find_open_frequency {
                    Some(self.open_frequency_hz.filter(|hz| {
                        hz.is_finite()
                            && (100.0..=crate::open_frequency::SEARCH_HIGH_HZ - 50.0).contains(hz)
                    })?)
                } else {
                    None
                };
                if self.active.is_some() {
                    self.end_active("Attempt replaced by fresh CQ");
                }
                let station = self.items.iter_mut().find(|s| s.call == target)?;
                station.attempted_cq_ms = cq_start_ms;
                self.active = Some(Exchange {
                    transmit_frequency_hz,
                    contact: Contact::new(station.call.clone(), cq_start_ms),
                    record: ContactRecord {
                        id: format!(
                            "{}:{}:{}:{:.0}:{}",
                            self.log_session, mine, station.call, cq_start_ms, self.next_id
                        ),
                        revision: 0,
                        station_call: mine.into(),
                        station_grid: grid.into(),
                        target: station.call.clone(),
                        grid: station.location.as_ref().map(|(_, l)| l.grid.clone()),
                        start_ms: cq_start_ms,
                        end_ms: cq_start_ms + 12_640.0,
                        outcome: "In progress".into(),
                        dial_hz: None,
                        frequency_read_ms: None,
                        evidence: station.received.clone(),
                    },
                    started_ms: cq_start_ms,
                    last_tx_ms: None,
                    transmissions: 0,
                    retries: 0,
                    sent_step: None,
                });
                self.checkpoint();
            } else if self.active.is_none() {
                return None;
            }
        }
        self.pin_active();
        let decision = self.policy_decision(now_ms, mine, grid);
        self.policy = Some(decision.clone());
        let PolicyDecision::Transmit {
            start_ms: due,
            receive_slot,
            step,
            message,
        } = decision
        else {
            if let PolicyDecision::End(reason) = decision {
                self.end_active(reason.label());
            }
            return None;
        };
        let a = self.active.as_ref()?;
        let s = self.items.iter().find(|s| s.call == a.contact.target)?;
        if wsjt_rusty::ft8::transmit::pack(&message).is_err() {
            return None;
        }
        self.next_id += 1;
        let p = Plan {
            id: self.next_id,
            target: s.call.clone(),
            message,
            start_ms: due,
            receive_slot,
            frequency_hz: a.transmit_frequency_hz.unwrap_or(s.frequency_hz),
            step,
        };
        self.pending = Some(p.clone());
        Some(p)
    }
    /// Revoke a reserved waveform when RX changes the required reply before
    /// audio starts. The UI cancels the transport and waits for release before replanning.
    /// After audio starts, finish that frame and use new state next time.
    pub fn refresh_pending(&mut self, now_ms: f64) {
        if let Some(p) = &self.pending
            && now_ms.is_finite()
            && now_ms < p.start_ms
            && !self.can_transmit(p.id)
        {
            self.pending = None;
        }
    }
    /// Preserve the actual transport/keying failure; obsolete callbacks are inert.
    pub fn fail(&mut self, id: u64, reason: &str) {
        if self.pending.as_ref().is_some_and(|p| p.id == id) {
            self.stop_with_reason(reason);
        }
    }
    pub fn complete(&mut self, id: u64, success: bool) {
        let Some(p) = self.pending.as_ref().filter(|p| p.id == id).cloned() else {
            return;
        };
        if !success {
            self.fail(id, "Transmit audio failed or canceled");
            return;
        }
        self.pending = None;
        if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target)
            && let Some(message) = s.sent_messages.get_mut(p.step)
        {
            *message = Some(p.message.clone());
            if p.step == 0 {
                s.contact_tries += 1;
            }
        }
        let completion = self
            .active
            .as_ref()
            .map(|a| manager::after_transmission(self.attempt(a), p.step, p.start_ms));
        if let Some(a) = &mut self.active {
            a.record.hear(Evidence {
                start_ms: p.start_ms,
                end_ms: p.start_ms + 12_640.0,
                direction: "TX audio".into(),
                message: p.message.clone(),
                payload: None,
                snr_db: None,
                frequency_hz: p.frequency_hz,
                received_at_ms: None,
            });
            if let Some(result) = &completion {
                a.transmissions = result.transmissions;
                a.retries = result.retries;
                a.sent_step = result.sent_step;
                a.last_tx_ms = result.last_tx_ms;
            }
            if let Some(s) = self.items.iter_mut().find(|s| s.call == p.target) {
                s.note = format!("Sent {} · awaiting response", p.message);
            }
        }
        self.checkpoint();
        if completion.is_some_and(|result| result.complete) {
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
    fn selected_conversation_keeps_received_history_but_needs_a_fresh_cq_to_start() {
        let mut board = Conversations::default();
        let payload = format!("{}001", "0".repeat(74));
        for (slot, text) in [
            "CQ K1ABC FN42",
            "K1ABC W9XYZ EN37",
            "W9XYZ K1ABC -12",
            "K1ABC W9XYZ R-10",
        ]
        .into_iter()
        .enumerate()
        {
            let start = slot as f64 * SLOT;
            board.update(
                start + 13_000.0,
                [(start, start + 12_640.0, text, payload.as_str(), -12.0)],
            );
        }
        let mut q = queue();
        q.remember_conversation(&board.items[0]);
        drop(board);
        assert_eq!(q.received_history("K1ABC").len(), 4);
        assert!(
            q.exchange_slots("K1ABC", 59_000.0)[..4]
                .iter()
                .all(|s| s.state == SlotState::Received)
        );
        assert!(q.plan(59_000.0, "W9XYZ", "EN37").is_none());
        assert!(q.pending.is_none());
        assert_eq!(q.items[0].contact_tries, 0);
        assert!(q.items[0].sent_messages.iter().all(Option::is_none));
        hear(&mut q, 60_500.0, "CQ K1ABC FN42");
        let p = q.plan(74_000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ EN37");
        assert!(matches!(q.policy, Some(PolicyDecision::Transmit { .. })));
    }

    /// User-supplied 2026-11-23 UTC evidence. Times are milliseconds after midnight.
    /// This trace must schedule 73 at 00:59:00.500; a transport interruption is
    /// a separate event and must retain its cause rather than silently reset.
    #[test]
    fn delayed_w4vpk_reports_and_self_echoes_cannot_authorize_a_stale_retry() {
        let mut q = StationQueue::default();
        q.add("W4VPK".into());
        q.add("N9IOX".into()); // Even explicitly queueing ourselves cannot make echoes control a QSO.
        q.enabled = true;
        q.require_receive_progress();
        q.find_open_frequency = true;
        q.open_frequency_hz = Some(626.8);
        let payload = format!("{}001", "0".repeat(74));
        q.hear("N9IOX", 5430827.0, "CQ W4VPK FM02", &payload, -7.7, 2621.4);
        q.receive_slot_completed(5430000.0);
        let first = q.plan(5444000.0, "N9IOX", "EM48").unwrap();
        assert_eq!(first.message, "W4VPK N9IOX EM48");
        q.complete(first.id, true);
        q.hear(
            "N9IOX",
            5445592.0,
            "W4VPK N9IOX EM48",
            &payload,
            -5.1,
            626.6,
        );
        // At the next reservation time, the receive-slot search is still running.
        assert!(q.plan(5474000.0, "N9IOX", "EM48").is_none());
        q.hear(
            "N9IOX",
            5460822.0,
            "N9IOX W4VPK -14",
            &payload,
            -15.7,
            2621.4,
        );
        let report = q.plan(5474900.0, "N9IOX", "EM48").unwrap();
        assert_eq!(report.message, "W4VPK N9IOX R-16");
        assert!(q.can_transmit(report.id));
        q.complete(report.id, true);
        q.hear(
            "N9IOX",
            5475562.0,
            "W4VPK N9IOX R-16",
            &payload,
            -3.8,
            626.6,
        );
        assert_eq!(q.items[0].snr_db, -15.7);
        assert!(q.plan(5504000.0, "N9IOX", "EM48").is_none());
        q.hear(
            "N9IOX",
            5490827.0,
            "N9IOX W4VPK -24",
            &payload,
            -9.4,
            2621.9,
        );
        q.receive_slot_completed(5490000.0);
        let retry = q.plan(5504900.0, "N9IOX", "EM48").unwrap();
        assert_eq!(retry.message, "W4VPK N9IOX R-09");
        q.complete(retry.id, true);
        q.hear(
            "N9IOX",
            5505562.0,
            "W4VPK N9IOX R-09",
            &payload,
            -4.2,
            626.6,
        );
        q.hear(
            "N9IOX",
            5520822.0,
            "N9IOX W4VPK RR73",
            &payload,
            -13.2,
            2621.9,
        );
        let last = q.plan(5534000.0, "N9IOX", "EM48").unwrap();
        assert_eq!(last.message, "W4VPK N9IOX 73");
        assert_eq!(last.frequency_hz, 626.8);
        q.complete(last.id, true);
        assert!(!q.contains("W4VPK"));
        let record = q.log_pending.values().next().unwrap();
        let sent: Vec<_> = record
            .evidence
            .iter()
            .filter(|e| e.direction == "TX audio")
            .map(|e| e.message.as_str())
            .collect();
        assert_eq!(
            sent,
            [
                "W4VPK N9IOX EM48",
                "W4VPK N9IOX R-16",
                "W4VPK N9IOX R-09",
                "W4VPK N9IOX 73"
            ]
        );
    }

    #[test]
    fn transport_authorization_rejects_late_progress_and_same_stage_content_changes() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        let stale = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        assert!(
            !q.can_transmit(stale.id),
            "transport must reject without waiting for an interval tick"
        );
        q.refresh_pending(45490.0); // Ten milliseconds before audio starts.
        assert!(q.pending.is_none());
        let p = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        q.hear(
            "W9XYZ",
            60500.0,
            "W9XYZ K1ABC -12",
            &format!("{}001", "0".repeat(74)),
            -3.0,
            1250.0,
        );
        assert!(
            !q.can_transmit(p.id),
            "same phase but new SNR/frequency also invalidates PCM"
        );
        q.revoke_pending(p.id);
        let next = q.plan(74100.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(next.message, "K1ABC W9XYZ R-03");
        assert_eq!(next.frequency_hz, 1250.0);
    }

    #[test]
    fn recorded_n9iox_kd2ruy_trace_schedules_closing_73() {
        let mut q = StationQueue::default();
        q.add("KD2RUY".into());
        q.enabled = true;
        q.find_open_frequency = true;
        q.open_frequency_hz = Some(2437.0);
        let events = [
            (
                3180801.0,
                "KD2RUY WM3I +04",
                -18.3,
                556.8,
                "10010110010111100111000111100111010010010100101100111111000111111010110111001",
            ),
            (
                3210801.0,
                "KD2RUY WM3I +04",
                -14.9,
                557.2,
                "10010110010111100111000111100111010010010100101100111111000111111010110111001",
            ),
            (
                3315931.0,
                "WM3I KD2RUY R-01",
                0.7,
                2098.4,
                "11101001001010010110011111100100101100101111001110001111001111111010110010001",
            ),
            (
                3360811.0,
                "KD2RUY WM3I RR73",
                -20.1,
                556.8,
                "10010110010111100111000111100111010010010100101100111111000111111001110101001",
            ),
            (
                3435928.0,
                "CQ KD2RUY FN12",
                -5.4,
                2097.9,
                "00000000000000000000000000100100101100101111001110001111000010100001001000001",
            ),
            (
                3465926.0,
                "CQ KD2RUY FN12",
                -2.8,
                2098.4,
                "00000000000000000000000000100100101100101111001110001111000010100001001000001",
            ),
            (
                3480576.0,
                "KD2RUY N9IOX EM48",
                2.0,
                2437.0,
                "10010110010111100111000111100000010100111011000000010001100010000100000000001",
            ),
            (
                3495926.0,
                "N9IOX KD2RUY -12",
                -9.8,
                2098.4,
                "00001010011101100000001000110100101100101111001110001111000111111010100111001",
            ),
            (
                3510541.0,
                "KD2RUY N9IOX R-10",
                1.2,
                2437.0,
                "10010110010111100111000111100000010100111011000000010001101111111010101001001",
            ),
            (
                3525926.0,
                "N9IOX KD2RUY RR73",
                0.9,
                2098.4,
                "00001010011101100000001000110100101100101111001110001111000111111001110101001",
            ),
        ];
        for (time, text, snr, hz, payload) in events {
            if time == 3480576.0 || time == 3510541.0 {
                let start = if time == 3480576.0 {
                    3480500.0
                } else {
                    3510500.0
                };
                let p = q.plan(start - 1000.0, "N9IOX", "EM48").unwrap();
                assert_eq!(p.message, text);
                assert_eq!(p.start_ms, start);
                assert_eq!(p.frequency_hz, 2437.0);
                q.complete(p.id, true);
            }
            q.hear("N9IOX", time, text, payload, snr, hz);
        }
        assert_eq!(
            q.active.as_ref().unwrap().contact.state(),
            QsoState::SendingFarewell
        );
        let p = q.plan(3539500.0, "N9IOX", "EM48").unwrap();
        assert_eq!(p.message, "KD2RUY N9IOX 73");
        assert_eq!(p.start_ms, 3540500.0);
        let mut failed = q.clone();
        failed.fail(p.id + 1, "old unrelated callback");
        assert!(failed.active.is_some());
        failed.fail(p.id, "PTT was not ready before UTC start");
        assert!(!failed.enabled);
        assert!(
            failed.items[0]
                .note
                .contains("PTT was not ready before UTC start")
        );
        assert!(
            failed
                .log_pending
                .values()
                .any(|r| r.outcome == "PTT was not ready before UTC start")
        );
        q.complete(p.id, true);
        assert!(q.log_pending.values().any(|r| r.outcome == "Completed"));
    }

    #[test]
    fn only_a_later_partner_slot_can_abandon_an_exchange_before_farewell() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        hear(&mut q, 30600.0, "N0CALL K1ABC FN42");
        let p = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ R-11");
        q.complete(p.id, true);
        hear(&mut q, 45500.0, "N0CALL K1ABC FN42"); // Our TX slot, not partner evidence.
        assert!(q.active.is_some());
        hear(&mut q, 60500.0, "N0CALL K1ABC FN42");
        assert!(q.active.is_none());
        assert!(q.items[0].note.contains("Partner answered another station"));
        assert!(!q.items[0].note.contains("awaiting response"));
    }

    #[test]
    fn rr73_keeps_closing_73_and_our_stage_evidence_when_partner_addresses_someone_else() {
        let mut q = StationQueue::default();
        q.add("KD2RUY".into());
        q.enabled = true;
        let payload = format!("{}001", "0".repeat(74));
        let hear =
            |q: &mut StationQueue, time, text| q.hear("N9IOX", time, text, &payload, -10.0, 1200.0);
        hear(&mut q, 500.0, "CQ KD2RUY FN12");
        let p = q.plan(14000.0, "N9IOX", "EM48").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "N9IOX KD2RUY -12");
        let p = q.plan(44000.0, "N9IOX", "EM48").unwrap();
        assert_eq!(p.message, "KD2RUY N9IOX R-10");
        q.complete(p.id, true);
        hear(&mut q, 60500.0, "N9IOX KD2RUY RR73");
        hear(&mut q, 60600.0, "TA1MB KD2RUY FN12");
        assert!(
            q.active.is_some(),
            "another recipient cannot cancel an owed farewell"
        );
        let mut later = q.clone();
        hear(&mut later, 90500.0, "CQ KD2RUY FN12");
        let late_plan = later.plan(104000.0, "N9IOX", "EM48").unwrap();
        assert_eq!(late_plan.message, "KD2RUY N9IOX 73");
        let reason = "Receive stopped: Audio clock discontinuity. Capture stopped.";
        later.stop_with_reason(reason);
        later.complete(late_plan.id, true);
        assert!(!later.enabled);
        assert!(later.active.is_none() && later.pending.is_none());
        assert!(later.items[0].state(104000.0).contains(reason));
        assert!(!later.items[0].note.contains("awaiting response"));
        assert!(later.log_pending.values().any(|r| r.outcome == reason));
        let slots = q.exchange_slots("KD2RUY", 73500.0);
        assert_eq!(
            slots[4]
                .evidence
                .as_ref()
                .map(|e| e.latest.message.as_str()),
            Some("N9IOX KD2RUY RR73")
        );
        let p = q
            .plan(74000.0, "N9IOX", "EM48")
            .expect("RR73 still owes our closing 73");
        assert_eq!(p.message, "KD2RUY N9IOX 73");
        hear(&mut q, 60700.0, "TA1MB KD2RUY FN12");
        q.refresh_pending(74100.0);
        assert_eq!(q.pending.as_ref().unwrap().id, p.id);
        q.complete(p.id, true);
        assert!(!q.contains("KD2RUY"));
        assert!(q.log_pending.values().any(|r| {
            r.outcome == "Completed"
                && r.evidence
                    .iter()
                    .any(|e| e.direction == "TX audio" && e.message.ends_with(" 73"))
        }));
    }

    #[test]
    fn cumulative_six_or_seven_calls_do_not_extend_each_attempts_four_call_budget() {
        let mut q = queue();
        for (cq, now) in [
            (500.0, 14000.0),
            (30500.0, 44000.0),
            (60500.0, 74000.0),
            (90500.0, 104000.0),
        ] {
            hear(&mut q, cq, "CQ K1ABC FN42");
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        q.advance_time(133700.0);
        assert!(q.active.is_none());
        // A sorting penalty is not a transmit ban: with no other fresh caller,
        // a new CQ can start another attempt while its old penalty remains.
        for (index, (cq, now)) in [
            (150500.0, 164000.0),
            (180500.0, 194000.0),
            (210500.0, 224000.0),
            (240500.0, 254000.0),
        ]
        .into_iter()
        .enumerate()
        {
            hear(&mut q, cq, "CQ K1ABC FN42");
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
            assert_eq!(q.active.as_ref().unwrap().transmissions, index + 1);
            assert_eq!(
                q.items
                    .iter()
                    .find(|s| s.call == "K1ABC")
                    .unwrap()
                    .contact_tries,
                index + 5
            );
        }
        q.advance_time(283700.0);
        assert!(q.active.is_none());
        assert_eq!(q.items.last().unwrap().sort_penalty_until_ms, 463700.0);
        assert_eq!(
            q.log_pending
                .values()
                .filter(|r| r.outcome == "No response after 4 tries")
                .count(),
            2
        );
    }

    #[test]
    fn ka5pud_repeated_reports_retry_roger_report_with_latest_snr() {
        // Operator log times relative to 2026-11-24T03:40:15Z.
        let mut q = StationQueue::default();
        q.add("KA5PUD".into());
        q.enabled = true;
        q.require_receive_progress();
        let cq = "00000000000000000000000000100100101011101110001010110011000010000011100100001";
        let report =
            "00001010011101100000001000110100101011101110001010110011000111111010101111001";
        let echo = "10010101110111000101011001100000010100111011000000010001101111111010011101001";
        q.hear(
            "N9IOX",
            -89_074.0,
            "NZ7L KA5PUD RR73",
            "10101110110111000100111000110100101011101110001010110011000111111001110101001",
            -11.5,
            1970.9,
        );
        q.hear("N9IOX", 936.0, "CQ KA5PUD EM20", cq, -5.0, 1970.9);
        let initial = q.plan(14_000.0, "N9IOX", "EM48").unwrap();
        q.complete(initial.id, true);

        for (start, available, snr, expected, transmissions) in [
            (30_926.0, 45_223.0, -21.7, "KA5PUD N9IOX R-22", 2),
            (60_921.0, 74_979.0, -11.2, "KA5PUD N9IOX R-11", 3),
            (90_921.0, 103_621.0, -8.5, "KA5PUD N9IOX R-09", 4),
        ] {
            // Each report is incremental evidence; the full search has not
            // certified receive-slot completion. Repeated text is still new RX.
            q.hear("N9IOX", start, "N9IOX KA5PUD -04", report, snr, 1971.4);
            let due = (start / SLOT).round() * SLOT + SLOT + 500.0;
            // A decode can precede reservations opening; tick again when due.
            let reply = q
                .plan(
                    f64::max(available, due - manager::RESERVATION_LEAD_MS),
                    "N9IOX",
                    "EM48",
                )
                .expect("repeated partner report must authorize a roger-report retry");
            assert_eq!(reply.message, expected);
            assert_eq!(reply.start_ms, due);
            assert!(q.can_transmit(reply.id));
            q.complete(reply.id, true);
            let active = q.active.as_ref().unwrap();
            assert_eq!(active.contact.state(), QsoState::SendingRogerReport);
            assert_eq!(active.transmissions, transmissions);
            assert_eq!(active.retries, transmissions - 1);
            if transmissions == 2 {
                q.hear("N9IOX", 45_571.0, expected, echo, -6.0, 288.5);
                assert!(
                    q.plan(73_700.0, "N9IOX", "EM48").is_none(),
                    "our own echo cannot authorize the next retry"
                );
            }
        }
        q.advance_time(133_699.0);
        assert!(q.active.is_some(), "retain the final reply window");
        q.advance_time(133_700.0);
        assert!(
            q.active.is_none(),
            "repeated reports must not reset retry limits"
        );
        assert_eq!(q.state(), ContactingState::WaitingForCq);
    }

    #[test]
    fn pi4dx_repeated_cq_retries_before_receive_worker_finishes() {
        // Times relative to 2026-11-24T03:28:30Z from the operator's log.
        let mut q = StationQueue::default();
        q.add("PI4DX".into());
        q.enabled = true;
        q.require_receive_progress();
        let cq_payload =
            "00000000000000000000000000100101110010001111110011010110000100010011010101001";
        let echo_payload =
            "10111001000111111001101011000000010100111011000000010001100010000100000000001";
        q.hear("N9IOX", 1_481.0, "CQ PI4DX JO21", cq_payload, -4.4, 2010.4);
        let first = q.plan(14_292.0, "N9IOX", "EM48").unwrap();
        assert_eq!(first.start_ms, 15_500.0);
        assert_eq!(first.message, "PI4DX N9IOX EM48");
        q.complete(first.id, true);
        q.hear(
            "N9IOX",
            15_575.0,
            "PI4DX N9IOX EM48",
            echo_payload,
            -6.3,
            285.4,
        );
        assert!(
            q.plan(43_700.0, "N9IOX", "EM48").is_none(),
            "self echo cannot authorize a retry"
        );

        for (start, available, snr, due, count) in [
            (31_490.0, 43_746.0, -4.8, 45_500.0, 2),
            (61_490.0, 73_771.0, -9.8, 75_500.0, 3),
        ] {
            // Incremental CQ is available while the full-band slot search is
            // still running, so there is no receive-slot-completed event yet.
            q.hear("N9IOX", start, "CQ PI4DX JO21", cq_payload, snr, 2010.4);
            let retry = q
                .plan(available, "N9IOX", "EM48")
                .expect("a renewed partner CQ must permit the next initial call");
            assert_eq!(retry.start_ms, due);
            assert_eq!(retry.message, "PI4DX N9IOX EM48");
            assert!(q.can_transmit(retry.id));
            q.complete(retry.id, true);
            let active = q.active.as_ref().unwrap();
            assert_eq!(active.contact.state(), QsoState::Calling);
            assert_eq!(
                active.transmissions, count,
                "CQ must not reset the attempt budget"
            );
        }
    }

    #[test]
    fn unanswered_qso_releases_active_state_and_all_stations_can_start_again() {
        for partner_replied in [false, true] {
            let mut q = queue();
            q.add("N0XYZ".into());
            q.add("W1AW".into());
            q.require_receive_progress();
            hear(&mut q, 500.0, "CQ K1ABC FN42");
            let first = q.plan(14_000.0, "W9XYZ", "EN37").unwrap();
            assert_eq!(first.message, "K1ABC W9XYZ EN37");
            q.complete(first.id, true);

            if partner_replied {
                hear(&mut q, 30_500.0, "W9XYZ K1ABC -09");
            }
            // No more partner messages: complete the remaining allowed calls or
            // roger-report replies using only receive-worker completion events.
            for receive_slot in [30_000.0, 60_000.0, 90_000.0] {
                q.receive_slot_completed(receive_slot);
                let reply = q.plan(receive_slot + 14_000.0, "W9XYZ", "EN37").unwrap();
                assert_eq!(
                    reply.message,
                    if partner_replied {
                        "K1ABC W9XYZ R-11"
                    } else {
                        "K1ABC W9XYZ EN37"
                    }
                );
                q.complete(reply.id, true);
            }

            q.advance_time(133_699.0);
            assert!(q.active.is_some(), "keep the final receive window open");
            q.receive_slot_completed(120_000.0);
            q.advance_time(133_700.0);
            assert!(q.active.is_none(), "silence must release the active QSO");
            assert!(q.pending.is_none(), "silence must release its reservation");
            assert_eq!(q.state(), ContactingState::WaitingForCq);
            assert!(q.enabled, "remain armed to select other stations");
            assert!(q.items.iter().all(|s| !s.completed));
            assert_eq!(
                q.log_pending.values().next().unwrap().outcome,
                if partner_replied {
                    "Attempt limit"
                } else {
                    "No response after 4 tries"
                }
            );
            assert!(q.plan(134_000.0, "W9XYZ", "EN37").is_none());
            assert!(q.active.is_none(), "old evidence cannot restart the QSO");

            // Test every candidate independently so FIFO/penalty ordering cannot
            // hide a station left ineligible by the abandoned exchange.
            for call in ["K1ABC", "N0XYZ", "W1AW"] {
                let mut next = q.clone();
                hear(&mut next, 150_500.0, &format!("CQ {call} FN42"));
                let plan = next.plan(164_000.0, "W9XYZ", "EN37").unwrap();
                assert_eq!(plan.target, call);
                assert_eq!(plan.message, format!("{call} W9XYZ EN37"));
                let active = next.active.as_ref().unwrap();
                assert_eq!(active.contact.state(), QsoState::Calling);
                assert_eq!(active.transmissions, 0);
                assert_eq!(active.retries, 0);
                assert_eq!(active.sent_step, None);
                assert_eq!(active.last_tx_ms, None);
                assert!(active.contact.heard.is_empty());
            }
        }
    }

    #[test]
    fn four_calls_demote_on_clock_tick_without_planning_another_transmission() {
        let mut q = queue();
        for (cq, now) in [
            (500.0, 14000.0),
            (30500.0, 44000.0),
            (60500.0, 74000.0),
            (90500.0, 104000.0),
        ] {
            hear(&mut q, cq, "CQ K1ABC FN42");
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        assert_eq!(q.items[0].contact_tries, 4);
        assert!(
            q.state_label()
                .contains("waiting for final reply (4/4 calls)")
        );
        q.advance_time(133699.0);
        assert!(q.active.is_some(), "retain the fourth call's reply window");
        // The UI can be waiting on radio readiness or a frequency query here.
        // There need not be another call to plan() to terminate this attempt.
        hear(&mut q, 120500.0, "CQ K1ABC FN42");
        q.advance_time(133700.0);
        assert!(q.active.is_none());
        assert!(q.pending.is_none());
        let station = q.items.last().unwrap();
        assert_eq!(station.call, "K1ABC");
        assert_eq!(station.sort_penalty_until_ms, 313700.0);
        assert_eq!(
            q.log_pending.values().next().unwrap().outcome,
            "No response after 4 tries"
        );
        assert!(
            q.plan(134000.0, "W9XYZ", "EN37").is_none(),
            "cannot reuse CQ heard before demotion"
        );
        hear(&mut q, 150500.0, "CQ K1ABC FN42");
        hear(&mut q, 150500.0, "CQ N0XYZ EN37");
        assert_eq!(q.plan(164000.0, "W9XYZ", "EN37").unwrap().target, "N0XYZ");
    }

    #[test]
    fn recontact_exclusion_uses_log_dates_and_expires_at_the_selected_boundary() {
        use crate::contact_log::exclusion_since;
        let day = 86_400_000.0;
        let now = 20.0 * day;
        let history = vec![CompletedContact {
            target: "K1ABC".into(),
            end_ms: now - 7.0 * day,
        }];
        let q = StationQueue::default();
        assert!(q.was_contacted("K1ABC", &history, exclusion_since(None, now)));
        assert!(q.was_contacted("K1ABC", &history, exclusion_since(Some(7), now - 1.0)));
        assert!(!q.was_contacted("K1ABC", &history, exclusion_since(Some(7), now)));
        assert!(!q.was_contacted("K1ABC", &history, exclusion_since(Some(0), now)));
        assert!(!q.was_contacted("W1XYZ", &history, exclusion_since(None, now)));
        // A fresh queue after restart makes the same decision using only the loaded log.
        let mut tracker = Conversations::default();
        let payload = format!("{}001", "0".repeat(74));
        tracker.update(
            now,
            [(
                now - 12_640.0,
                now,
                "CQ K1ABC FN42",
                payload.as_str(),
                -10.0,
            )],
        );
        let mut restarted = StationQueue::default();
        assert_eq!(
            restarted.add_cq_stations(
                &tracker.items,
                tracker.now_ms,
                &history,
                exclusion_since(None, now),
                "W9XYZ"
            ),
            0
        );
        assert_eq!(
            restarted.add_cq_stations(
                &tracker.items,
                tracker.now_ms,
                &history,
                exclusion_since(Some(7), now),
                "W9XYZ"
            ),
            1
        );
    }

    #[test]
    fn final_stage_retry_keeps_its_receive_window_open() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        for now in [44000.0, 74000.0, 104000.0] {
            let p = q.plan(now, "W9XYZ", "EN37").unwrap();
            q.complete(p.id, true);
        }
        assert!(q.plan(118000.0, "W9XYZ", "EN37").is_none());
        assert!(q.active.is_some());
        hear(&mut q, 120500.0, "W9XYZ K1ABC RR73");
        let p = q.plan(134000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(p.message, "K1ABC W9XYZ 73");
    }

    #[test]
    fn report_and_rr73_revoke_reserved_retries_without_leaving_the_qso() {
        let mut q = queue();
        assert_eq!(q.state(), ContactingState::WaitingForCq);
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let first = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(first.id, true);
        let obsolete_call = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        q.refresh_pending(44100.0);
        assert!(q.pending.is_none());
        q.complete(obsolete_call.id, true); // A stale transport callback is harmless.
        assert_eq!(q.items[0].contact_tries, 1);
        let report = q.plan(44200.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(report.message, "K1ABC W9XYZ R-11");
        q.complete(report.id, true);
        let obsolete_report = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        hear(&mut q, 60500.0, "W9XYZ K1ABC RR73");
        q.refresh_pending(74100.0);
        q.complete(obsolete_report.id, true);
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        let farewell = q.plan(74200.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(farewell.message, "K1ABC W9XYZ 73");
        assert_eq!(q.active.as_ref().unwrap().transmissions, 2);
        q.complete(farewell.id, true);
        assert_eq!(q.state(), ContactingState::WaitingForCq);
        assert!(q.plan(104000.0, "W9XYZ", "EN37").is_none());
        hear(&mut q, 90500.0, "CQ N0XYZ EN37");
        assert_eq!(q.plan(104000.0, "W9XYZ", "EN37").unwrap().target, "N0XYZ");
        q.stop();
        assert_eq!(q.state(), ContactingState::Paused);
    }

    #[test]
    fn late_report_is_not_hidden_by_newer_card_data_or_old_playback_completion() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let first = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(first.id, true);
        let retry = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        hear(&mut q, 60500.0, "CQ K1ABC FN42");
        hear(&mut q, 30500.0, "W9XYZ K1ABC -09");
        // An already started frame is accounted as sent but cannot regress RX state.
        q.refresh_pending(45500.0);
        assert!(q.pending.is_some());
        q.complete(retry.id, true);
        assert_eq!(q.active.as_ref().unwrap().retries, 0);
        let next = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(next.message, "K1ABC W9XYZ R-11");
        assert_eq!(q.items[0].latest_text, "CQ K1ABC FN42");
    }

    #[test]
    fn open_frequency_is_held_across_retries_replies_and_setting_changes() {
        let mut q = queue();
        q.find_open_frequency = true;
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        assert!(q.plan(14000.0, "W9XYZ", "EN37").is_none());
        assert!(q.active.is_none(), "wait for spectrum without consuming CQ");
        q.open_frequency_hz = Some(f32::NAN);
        assert!(q.plan(14000.0, "W9XYZ", "EN37").is_none());
        q.open_frequency_hz = Some(2100.0);
        let first = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(first.frequency_hz, 2100.0);
        q.complete(first.id, true);
        q.open_frequency_hz = Some(2400.0);
        let retry = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(retry.frequency_hz, 2100.0);
        q.complete(retry.id, true);
        q.find_open_frequency = false;
        q.hear(
            "W9XYZ",
            60500.0,
            "W9XYZ K1ABC -12",
            &format!("{}001", "0".repeat(74)),
            -11.0,
            3450.0,
        );
        let reply = q.plan(74000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(reply.frequency_hz, 2100.0);
        assert!(reply.message.contains("R-11"));
        q.cancel_active();
        q.find_open_frequency = true;
        hear(&mut q, 90500.0, "CQ N0XYZ FN42");
        let next = q.plan(104000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(next.target, "N0XYZ");
        assert_eq!(
            next.frequency_hz, 2400.0,
            "new attempt uses new spectrum choice"
        );
    }

    #[test]
    fn waterfall_marks_only_the_queued_sender_and_active_target() {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        let payload = format!("{}001", "0".repeat(74));
        assert_eq!(q.marker_status("CQ DX K1ABC FN42", &payload), (true, false));
        assert_eq!(q.marker_status("K1ABC N0XYZ -10", &payload), (false, false));
        assert_eq!(
            q.marker_status("N0XYZ <K1ABC> -10", &payload),
            (true, false)
        );
        assert_eq!(
            q.marker_status("CQ K1ABC FN42", &format!("{}000", "0".repeat(74))),
            (false, false)
        );
        q.enabled = true;
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(q.marker_status("CQ K1ABC FN42", &payload), (true, true));
        q.remove("K1ABC");
        assert_eq!(q.marker_status("CQ K1ABC FN42", &payload), (false, false));
    }

    #[test]
    fn recurring_sort_combines_keys_expires_penalties_and_pins_active() {
        let mut q = StationQueue::default();
        for call in ["K1ABC", "N0XYZ", "W1AW"] {
            q.add(call.into());
        }
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        hear(&mut q, 500.0, "CQ N0XYZ EN37");
        hear(&mut q, 500.0, "CQ W1AW EM00");
        q.items[0].snr_db = 10.0;
        q.items[1].snr_db = 0.0;
        q.items[2].snr_db = 0.0;
        q.items[0].sort_penalty_until_ms = 180000.0;
        q.sort_preferences = [true, true, true];
        q.sort_cycle(14000.0, "EN37");
        assert_eq!(
            q.items.iter().map(|s| s.call.as_str()).collect::<Vec<_>>(),
            ["N0XYZ", "W1AW", "K1ABC"]
        );
        q.to_front("W1AW");
        q.sort_cycle(14500.0, "EN37");
        assert_eq!(
            q.items[0].call, "W1AW",
            "manual priority lasts until next cycle"
        );
        q.sort_cycle(15000.0, "EN37");
        assert_eq!(q.items[0].call, "N0XYZ");
        q.sort_cycle(180000.0, "EN37");
        assert_eq!(
            q.items[0].call, "K1ABC",
            "strongest returns after penalty expires"
        );
        q.enabled = true;
        hear(&mut q, 180500.0, "CQ N0XYZ EN37");
        let p = q.plan(194000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        hear(&mut q, 210500.0, "W9XYZ N0XYZ -10");
        q.sort_cycle(225000.0, "EN37");
        assert_eq!(q.items[0].call, "N0XYZ", "active wins over all sort keys");
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
        let contacted = vec![CompletedContact {
            target: "K1ABC".into(),
            end_ms: 1000.0,
        }];
        assert_eq!(
            q.add_cq_stations(
                &tracker.items,
                tracker.now_ms,
                &contacted,
                f64::NEG_INFINITY,
                "W9XYZ"
            ),
            1
        );
        assert_eq!(
            q.items.iter().map(|s| s.call.as_str()).collect::<Vec<_>>(),
            ["N0XYZ", "W6XYZ"]
        );
        assert!(q.items[1].location.is_some());
        assert_eq!(
            q.add_cq_stations(
                &tracker.items,
                tracker.now_ms,
                &contacted,
                f64::NEG_INFINITY,
                "W9XYZ"
            ),
            0
        );
        q.clear();
        assert_eq!(
            q.add_cq_stations(&tracker.items, 80_000.0, &[], f64::NEG_INFINITY, "W9XYZ"),
            0
        );
        // Pending log writes block until commit; afterward the database summary is authoritative.
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
        assert!(completed.was_contacted("K1ABC", &[], f64::NEG_INFINITY));
        completed.recent_logs.push(record.clone());
        completed.acknowledge_log(std::slice::from_ref(&record));
        assert!(!completed.was_contacted("K1ABC", &[], f64::NEG_INFINITY));
        assert!(completed.was_contacted(
            "K1ABC",
            &[CompletedContact {
                target: record.target,
                end_ms: record.end_ms
            }],
            f64::NEG_INFINITY
        ));
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
    fn completed_frequency_updates_archived_and_acknowledged_contact() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ K1ABC FN42");
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.record_frequency(p.id, Some((14_074_000, 13900.0)));
        q.complete(p.id, true);
        hear(&mut q, 30500.0, "W9XYZ K1ABC 73");
        let saved = q.log_pending.values().cloned().collect::<Vec<_>>();
        let id = saved[0].id.clone();
        q.acknowledge_log(&saved);
        q.record_completed_frequency("unrelated", (7_074_000, 45000.0));
        assert!(q.log_pending.is_empty());
        q.record_completed_frequency(&id, (21_074_000, 46000.0));
        q.acknowledge_log(&saved);
        let updated = &q.log_pending[&id];
        assert_eq!(updated.dial_hz, Some(21_074_000));
        assert_eq!(updated.frequency_read_ms, Some(46000.0));
        assert!(updated.revision > saved[0].revision);
        assert_eq!(q.recent_logs.last().unwrap().dial_hz, updated.dial_hz);
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
    fn fresh_cq_revokes_unstarted_attempt_reservation_before_switching() {
        let mut q = StationQueue::default();
        q.add("KM4ESU".into());
        q.add("N5IH".into());
        q.enabled = true;
        hear(&mut q, 500.0, "CQ KM4ESU FN42");
        let first = q.plan(14_000.0, "W9XYZ", "EN37").unwrap();
        q.complete(first.id, true);
        let old = q.plan(44_000.0, "W9XYZ", "EN37").unwrap();
        hear(&mut q, 30_500.0, "CQ N5IH EM20");
        assert!(!q.can_transmit(old.id));
        assert!(
            q.plan(44_100.0, "W9XYZ", "EN37").is_none(),
            "pending transport must be released first"
        );
        q.refresh_pending(44_100.0);
        let replacement = q.plan(44_100.0, "W9XYZ", "EN37").unwrap();
        assert_eq!(replacement.target, "N5IH");
        q.complete(old.id, true);
        assert_eq!(
            q.pending.as_ref().unwrap().id,
            replacement.id,
            "obsolete callback must not complete new audio"
        );
        assert_eq!(q.active.as_ref().unwrap().transmissions, 0);
    }

    #[test]
    fn fresh_cq_replaces_attempt_until_partner_report_establishes_qso() {
        for (partner_message, established) in [
            (None, false),
            (Some("W9XYZ KM4ESU FN42"), false),
            (Some("W9XYZ KM4ESU -10"), true),
            (Some("W9XYZ KM4ESU R-10"), true),
        ] {
            let mut q = StationQueue::default();
            q.add("KM4ESU".into());
            q.add("N5IH".into());
            q.enabled = true;
            hear(&mut q, 500.0, "CQ KM4ESU FN42");
            let first = q.plan(14_000.0, "W9XYZ", "EN37").unwrap();
            q.complete(first.id, true);
            assert!(q.state_label().starts_with("Attempting contact with"));
            if let Some(message) = partner_message {
                hear(&mut q, 30_500.0, message);
                let reply = q.plan(44_000.0, "W9XYZ", "EN37").unwrap();
                q.complete(reply.id, true);
            }
            assert_eq!(
                q.active.as_ref().unwrap().contact.is_established(),
                established
            );
            hear(&mut q, 60_500.0, "CQ N5IH EM20");
            q.to_front("N5IH");
            let next = q.plan(74_000.0, "W9XYZ", "EN37").unwrap();
            if established {
                assert_eq!(next.target, "KM4ESU", "partner report earns QSO priority");
                assert!(q.state_label().starts_with("In QSO with"));
            } else {
                assert_eq!(next.message, "N5IH W9XYZ EN37");
                assert_eq!(next.start_ms, 75_500.0);
                assert!(q.can_transmit(next.id));
                let active = q.active.as_ref().unwrap();
                assert_eq!(active.transmissions, 0);
                assert_eq!(active.retries, 0);
                assert!(q.contains("KM4ESU"), "keep the previous station queued");
                assert!(
                    q.log_pending.values().any(
                        |r| r.target == "KM4ESU" && r.outcome == "Attempt replaced by fresh CQ"
                    )
                );
            }
        }
    }

    #[test]
    fn active_station_moves_first_and_survives_priority_changes() {
        let mut q = queue();
        hear(&mut q, 500.0, "CQ N0XYZ EN37");
        let plan = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(plan.id, true);
        hear(&mut q, 30500.0, "W9XYZ N0XYZ -10");
        let plan = q.plan(44000.0, "W9XYZ", "EN37").unwrap();
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
        assert_eq!(q.items.last().unwrap().sort_penalty_until_ms, 314000.0);
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
    fn decode_arrival_time_is_retained_for_active_and_archived_rx_evidence() {
        let mut q = StationQueue::default();
        q.add("K1ABC".into());
        q.enabled = true;
        let payload = format!("{}001", "0".repeat(74));
        q.hear("W9XYZ", 500.0, "CQ K1ABC FN42", &payload, -7.0, 1200.0);
        q.record_arrival(500.0, "CQ K1ABC FN42", 14000.0);
        let p = q.plan(14000.0, "W9XYZ", "EN37").unwrap();
        q.complete(p.id, true);
        q.hear("W9XYZ", 30500.0, "W9XYZ K1ABC -12", &payload, -8.0, 1200.0);
        q.record_arrival(30500.0, "W9XYZ K1ABC -12", 45600.0);
        q.record_arrival(30500.0, "W9XYZ K1ABC -12", 60000.0);
        let record = &q.active.as_ref().unwrap().record;
        let event = record
            .evidence
            .iter()
            .find(|e| e.message == "W9XYZ K1ABC -12")
            .unwrap();
        assert_eq!(event.received_at_ms, Some(45600.0));
        assert!(
            event.received_at_ms.unwrap() > p.start_ms + 30000.0,
            "radio frame time must not hide a decode arriving after TX"
        );
        q.stop();
        assert_eq!(
            q.log_pending
                .values()
                .next()
                .unwrap()
                .evidence
                .iter()
                .find(|e| e.message == "W9XYZ K1ABC -12")
                .unwrap()
                .received_at_ms,
            Some(45600.0)
        );
    }

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
