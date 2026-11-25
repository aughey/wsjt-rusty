//! Procedural QSO logic. Selection and the running exchange live on the async stack.
use std::time::Duration;
pub use web_time::Instant;

pub trait Conversation: Clone + PartialEq {
    fn call_sign(&self) -> &str;
    fn active_cq(&self, now: &Instant, window: &Duration) -> bool;
    /// Latest validated partner frame. A newer earlier-stage reply can rewind it.
    fn last_reply(&self) -> Reply;
    /// Received evidence, retained independently of the latest reply.
    fn has_reply(&self, reply: Reply) -> bool {
        self.last_reply() == reply
    }
    /// Completed audio receipts, used to retain limits when resuming an attempt.
    fn sent_messages(&self) -> Vec<TransmitTypes> {
        Vec::new()
    }
    fn active(&self) -> bool;
    fn age(&self) -> Duration {
        Duration::ZERO
    }
    fn can_resume(&self, _now: &Instant, _window: &Duration) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    Cq,
    Locator,
    Report,
    RogerReport,
    Acknowledgment,
    Farewell,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransmitTypes {
    Reply,
    Report,
    RReport,
    R73,
    Farewell,
}
impl Reply {
    pub fn response(self) -> Option<TransmitTypes> {
        Some(match self {
            Self::Cq => TransmitTypes::Reply,
            Self::Locator => TransmitTypes::Report,
            Self::Report => TransmitTypes::RReport,
            Self::RogerReport => TransmitTypes::R73,
            Self::Acknowledgment => TransmitTypes::Farewell,
            Self::Farewell => return None,
        })
    }
}
impl TransmitTypes {
    /// Closing audio completes the local exchange; it does not confirm RF delivery.
    pub fn expects_reply(self) -> bool {
        self != Self::Farewell
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Unanswered,
    AttemptLimit,
    Interrupted,
    Replaced,
}

#[allow(async_fn_in_trait)]
pub trait Control {
    type Conversation: Conversation;
    type Error;
    /// One-way notification to the display/log adapter.
    fn set_current_conversation(
        &mut self,
        conversation: Option<&Self::Conversation>,
    ) -> Result<(), Self::Error>;
    fn conversations(&self) -> impl Iterator<Item = Self::Conversation>;
    async fn wait_for_xmit_time(&self, offset_before: &Duration);
    async fn wait_for_reply(&self, conversation: &Self::Conversation, timeout: Duration);
    /// True means completed audio; false means superseded before audio.
    /// Dropping this future cancels output and arranges unkeying.
    async fn transmit(
        &mut self,
        conversation: &Self::Conversation,
        tt: TransmitTypes,
        timeout: Duration,
    ) -> Result<bool, Self::Error>;
    /// Publish the outcome; the log service retains pending writes until commit.
    async fn finished(
        &mut self,
        conversation: &Self::Conversation,
        outcome: Outcome,
    ) -> Result<(), Self::Error>;
    fn info(&mut self, info: &str);
    fn now(&self) -> Instant;
}

pub async fn wait_for_station_sending_cq<C: Control>(control: &mut C) -> C::Conversation {
    loop {
        control
            .wait_for_xmit_time(&Duration::from_millis(200))
            .await;
        let now = control.now();
        if let Some(station) = control
            .conversations()
            .find(|c| c.active() && c.active_cq(&now, &Duration::from_secs(5)))
        {
            return station;
        }
        control.info("No fresh eligible CQ; waiting for the next opportunity");
    }
}

pub async fn contact_stations<C: Control>(control: &mut C) -> Result<(), C::Error> {
    loop {
        control.info("Waiting for available CQ station");
        let station = loop {
            control
                .wait_for_xmit_time(&Duration::from_millis(200))
                .await;
            if let Some(station) = select_next(control) {
                break station;
            }
        };
        control.set_current_conversation(Some(&station))?;
        control.info(&format!("Attempting to contact: {}", station.call_sign()));
        let outcome = match exchange(control, &station).await {
            Ok(outcome) => outcome,
            Err(error) => {
                let _ = control.finished(&station, Outcome::Interrupted).await;
                let _ = control.set_current_conversation(None);
                return Err(error);
            }
        };
        control.info(&format!(
            "{}: exchange ended: {outcome:?}",
            station.call_sign()
        ));
        control.finished(&station, outcome).await?;
        control.set_current_conversation(None)?;
    }
}

fn select_next<C: Control>(control: &C) -> Option<C::Conversation> {
    let now = control.now();
    let window = Duration::from_secs(5);
    control
        .conversations()
        .find(|c| c.active() && c.active_cq(&now, &window))
}

// for candidate in control.conversations() {
//     if let Some(index) = suspended.iter().position(|c| {
//         c.call_sign() == candidate.call_sign()
//             && c.active()
//             && c.can_resume(&now, &window)
//             && c.sent_messages().len() < 8
//             && c.age() < Duration::from_secs(180)
//     }) {
//         return Some(suspended.remove(index));
//     }
// }
// let station = control
//     .conversations()
//     .find(|c| c.active() && c.active_cq(&now, &window))?;
// suspended.retain(|c| c.call_sign() != station.call_sign());
// Some(station)
//}

fn should_replace<C: Control>(control: &C, station: &C::Conversation, sent: usize) -> bool {
    if [
        Reply::Report,
        Reply::RogerReport,
        Reply::Acknowledgment,
        Reply::Farewell,
    ]
    .into_iter()
    .any(|reply| station.has_reply(reply))
    {
        return false;
    }
    let now = control.now();
    let window = Duration::from_secs(5);
    if control
        .conversations()
        .any(|c| c.call_sign() != station.call_sign() && c.active() && c.can_resume(&now, &window))
    {
        return true;
    }
    sent > 0
        && control
            .conversations()
            .find(|c| c.active() && c.active_cq(&now, &window))
            .is_some_and(|c| c.call_sign() != station.call_sign())
}

/// Only accounting: successful sends and elapsed time, with no protocol flags.
struct Budget {
    started: Instant,
    sent: Vec<TransmitTypes>,
}
impl Budget {
    fn remaining(&self, now: Instant, age: Duration) -> Duration {
        Duration::from_secs(180).saturating_sub(now.duration_since(self.started).max(age))
    }
    fn tries(&self, message: TransmitTypes) -> usize {
        if message == TransmitTypes::Reply {
            self.sent.iter().filter(|m| **m == message).count()
        } else {
            self.sent
                .iter()
                .rev()
                .take_while(|m| **m == message)
                .count()
        }
    }
    fn limit(&self, message: TransmitTypes, now: Instant, age: Duration) -> Option<Outcome> {
        if self.sent.len() >= 8 || self.remaining(now, age).is_zero() {
            return Some(Outcome::AttemptLimit);
        }
        match (message, self.tries(message)) {
            (TransmitTypes::Reply, 4..) => Some(Outcome::Unanswered),
            (_, 3..) if message != TransmitTypes::Reply => Some(Outcome::AttemptLimit),
            _ => None,
        }
    }
}

pub async fn exchange<C: Control>(
    control: &mut C,
    station: &C::Conversation,
) -> Result<Outcome, C::Error> {
    let mut budget = Budget {
        started: control.now(),
        sent: station.sent_messages(),
    };
    loop {
        if !station.active() {
            return Ok(Outcome::Interrupted);
        }
        let reply = station.last_reply();
        control.info(&format!(
            "{}: last received reply: {reply:?}",
            station.call_sign()
        ));
        let Some(message) = reply.response() else {
            return Ok(Outcome::Completed);
        };
        if let Some(limit) = budget.limit(message, control.now(), station.age()) {
            return Ok(limit);
        }
        if should_replace(control, station, budget.sent.len()) {
            return Ok(Outcome::Replaced);
        }
        control.info(&format!(
            "{}: sending {message:?}, try {} ({} completed transmissions total)",
            station.call_sign(),
            budget.tries(message) + 1,
            budget.sent.len()
        ));
        let sent = control
            .transmit(
                station,
                message,
                budget.remaining(control.now(), station.age()),
            )
            .await
            .inspect_err(|_| control.info("Transmission failed; ending contacting"))?;
        if !sent {
            control.info("Superseded before audio; rechecking without spending a try");
            continue;
        }
        budget.sent.push(message);
        if !message.expects_reply() {
            return Ok(Outcome::Completed);
        }
        control.info(&format!(
            "{}: {message:?} audio completed; waiting for a reply",
            station.call_sign()
        ));
        control
            .wait_for_reply(station, budget.remaining(control.now(), station.age()))
            .await;
    }
}

/// Compatibility entry point for callers exchanging with a supplied station.
pub async fn finish_qso<C: Control>(
    control: &mut C,
    station: &C::Conversation,
) -> Result<Outcome, C::Error> {
    exchange(control, station).await
}

#[cfg(test)]
mod tests;
