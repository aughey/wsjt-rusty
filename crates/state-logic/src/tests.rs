use super::*;
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::{Future, pending};
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

#[derive(Clone)]
struct Station {
    reply: Rc<Cell<Option<Reply>>>,
    active: Rc<Cell<bool>>,
}

impl Conversation for Station {
    fn active_cq(&self, _: &Instant, _: &Duration) -> bool {
        true
    }
    fn reply(&self) -> Option<Reply> {
        self.reply.get()
    }
    fn active(&self) -> bool {
        self.active.get()
    }
}

struct Fake {
    station: Station,
    clock: Cell<Instant>,
    replies: RefCell<VecDeque<Option<Reply>>>,
    sent: Vec<TransmitTypes>,
    outcomes: Vec<Outcome>,
    waits: Cell<usize>,
    fail: bool,
    cancel_tx: bool,
    keyed: Rc<Cell<bool>>,
}

impl Fake {
    fn new(replies: impl IntoIterator<Item = Option<Reply>>) -> Self {
        Self {
            station: Station {
                reply: Rc::new(Cell::new(None)),
                active: Rc::new(Cell::new(true)),
            },
            clock: Cell::new(Instant::now()),
            replies: RefCell::new(replies.into_iter().collect()),
            sent: vec![],
            outcomes: vec![],
            waits: Cell::new(0),
            fail: false,
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
    fn conversations(&self) -> impl Iterator<Item = Station> {
        [self.station.clone()].into_iter()
    }
    async fn wait_for_xmit_time(&self, _: &Duration) {
        if !self.outcomes.is_empty() {
            pending::<()>().await;
        }
    }
    async fn wait_for_reply(&self, _: &Station) {
        self.waits.set(self.waits.get() + 1);
        self.clock.set(self.clock.get() + Duration::from_secs(30));
        if let Some(reply) = self.replies.borrow_mut().pop_front().flatten() {
            self.station.reply.set(Some(reply));
        }
    }
    async fn transmit(&mut self, _: &Station, tt: TransmitTypes) -> Result<(), Self::Error> {
        self.keyed.set(true);
        let _keyed = Keyed(self.keyed.clone());
        if self.fail {
            return Err("audio failed");
        }
        if self.cancel_tx {
            pending::<()>().await;
        }
        self.sent.push(tt);
        Ok(())
    }
    async fn finished(&mut self, _: &Station, outcome: Outcome) -> Result<(), Self::Error> {
        self.outcomes.push(outcome);
        Ok(())
    }
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
