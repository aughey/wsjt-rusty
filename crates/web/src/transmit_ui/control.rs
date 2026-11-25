//! Device/display adapter. The procedure owns selection; these handles own RX facts.
use super::*;
use gloo_timers::future::TimeoutFuture;
use state_logic::{Control, Conversation, Instant, Outcome, Reply, TransmitTypes};
use std::{
    cell::{Cell, RefCell},
    time::Duration,
};
use wsjt_receiver::{
    contact_log::{ContactRecord, Evidence},
    contact_queue::{Exchange, Plan},
    transmit::{Contact, QsoState},
};

#[wasm_bindgen(js_namespace = console)]
extern "C" {
    #[wasm_bindgen(js_name = info)]
    fn console_info(message: &str);
}

#[derive(Clone)]
pub(super) struct BrowserConversation {
    call: String,
    opening_ms: f64,
    generation: u64,
    queue: RwSignal<StationQueue>,
    exchange: Rc<RefCell<Option<Exchange>>>,
}
impl PartialEq for BrowserConversation {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.exchange, &other.exchange)
    }
}
impl BrowserConversation {
    fn contact(&self) -> Option<Contact> {
        let mut contact = self.exchange.borrow().as_ref()?.contact.clone();
        let mine = self.exchange.borrow().as_ref()?.record.station_call.clone();
        self.queue.with_untracked(|q| {
            for e in &q.recent_received {
                if let Some(payload) = &e.payload {
                    contact.hear(&mine, e.start_ms, &e.message, payload);
                }
            }
        });
        Some(contact)
    }
    fn live(&self) -> bool {
        self.queue
            .try_with_untracked(|q| {
                q.enabled
                    && q.items.iter().any(|s| {
                        s.call == self.call && s.generation == self.generation && !s.completed
                    })
            })
            .unwrap_or(false)
    }
    fn due(&self) -> Option<f64> {
        let contact = self.contact()?;
        let data = self.exchange.borrow();
        let a = data.as_ref()?;
        let mut due = (contact.receive_slot + 1) as f64 * 15_000.0 + 500.0;
        if let Some(last) = a.last_tx_ms {
            due = due.max(last + 30_000.0);
            let earliest = js_sys::Date::now() + 200.0;
            if due < earliest {
                due += ((earliest - due) / 30_000.0).ceil() * 30_000.0;
            }
        }
        Some(due)
    }
    fn receive_ready(&self, due: f64) -> bool {
        let Some(contact) = self.contact() else {
            return false;
        };
        let data = self.exchange.borrow();
        let Some(a) = data.as_ref() else {
            return false;
        };
        let slot = ((due - 500.0) / 15_000.0).round() as i64 - 1;
        a.last_tx_ms.is_none()
            || contact.is_established()
            || contact.state() == QsoState::SendingFarewell
            || self
                .queue
                .with_untracked(|q| q.completed_receive_slot().is_some_and(|s| s >= slot))
            || contact.last_directed_slot == Some(slot)
            || (contact.state() == QsoState::Calling && contact.receive_slot == slot)
    }
    fn received_replies(&self) -> Vec<(f64, Reply)> {
        let recorded = self
            .exchange
            .borrow()
            .as_ref()
            .map(|a| a.record.evidence.clone())
            .unwrap_or_default();
        let mine = self
            .exchange
            .borrow()
            .as_ref()
            .map(|a| a.record.station_call.clone());
        self.queue.with_untracked(|q| {
            recorded
                .iter()
                .chain(q.recent_received.iter())
                .filter_map(|e| {
                    let slot = (e.start_ms / 15000.0).round() as i64;
                    let opening = (self.opening_ms / 15000.0).round() as i64;
                    if e.start_ms < self.opening_ms || slot.rem_euclid(2) != opening.rem_euclid(2) {
                        return None;
                    }
                    let words: Vec<_> = e.message.split_whitespace().collect();
                    let kind = ft8_conversations::classify(&e.message, e.payload.as_deref()?);
                    use ft8_conversations::MessageKind;
                    let reply = if kind == MessageKind::Cq && words.contains(&self.call.as_str()) {
                        Reply::Cq
                    } else {
                        if slot <= opening
                            || words.len() != 3
                            || Some(words[0]) != mine.as_deref()
                            || words[1] != self.call
                        {
                            return None;
                        }
                        match kind {
                            MessageKind::Reply => Reply::Locator,
                            MessageKind::Report => Reply::Report,
                            MessageKind::RogerReport => Reply::RogerReport,
                            MessageKind::Acknowledgment => Reply::Acknowledgment,
                            MessageKind::Signoff if words[2] == "RR73" => Reply::Acknowledgment,
                            MessageKind::Signoff => Reply::Farewell,
                            _ => return None,
                        }
                    };
                    Some((e.start_ms, reply))
                })
                .collect()
        })
    }
    fn step(tt: TransmitTypes) -> usize {
        match tt {
            TransmitTypes::Reply => 0,
            TransmitTypes::Report => 1,
            TransmitTypes::RReport => 2,
            TransmitTypes::R73 => 3,
            TransmitTypes::Farewell => 4,
        }
    }
    fn message(&self, tt: TransmitTypes, a: &Exchange, snr_db: f32) -> String {
        let report = snr_db.round().clamp(-50.0, 49.0) as i32;
        let tail = match tt {
            TransmitTypes::Reply => a.record.station_grid.clone(),
            TransmitTypes::Report => format!("{report:+03}"),
            TransmitTypes::RReport => format!("R{report:+03}"),
            TransmitTypes::R73 => "RR73".into(),
            TransmitTypes::Farewell => "73".into(),
        };
        format!("{} {} {tail}", self.call, a.record.station_call)
    }
    fn specification(&self, tt: TransmitTypes, now: f64) -> Option<Plan> {
        if !self.active() {
            return None;
        }
        let contact = self.contact()?;
        if !self.accepts_response(tt) {
            return None;
        }
        let due = self.due()?;
        if !(due - 1800.0..=due - 200.0).contains(&now) || !self.receive_ready(due) {
            return None;
        }
        let data = self.exchange.borrow();
        let a = data.as_ref()?;
        self.queue.with_untracked(|q| {
            let s = q.items.iter().find(|s| s.call == self.call)?;
            let frequency_hz = a.transmit_frequency_hz.unwrap_or(s.frequency_hz);
            if !frequency_hz.is_finite() || !(100.0..=4000.0).contains(&frequency_hz) {
                return None;
            }
            let message = self.message(tt, a, s.snr_db);
            transmit::pack(&message).ok()?;
            Some(Plan {
                id: 0,
                target: self.call.clone(),
                message,
                start_ms: due,
                receive_slot: contact.receive_slot,
                frequency_hz,
                step: Self::step(tt),
            })
        })
    }
    fn accepts_response(&self, tt: TransmitTypes) -> bool {
        self.last_reply().response() == Some(tt)
    }
}
impl Conversation for BrowserConversation {
    fn call_sign(&self) -> &str {
        &self.call
    }
    fn active_cq(&self, _: &Instant, window: &Duration) -> bool {
        self.queue.with_untracked(|q| {
            let now = js_sys::Date::now();
            let boundary = (now / 15_000.0).round() * 15_000.0;
            if !(boundary - 200.0..=boundary + 300.0).contains(&now) {
                return false;
            }
            q.items
                .iter()
                .find(|s| s.call == self.call && s.generation == self.generation)
                .is_some_and(|s| {
                    !s.completed
                        && q.log_pending.len() < 200
                        && (!q.find_open_frequency || q.open_frequency_hz.is_some())
                        && s.last_cq_ms == Some(self.opening_ms)
                        && self.opening_ms > s.attempted_cq_ms
                        && s.frequency_hz.is_finite()
                        && (100.0..=4000.0).contains(&s.frequency_hz)
                        && (0.0..=window.as_secs_f64() * 1000.0)
                            .contains(&(boundary - self.opening_ms - 12640.0))
                        && q.cq_is_calling(&self.call, boundary)
                })
        })
    }
    fn last_reply(&self) -> Reply {
        self.received_replies()
            .into_iter()
            .max_by(|a, b| a.0.total_cmp(&b.0))
            .map_or(Reply::Cq, |(_, reply)| reply)
    }
    fn has_reply(&self, reply: Reply) -> bool {
        self.received_replies()
            .iter()
            .any(|(_, received)| *received == reply)
    }
    fn sent_messages(&self) -> Vec<TransmitTypes> {
        self.exchange
            .borrow()
            .as_ref()
            .map(|a| {
                a.record
                    .evidence
                    .iter()
                    .filter(|e| e.direction == "TX audio")
                    .filter_map(|e| {
                        let tail = e.message.split_whitespace().last()?;
                        Some(match tail {
                            "73" => TransmitTypes::Farewell,
                            "RRR" | "RR73" => TransmitTypes::R73,
                            report
                                if report
                                    .strip_prefix('R')
                                    .is_some_and(|r| r.parse::<i32>().is_ok()) =>
                            {
                                TransmitTypes::RReport
                            }
                            report if report.parse::<i32>().is_ok() => TransmitTypes::Report,
                            _ => TransmitTypes::Reply,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
    fn active(&self) -> bool {
        self.live()
            && self.contact().is_none_or(|c| {
                c.abandoned() != Some(ft8_conversations::manager::EndReason::PartnerAnsweredAnother)
            })
    }
    fn age(&self) -> Duration {
        Duration::from_secs_f64(((js_sys::Date::now() - self.opening_ms) / 1000.0).max(0.0))
    }
    fn can_resume(&self, _: &Instant, window: &Duration) -> bool {
        let Some(c) = self.contact() else {
            return false;
        };
        let data = self.exchange.borrow();
        let Some(a) = data.as_ref() else {
            return false;
        };
        let now = js_sys::Date::now();
        let boundary = (now / 15_000.0).round() * 15_000.0;
        a.record.outcome == "Attempt replaced by fresh CQ"
            && self.active()
            && self.queue.with_untracked(|q| {
                q.items
                    .iter()
                    .find(|s| s.call == self.call)
                    .is_some_and(|s| s.attempted_cq_ms <= self.opening_ms)
            })
            && c.is_established()
            && c.state() != QsoState::Closed
            && (boundary - 200.0..=boundary + 300.0).contains(&now)
            && c.last_directed_slot == Some((boundary / 15000.0).round() as i64 - 1)
            && (0.0..=window.as_secs_f64() * 1000.0).contains(&(boundary - c.latest_ms - 12640.0))
            && a.last_tx_ms
                .is_some_and(|last| c.latest_ms > last && boundary + 500.0 >= last + 30000.0)
    }
}

/// All live observations/logs, with no notion of a selected conversation.
/// Kept mounted across Stop so delayed RX can still update accepted log records.
pub(super) struct BrowserConversations {
    queue: RwSignal<StationQueue>,
    mine: RwSignal<String>,
    grid: RwSignal<String>,
    entries: RefCell<Vec<BrowserConversation>>,
    revision: Cell<Option<u64>>,
    next_id: Cell<u64>,
}
impl BrowserConversations {
    pub fn new(
        queue: RwSignal<StationQueue>,
        mine: RwSignal<String>,
        grid: RwSignal<String>,
    ) -> Self {
        Self {
            queue,
            mine,
            grid,
            entries: RefCell::new(Vec::new()),
            revision: Cell::new(None),
            next_id: Cell::new(0),
        }
    }
    fn list(&self) -> Vec<BrowserConversation> {
        let mine = self.mine.get_untracked();
        let mut entries = self.entries.borrow_mut();
        self.queue.with_untracked(|q| {
            q.items
                .iter()
                .filter(|s| s.call != mine && transmit::pack_call(&s.call).is_ok())
                .filter_map(|s| {
                    let opening_ms = s.last_cq_ms?;
                    if let Some(c) = entries.iter().find(|c| {
                        c.call == s.call
                            && c.generation == s.generation
                            && c.opening_ms == opening_ms
                    }) {
                        return Some(c.clone());
                    }
                    let c = BrowserConversation {
                        call: s.call.clone(),
                        opening_ms,
                        generation: s.generation,
                        queue: self.queue,
                        exchange: Rc::new(RefCell::new(None)),
                    };
                    entries.push(c.clone());
                    if entries.len() > 200 {
                        entries.remove(0);
                    }
                    Some(c)
                })
                .collect()
        })
    }
    pub fn refresh(&self) {
        let revision = self.queue.with_untracked(|q| q.observation_revision);
        if self.revision.replace(Some(revision)) == Some(revision) {
            return;
        }
        let events = self.queue.with_untracked(|q| q.recent_received.clone());
        let entries = self.entries.borrow().clone();
        for c in &entries {
            let mut data = c.exchange.borrow_mut();
            let Some(a) = data.as_mut() else {
                continue;
            };
            if let Some(record) = self
                .queue
                .with_untracked(|q| q.attempt_record(&a.record.id).cloned())
                && record.revision > a.record.revision
            {
                a.record = record;
            }
            let mut changed = false;
            for e in &events {
                if e.start_ms < c.opening_ms || !e.message.split_whitespace().any(|w| w == c.call) {
                    continue;
                }
                // A newer opening for the same station owns its subsequent RX.
                if entries.iter().any(|other| {
                    other.call == c.call
                        && other.opening_ms > c.opening_ms
                        && other.opening_ms <= e.start_ms
                        && other.exchange.borrow().is_some()
                }) {
                    continue;
                }
                let complete =
                    a.record.outcome != "In progress" && a.record.accepts_late_completion(e);
                if e.start_ms <= c.opening_ms + 180000.0
                    || complete
                    || a.record.accepts_replaced_partner_evidence(e)
                {
                    if let Some(payload) = &e.payload {
                        a.contact
                            .hear(&a.record.station_call, e.start_ms, &e.message, payload);
                    }
                    if a.record.evidence.len() < 256 {
                        changed |= a.record.hear(e.clone());
                    }
                    if let Some(arrival) = e.received_at_ms {
                        changed |= a.record.record_arrival(e.start_ms, &e.message, arrival);
                    }
                    if complete {
                        a.record.outcome = "Completed".into();
                        changed = true;
                    }
                }
            }
            if changed {
                let full = a.record.evidence.len() >= 255 && a.record.outcome == "In progress";
                if full {
                    a.record.outcome = "Evidence limit reached".into();
                }
                a.record.revision += 1;
                self.queue.update(|q| {
                    q.publish_attempt(a);
                    if full {
                        q.enabled = false;
                    }
                    if a.record.outcome == "Completed"
                        && q.items
                            .iter()
                            .any(|s| s.call == c.call && s.generation == c.generation)
                        && q.active
                            .as_ref()
                            .is_none_or(|shown| shown.record.id != a.record.id)
                    {
                        q.remove(&c.call);
                    }
                });
            }
        }
    }
    fn evidence_changed(&self, current: &BrowserConversation) -> bool {
        if current
            .due()
            .is_none_or(|due| js_sys::Date::now() < due - 700.0)
        {
            return false;
        }
        if current.contact().is_some_and(|c| c.is_established()) {
            return false;
        }
        let now = Instant::now();
        let window = Duration::from_secs(5);
        let candidates = self.list();
        if candidates.iter().any(|candidate| {
            self.entries
                .borrow()
                .iter()
                .any(|c| c.call == candidate.call && c.can_resume(&now, &window))
        }) {
            return true;
        }
        current
            .exchange
            .borrow()
            .as_ref()
            .is_some_and(|a| a.last_tx_ms.is_some())
            && candidates
                .into_iter()
                .find(|c| c.active() && c.active_cq(&now, &window))
                .is_some_and(|c| c.call != current.call)
    }
    fn observe_selection(&self, c: &BrowserConversation) -> Result<(), String> {
        self.refresh();
        if c.exchange.borrow().is_none() {
            let id = self.next_id.get() + 1;
            self.next_id.set(id);
            let mut data = c.exchange.borrow_mut();
            *data = Some(self.queue.with_untracked(|q| {
                if q.log_pending.len() >= 200 {
                    return Err("Contact log backlog is full".into());
                }
                let s = q
                    .items
                    .iter()
                    .find(|s| s.call == c.call)
                    .ok_or("Station removed")?;
                let transmit_frequency_hz = if q.find_open_frequency {
                    Some(q.open_frequency_hz.ok_or("Waiting for an open frequency")?)
                } else {
                    None
                };
                Ok::<_, String>(Exchange {
                    transmit_frequency_hz,
                    contact: Contact::new(c.call.clone(), c.opening_ms),
                    record: ContactRecord {
                        id: format!(
                            "{}:{}:{}:{:.0}:async-{id}",
                            q.log_session,
                            self.mine.get_untracked(),
                            c.call,
                            c.opening_ms
                        ),
                        revision: 1,
                        station_call: self.mine.get_untracked(),
                        station_grid: self.grid.get_untracked(),
                        target: c.call.clone(),
                        grid: s.location.as_ref().map(|(_, l)| l.grid.clone()),
                        start_ms: c.opening_ms,
                        end_ms: c.opening_ms + 12640.0,
                        outcome: "In progress".into(),
                        dial_hz: None,
                        frequency_read_ms: None,
                        evidence: s.received.clone(),
                    },
                    started_ms: c.opening_ms,
                    last_tx_ms: None,
                    transmissions: 0,
                    retries: 0,
                    sent_step: None,
                })
            })?);
        } else {
            let mut data = c.exchange.borrow_mut();
            let a = data.as_mut().unwrap();
            a.record.outcome = "In progress".into();
            a.record.revision += 1;
        }
        let data = c.exchange.borrow();
        let a = data.as_ref().unwrap();
        self.queue.update(|q| {
            if let Some(s) = q.items.iter_mut().find(|s| s.call == c.call) {
                s.attempted_cq_ms = c.opening_ms;
            }
            q.active = Some(a.clone());
            q.procedure_status = Some(format!("Selected {}", c.call));
            q.publish_attempt(a);
        });
        Ok(())
    }
}

#[derive(Clone)]
pub(super) struct FrameApproval {
    station: BrowserConversation,
    pub plan: Plan,
    message: TransmitTypes,
    conversations: Rc<BrowserConversations>,
}
impl FrameApproval {
    pub fn valid(&self) -> bool {
        if !self.station.active() || self.conversations.evidence_changed(&self.station) {
            return false;
        }
        let data = self.station.exchange.borrow();
        let Some(a) = data.as_ref() else {
            return false;
        };
        self.station.queue.with_untracked(|q| {
            let Some(s) = q.items.iter().find(|s| s.call == self.station.call) else {
                return false;
            };
            self.station.accepts_response(self.message)
                && self.plan.message == self.station.message(self.message, a, s.snr_db)
                && self.plan.frequency_hz == a.transmit_frequency_hz.unwrap_or(s.frequency_hz)
        })
    }
}

pub(super) struct BrowserControl {
    pub queue: RwSignal<StationQueue>,
    pub radio: RadioControl,
    pub output_id: RwSignal<String>,
    pub use_radio: RwSignal<bool>,
    pub open_frequency: ReadSignal<Option<(f64, f32)>>,
    pub frequency_query_busy: RwSignal<bool>,
    pub status: RwSignal<String>,
    pub busy: RwSignal<bool>,
    pub current_plan: StoredValue<Option<u64>, LocalStorage>,
    pub tx: JsValue,
    pub conversations: Rc<BrowserConversations>,
    pub approval: StoredValue<Option<FrameApproval>, LocalStorage>,
}
impl BrowserControl {
    fn action_failed(&self, c: &BrowserConversation, reason: &str) {
        if let Some(a) = c.exchange.borrow_mut().as_mut() {
            a.record.outcome = reason.to_owned();
            a.record.revision += 1;
            self.queue.update(|q| q.publish_attempt(a));
        }
    }
    fn ready(&self) -> bool {
        self.queue.with_untracked(|q| q.enabled)
            && !self.frequency_query_busy.get_untracked()
            && !transmitter_busy(&self.tx)
            && transmitter_prepared(&self.tx)
            && (!self.use_radio.get_untracked() || self.radio.ready())
    }
    fn waiting(&self, status: &str) {
        if self.status.get_untracked() != status {
            console_info(&format!("[QSO] {status}"));
            self.status.set(status.into());
            self.queue
                .update(|q| q.procedure_status = Some(status.into()));
        }
    }
    fn update_frequency(&self) {
        self.queue.update(|q| {
            q.open_frequency_hz = crate::open_frequency::recent_choice(
                self.open_frequency.get_untracked(),
                js_sys::Date::now(),
            )
        });
    }
}
struct TransmissionGuard {
    tx: JsValue,
    queue: RwSignal<StationQueue>,
    current_plan: StoredValue<Option<u64>, LocalStorage>,
    id: u64,
    completed: bool,
}
impl Drop for TransmissionGuard {
    fn drop(&mut self) {
        if !self.completed && self.current_plan.try_with_value(|p| *p).flatten() == Some(self.id) {
            self.queue.try_update(|q| q.pending = None);
            self.current_plan.try_set_value(None);
            let _ = cancel_current(&self.tx);
        }
    }
}
impl Control for BrowserControl {
    type Conversation = BrowserConversation;
    type Error = String;
    fn set_current_conversation(&mut self, c: Option<&BrowserConversation>) -> Result<(), String> {
        match c {
            Some(c) => {
                self.update_frequency();
                self.conversations.observe_selection(c)
            }
            None => {
                self.queue.update(|q| q.active = None);
                Ok(())
            }
        }
    }
    fn conversations(&self) -> impl Iterator<Item = BrowserConversation> {
        self.update_frequency();
        let fresh = self.conversations.list();
        let entries = self.conversations.entries.borrow();
        fresh
            .into_iter()
            .flat_map(|candidate| {
                let mut observed: Vec<_> = entries
                    .iter()
                    .filter(|c| {
                        c.call == candidate.call
                            && c.generation == candidate.generation
                            && c.opening_ms != candidate.opening_ms
                            && c.exchange.borrow().is_some()
                    })
                    .cloned()
                    .collect();
                observed.push(candidate);
                observed
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
    async fn wait_for_xmit_time(&self, offset: &Duration) {
        loop {
            TimeoutFuture::new(25).await;
            self.update_frequency();
            let now = js_sys::Date::now();
            let boundary = (now / 15000.0).round() * 15000.0;
            if self.ready()
                && (boundary - offset.as_secs_f64() * 1000.0..=boundary + 300.0).contains(&now)
            {
                return;
            }
            TimeoutFuture::new(25).await;
        }
    }
    async fn wait_for_reply(&self, c: &BrowserConversation, timeout: Duration) {
        let deadline = js_sys::Date::now() + timeout.as_secs_f64() * 1000.0;
        while c.active() && js_sys::Date::now() < deadline {
            if c.last_reply() == Reply::Farewell {
                return;
            }
            if let Some(due) = c.due() {
                if js_sys::Date::now() >= due - 1800.0 && !c.receive_ready(due) {
                    self.waiting("Listening · receive slot still decoding");
                }
                let lead = if matches!(
                    c.last_reply(),
                    Reply::Report | Reply::RogerReport | Reply::Acknowledgment | Reply::Farewell
                ) {
                    1800.0
                } else {
                    700.0
                };
                if js_sys::Date::now() >= due - lead {
                    return;
                }
            }
            TimeoutFuture::new(25).await;
        }
    }
    async fn transmit(
        &mut self,
        c: &BrowserConversation,
        tt: TransmitTypes,
        timeout: Duration,
    ) -> Result<bool, String> {
        let deadline = js_sys::Date::now() + timeout.as_secs_f64() * 1000.0;
        let mut plan = loop {
            if js_sys::Date::now() >= deadline {
                return Ok(false);
            }
            if !c.active() || !c.accepts_response(tt) {
                return Ok(false);
            }
            if self.conversations.evidence_changed(c) {
                return Ok(false);
            }
            if self.ready()
                && let Some(plan) = c.specification(tt, js_sys::Date::now())
                && plan.start_ms <= deadline
            {
                break plan;
            }
            if let Some(due) = c.due()
                && c.exchange
                    .borrow()
                    .as_ref()
                    .is_some_and(|a| a.last_tx_ms.is_none())
                && js_sys::Date::now() > due - 200.0
            {
                return Err("CQ response deadline missed".into());
            }
            if let Some(due) = c.due()
                && js_sys::Date::now() >= due - 1800.0
                && !c.receive_ready(due)
            {
                self.waiting("Listening · receive slot still decoding");
            }
            TimeoutFuture::new(25).await;
        };
        let id = self.conversations.next_id.get() + 1;
        self.conversations.next_id.set(id);
        plan.id = id;
        self.queue.update(|q| {
            q.pending = Some(plan.clone());
            q.procedure_status = Some(format!("Transmit next · {}", plan.message));
        });
        self.approval.set_value(Some(FrameApproval {
            station: c.clone(),
            plan: plan.clone(),
            message: tt,
            conversations: self.conversations.clone(),
        }));
        let samples = transmit::audio(&plan.message, plan.frequency_hz).map_err(|reason| {
            self.action_failed(c, reason);
            reason.to_owned()
        })?;
        if self.use_radio.get_untracked() {
            self.radio
                .reserve()
                .inspect_err(|reason| self.action_failed(c, reason))?;
        }
        self.current_plan.set_value(Some(id));
        let mut guard = TransmissionGuard {
            tx: self.tx.clone(),
            queue: self.queue,
            current_plan: self.current_plan,
            id,
            completed: false,
        };
        self.busy.set(true);
        self.info(&format!("{} · scheduled {}", c.call, plan.message));
        let setup = play_transmission(
            &self.tx,
            &samples,
            plan.receive_slot as f64,
            &self.output_id.get_untracked(),
            self.use_radio.get_untracked(),
            plan.start_ms,
            id as f64,
        );
        JsFuture::from(setup).await.map_err(|error| {
            let reason = error_text(error);
            self.action_failed(c, &reason);
            reason
        })?;
        while transmitter_busy(&self.tx) {
            TimeoutFuture::new(25).await;
        }
        let outcome = transmitter_outcome(&self.tx);
        let reason = transmitter_status(&self.tx);
        let current = self.current_plan.get_value() == Some(id);
        let success = current && outcome == "complete";
        guard.completed = true;
        self.current_plan.set_value(None);
        self.approval.set_value(None);
        self.queue.update(|q| q.pending = None);
        self.busy.set(false);
        self.info(&reason);
        if success {
            self.conversations.refresh();
            let contact = c.contact().unwrap();
            let mut data = c.exchange.borrow_mut();
            let a = data.as_mut().unwrap();
            a.contact = contact;
            a.transmissions += 1;
            a.retries = if a.sent_step == Some(plan.step) {
                a.retries + 1
            } else {
                1
            };
            a.sent_step = Some(plan.step);
            a.last_tx_ms = Some(plan.start_ms);
            if let Some((hz, ms)) = self.radio.frequency()
                && a.record.dial_hz.is_none()
            {
                a.record.dial_hz = Some(hz);
                a.record.frequency_read_ms = Some(ms);
            }
            a.record.hear(Evidence {
                start_ms: plan.start_ms,
                end_ms: plan.start_ms + 12640.0,
                direction: "TX audio".into(),
                message: plan.message.clone(),
                payload: None,
                snr_db: None,
                frequency_hz: plan.frequency_hz,
                received_at_ms: None,
            });
            a.record.revision += 1;
            self.queue.update(|q| {
                if let Some(s) = q.items.iter_mut().find(|s| s.call == c.call) {
                    s.sent_messages[plan.step] = Some(plan.message.clone());
                    if plan.step == 0 {
                        s.contact_tries += 1;
                    }
                }
                q.publish_attempt(a);
            });
            Ok(true)
        } else if outcome == "superseded" || !current {
            Ok(false)
        } else {
            self.action_failed(c, &reason);
            Err(reason)
        }
    }
    async fn finished(&mut self, c: &BrowserConversation, outcome: Outcome) -> Result<(), String> {
        self.conversations.refresh();
        let mut data = c.exchange.borrow_mut();
        let Some(a) = data.as_mut() else {
            return Ok(());
        };
        a.record.outcome = if outcome == Outcome::Interrupted && a.record.outcome != "In progress" {
            a.record.outcome.clone()
        } else {
            match outcome {
                Outcome::Completed => "Completed",
                Outcome::Unanswered => "No response after 4 tries",
                Outcome::AttemptLimit => "Attempt limit",
                Outcome::Interrupted => a.contact.abandoned().map_or("Interrupted", |r| r.label()),
                Outcome::Replaced => "Attempt replaced by fresh CQ",
            }
            .into()
        };
        a.record.revision += 1;
        self.queue.update(|q| {
            q.publish_attempt(a);
            q.active = None;
            q.procedure_status = Some(format!("Attempt ended · {}", a.record.outcome));
            if let Some(s) = q.items.iter_mut().find(|s| s.call == c.call) {
                s.note = format!(
                    "Attempt ended: {} · waiting for a fresh CQ",
                    a.record.outcome
                );
                if outcome != Outcome::Replaced {
                    s.attempted_cq_ms = s.latest_ms;
                }
                if outcome == Outcome::Unanswered {
                    s.sort_penalty_until_ms = js_sys::Date::now() + 180000.0;
                }
            }
            if outcome == Outcome::Completed {
                q.remove(&c.call);
            } else if outcome != Outcome::Replaced {
                q.to_back(&c.call);
            }
        });
        Ok(())
    }
    fn info(&mut self, info: &str) {
        console_info(&format!("[QSO] {info}"));
        self.status.set(info.to_owned());
    }
    fn now(&self) -> Instant {
        Instant::now()
    }
}
