use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::{Future, pending};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[derive(Clone)]
struct Station {
    call: String,
    cq: Rc<Cell<bool>>,
    resumable: Rc<Cell<bool>>,
    reply: Rc<Cell<Option<Reply>>>,
    active: Rc<Cell<bool>>,
    sent: Rc<RefCell<Vec<TransmitTypes>>>,
    received: Rc<RefCell<Vec<Reply>>>,
}

impl PartialEq for Station {
    fn eq(&self, other: &Self) -> bool {
        Rc::ptr_eq(&self.sent, &other.sent)
    }
}

impl Conversation for Station {
    fn call_sign(&self) -> &str {
        &self.call
    }
    fn active_cq(&self, _: &Instant, _: &Duration) -> bool {
        self.cq.get()
    }
    fn can_resume(&self, _: &Instant, _: &Duration) -> bool {
        self.resumable.get()
    }
    fn last_reply(&self) -> Reply {
        self.reply.get().unwrap_or(Reply::Cq)
    }
    fn has_reply(&self, reply: Reply) -> bool {
        self.last_reply() == reply || self.received.borrow().contains(&reply)
    }
    fn sent_messages(&self) -> Vec<TransmitTypes> {
        self.sent.borrow().clone()
    }
    fn active(&self) -> bool {
        self.active.get()
    }
}

struct Fake {
    station: Station,
    selections: Vec<Option<String>>,
    candidates: Vec<Station>,
    clock: Cell<Instant>,
    replies: RefCell<VecDeque<Option<Reply>>>,
    sent: Vec<TransmitTypes>,
    outcomes: Vec<Outcome>,
    waits: Cell<usize>,
    fail: bool,
    supersede: bool,
    supersede_farewell: bool,
    wait_step: Duration,
    cancel_tx: bool,
    keyed: Rc<Cell<bool>>,
}

impl Fake {
    fn new(replies: impl IntoIterator<Item = Option<Reply>>) -> Self {
        let station = Station {
            call: "K1ABC".into(),
            cq: Rc::new(Cell::new(true)),
            resumable: Rc::new(Cell::new(false)),
            reply: Rc::new(Cell::new(None)),
            active: Rc::new(Cell::new(true)),
            sent: Rc::new(RefCell::new(Vec::new())),
            received: Rc::new(RefCell::new(vec![Reply::Cq])),
        };
        Self {
            station: station.clone(),
            candidates: vec![station],
            selections: vec![],
            clock: Cell::new(Instant::now()),
            replies: RefCell::new(replies.into_iter().collect()),
            sent: vec![],
            outcomes: vec![],
            waits: Cell::new(0),
            fail: false,
            supersede: false,
            supersede_farewell: false,
            wait_step: Duration::from_secs(30),
            cancel_tx: false,
            keyed: Rc::new(Cell::new(false)),
        }
    }
}

struct Keyed(Rc<Cell<bool>>);
impl Drop for Keyed {
    fn drop(&mut self) {
        self.0.set(false);
    }
}

impl Control for Fake {
    type Conversation = Station;
    type Error = &'static str;
    fn set_current_conversation(&mut self, station: Option<&Station>) -> Result<(), Self::Error> {
        self.selections.push(station.map(|s| s.call.clone()));
        if let Some(station) = station {
            station.cq.set(false);
        }
        Ok(())
    }
    fn conversations(&self) -> impl Iterator<Item = Station> {
        self.candidates.clone().into_iter()
    }
    async fn wait_for_xmit_time(&self, _: &Duration) {
        if !self.outcomes.is_empty() {
            pending::<()>().await;
        }
    }
    async fn wait_for_reply(&self, station: &Station, _: Duration) {
        self.waits.set(self.waits.get() + 1);
        self.clock.set(self.clock.get() + self.wait_step);
        if let Some(reply) = self.replies.borrow_mut().pop_front().flatten() {
            station.reply.set(Some(reply));
            station.received.borrow_mut().push(reply);
        }
    }
    async fn transmit(
        &mut self,
        station: &Station,
        tt: TransmitTypes,
        _: Duration,
    ) -> Result<bool, Self::Error> {
        if tt == TransmitTypes::Farewell && self.supersede_farewell {
            self.supersede_farewell = false;
            self.station.reply.set(Some(Reply::Cq));
            return Ok(false);
        }
        if self.supersede {
            self.supersede = false;
            self.station.reply.set(Some(Reply::Report));
            return Ok(false);
        }
        self.keyed.set(true);
        let _keyed = Keyed(self.keyed.clone());
        if self.fail {
            return Err("audio failed");
        }
        if self.cancel_tx {
            pending::<()>().await;
        }
        self.sent.push(tt);
        station.sent.borrow_mut().push(tt);
        Ok(true)
    }
    async fn finished(&mut self, _: &Station, outcome: Outcome) -> Result<(), Self::Error> {
        self.outcomes.push(outcome);
        Ok(())
    }
    fn info(&mut self, _: &str) {}
    fn now(&self) -> Instant {
        self.clock.get()
    }
}

fn poll<F: Future>(future: std::pin::Pin<&mut F>) -> Poll<F::Output> {
    future.poll(&mut Context::from_waker(Waker::noop()))
}

fn finish(control: &mut Fake) -> Result<Outcome, &'static str> {
    let station = control.station.clone();
    let mut future = std::pin::pin!(finish_qso(control, &station));
    match poll(future.as_mut()) {
        Poll::Ready(result) => result,
        Poll::Pending => panic!("unexpected wait"),
    }
}

#[test]
fn exchange_reads_as_locator_report_roger_report_acknowledgment_and_73() {
    let mut control = Fake::new([
        Some(Reply::Locator),
        Some(Reply::Report),
        Some(Reply::RogerReport),
        Some(Reply::Acknowledgment),
    ]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(
        control.sent,
        [
            TransmitTypes::Reply,
            TransmitTypes::Report,
            TransmitTypes::RReport,
            TransmitTypes::R73,
            TransmitTypes::Farewell
        ]
    );
}

#[test]
fn direct_report_skips_the_unneeded_report_stage() {
    let mut control = Fake::new([Some(Reply::Report), Some(Reply::Acknowledgment)]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(
        control.sent,
        [
            TransmitTypes::Reply,
            TransmitTypes::RReport,
            TransmitTypes::Farewell
        ]
    );
}

#[test]
fn four_calls_keep_all_four_reply_windows() {
    let mut control = Fake::new([]);
    assert_eq!(finish(&mut control), Ok(Outcome::Unanswered));
    assert_eq!(control.sent.len(), 4);
    assert_eq!(control.waits.get(), 4);

    let mut control = Fake::new([None, None, None, Some(Reply::Acknowledgment)]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(control.sent.last(), Some(&TransmitTypes::Farewell));
}

#[test]
fn later_stage_has_three_tries() {
    let mut control = Fake::new([Some(Reply::Report)]);
    assert_eq!(finish(&mut control), Ok(Outcome::AttemptLimit));
    assert_eq!(
        control.sent,
        [
            TransmitTypes::Reply,
            TransmitTypes::RReport,
            TransmitTypes::RReport,
            TransmitTypes::RReport
        ]
    );
    assert_eq!(control.waits.get(), 4);
}

#[test]
fn superseded_audio_rereads_shared_reply_without_spending_an_attempt() {
    let mut control = Fake::new([Some(Reply::Acknowledgment)]);
    control.supersede = true;
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(
        control.sent,
        [TransmitTypes::RReport, TransmitTypes::Farewell]
    );
    assert_eq!(control.waits.get(), 1);
}

#[test]
fn received_73_already_completes_the_exchange() {
    let mut control = Fake::new([Some(Reply::Farewell)]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(control.sent, [TransmitTypes::Reply]);
}

#[test]
fn inactive_attempts_do_not_transmit() {
    let mut control = Fake::new([]);
    control.station.active.set(false);
    assert_eq!(finish(&mut control), Ok(Outcome::Interrupted));
    assert!(control.sent.is_empty());
}

#[test]
fn errors_and_cancellation_release_the_transmit_guard() {
    let mut control = Fake::new([]);
    control.fail = true;
    assert_eq!(finish(&mut control), Err("audio failed"));
    assert!(!control.keyed.get());
    assert!(control.sent.is_empty());

    control.fail = false;
    control.cancel_tx = true;
    let station = control.station.clone();
    let keyed = control.keyed.clone();
    let mut future = Box::pin(finish_qso(&mut control, &station));
    assert!(poll(future.as_mut()).is_pending());
    assert!(keyed.get());
    drop(future);
    assert!(!keyed.get());
}

#[test]
fn contacting_finishes_then_returns_to_waiting_for_a_station() {
    let mut control = Fake::new([Some(Reply::Acknowledgment)]);
    let mut future = Box::pin(contact_stations(&mut control));
    assert!(poll(future.as_mut()).is_pending());
    drop(future);
    assert_eq!(control.outcomes, [Outcome::Completed]);
    assert_eq!(
        control.sent,
        [TransmitTypes::Reply, TransmitTypes::Farewell]
    );
}

#[test]
fn newer_locator_rewinds_the_response_after_a_report() {
    let mut control = Fake::new([
        Some(Reply::Report),
        Some(Reply::Locator),
        Some(Reply::Report),
        Some(Reply::Acknowledgment),
    ]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(
        control.sent,
        [
            TransmitTypes::Reply,
            TransmitTypes::RReport,
            TransmitTypes::Report,
            TransmitTypes::RReport,
            TransmitTypes::Farewell
        ]
    );
}
#[test]
fn newer_cq_can_request_another_locator_without_resetting_the_attempt() {
    let mut control = Fake::new([
        Some(Reply::Locator),
        Some(Reply::Cq),
        Some(Reply::Report),
        Some(Reply::Acknowledgment),
    ]);
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(
        control.sent,
        [
            TransmitTypes::Reply,
            TransmitTypes::Report,
            TransmitTypes::Reply,
            TransmitTypes::RReport,
            TransmitTypes::Farewell
        ]
    );
}
#[test]
fn procedure_notifies_selection_then_clears_it_after_finishing() {
    let mut control = Fake::new([Some(Reply::Acknowledgment)]);
    let mut future = Box::pin(contact_stations(&mut control));
    assert!(poll(future.as_mut()).is_pending());
    drop(future);
    assert_eq!(control.selections, [Some("K1ABC".into()), None]);
}
#[test]
fn prior_attempt_requires_a_fresh_cq_to_be_selected_again() {
    let control = Fake::new([]);
    let station = control.station.clone();
    station.resumable.set(true);
    station.cq.set(false);
    station.sent.borrow_mut().extend([TransmitTypes::Reply; 3]);
    assert!(select_next(&control).is_none());

    station.cq.set(true);
    let selected = select_next(&control).unwrap();
    assert_eq!(selected.call_sign(), "K1ABC");
    assert_eq!(selected.sent_messages().len(), 3);
}

#[test]
fn selection_skips_inactive_and_non_cq_stations_in_priority_order() {
    let mut control = Fake::new([]);
    let inactive = Fake::new([]).station;
    inactive.active.set(false);
    let waiting = Fake::new([]).station;
    waiting.cq.set(false);
    waiting.resumable.set(true);
    let mut first_eligible = Fake::new([]).station;
    first_eligible.call = "K2XYZ".into();
    control.candidates = vec![inactive, waiting, first_eligible, control.station.clone()];

    assert_eq!(select_next(&control).unwrap().call_sign(), "K2XYZ");
}

#[test]
fn changing_reply_stages_still_cannot_exceed_eight_transmissions() {
    let mut control = Fake::new((0..8).map(|i| {
        Some(if i % 2 == 0 {
            Reply::Locator
        } else {
            Reply::Report
        })
    }));
    control.wait_step = Duration::from_secs(1);
    assert_eq!(finish(&mut control), Ok(Outcome::AttemptLimit));
    assert_eq!(control.sent.len(), 8);
    assert_eq!(control.waits.get(), 8);
}
#[test]
fn elapsed_attempt_limit_is_checked_after_receive() {
    let mut control = Fake::new([]);
    control.wait_step = Duration::from_secs(60);
    assert_eq!(finish(&mut control), Ok(Outcome::AttemptLimit));
    assert_eq!(control.sent.len(), 3);
}
#[test]
fn superseded_farewell_follows_the_latest_reply_without_a_closing_flag() {
    let mut control = Fake::new([Some(Reply::Acknowledgment), Some(Reply::Farewell)]);
    control.supersede_farewell = true;
    assert_eq!(finish(&mut control), Ok(Outcome::Completed));
    assert_eq!(control.sent, [TransmitTypes::Reply, TransmitTypes::Reply]);
}

#[test]
fn driver_clears_display_selection_when_an_action_fails() {
    let mut control = Fake::new([]);
    control.fail = true;
    let mut future = Box::pin(contact_stations(&mut control));
    assert!(matches!(
        poll(future.as_mut()),
        Poll::Ready(Err("audio failed"))
    ));
    drop(future);
    assert_eq!(control.selections, [Some("K1ABC".into()), None]);
    assert_eq!(control.outcomes, [Outcome::Interrupted]);
}

#[test]
fn exchange_keeps_the_original_call_limit_from_audio_receipts() {
    let mut control = Fake::new([]);
    control
        .station
        .sent
        .borrow_mut()
        .extend([TransmitTypes::Reply; 3]);
    assert_eq!(finish(&mut control), Ok(Outcome::Unanswered));
    assert_eq!(control.sent, [TransmitTypes::Reply]);
    assert_eq!(control.waits.get(), 1);
    assert_eq!(control.station.sent_messages().len(), 4);
}

#[test]
fn replacement_exits_exchange_and_leaves_the_next_selection_to_the_caller() {
    let mut control = Fake::new([]);
    let mut other = Fake::new([]).station;
    other.call = "K2XYZ".into();
    control.candidates.insert(0, other);
    assert_eq!(finish(&mut control), Ok(Outcome::Replaced));
    assert_eq!(control.sent, [TransmitTypes::Reply]);
    assert!(control.selections.is_empty());
    let next = select_next(&control).unwrap();
    assert_eq!(next.call_sign(), "K2XYZ");
}
