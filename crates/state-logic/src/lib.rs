//! Procedural QSO logic. The running exchange lives on the async stack.
use std::time::{Duration, Instant};

pub trait Conversation {
    fn call_sign(&self) -> &str;
    fn active_cq(&self, now: &Instant, window: &Duration) -> bool;
    /// Latest validated reply from this station to us, not local sent audio.
    /// Delayed replies may advance this value, but must never move it backward.
    fn reply(&self) -> Option<Reply>;
    /// False when this attempt is removed, replaced, stopped, or abandoned.
    /// A received acknowledgment still owes 73 despite subsequent partner CQ.
    fn active(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Reply {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Completed,
    Unanswered,
    AttemptLimit,
    Interrupted,
}

// Implementations may use a single-thread runtime and non-Send futures.
#[allow(async_fn_in_trait)]
pub trait Control {
    type Conversation: Conversation;
    type Error;

    /// Live handles in queue priority order; observations continue during awaits.
    fn conversations(&self) -> impl Iterator<Item = Self::Conversation>;
    /// Wait for a new selection boundary. Freshness is measured at the intended
    /// TX boundary, and an old CQ must not roll into a later slot.
    async fn wait_for_xmit_time(&self, offset_before: &Duration);
    /// Wait through the partner's receive turn until our next TX opportunity.
    /// Observe receive readiness, replacement/removal and attempt deadlines.
    async fn wait_for_reply(&self, conversation: &Self::Conversation);
    /// Send the requested response using the station's latest measurements.
    /// Revalidate before keying and audio; return success only for completed
    /// audio. Dropping this future must cancel output and arrange unkeying,
    /// including when a key acknowledgment is pending.
    async fn transmit(
        &mut self,
        conversation: &Self::Conversation,
        tt: TransmitTypes,
    ) -> Result<(), Self::Error>;
    /// Update the queue and persist the outcome; acknowledge only a durable log
    /// commit. Cancellation must retain accepted writes for retry.
    async fn finished(
        &mut self,
        conversation: &Self::Conversation,
        outcome: Outcome,
    ) -> Result<(), Self::Error>;
    fn info(&mut self, info: &str);
    fn now(&self) -> Instant;
}

pub async fn wait_for_station_sending_cq<C: Control>(control: &mut C) -> C::Conversation {
    let cq_window = Duration::from_secs(5);

    loop {
        // Choose a station just before the transmit boundary.
        control
            .wait_for_xmit_time(&Duration::from_millis(200))
            .await;
        let now = control.now();
        if let Some(station) = control
            .conversations()
            .find(|c| c.active() && c.active_cq(&now, &cq_window))
        {
            return station;
        }
    }
}

pub async fn contact_stations<C: Control>(control: &mut C) -> Result<(), C::Error> {
    loop {
        control.info("Waiting for available CQ station");
        let station = wait_for_station_sending_cq(control).await;
        control.info(&format!("Attempting to contact: {}", station.call_sign()));
        let outcome = finish_qso(control, &station).await?;
        control.finished(&station, outcome).await?;
    }
}

pub async fn finish_qso<C: Control>(
    control: &mut C,
    station: &C::Conversation,
) -> Result<Outcome, C::Error> {
    let mut budget = Budget {
        started: control.now(),
        transmissions: 0,
    };

    // Call with our locator, allowing four unanswered calls.
    control.info("Sending locator");
    if let Some(end) = send_until_reply(control, station, TransmitTypes::Reply, &mut budget).await?
    {
        return Ok(end);
    }

    // Exchange reports. A later reply skips stages we no longer need to send.
    if let Some(end) =
        send_until_reply(control, station, TransmitTypes::Report, &mut budget).await?
    {
        return Ok(end);
    }

    if let Some(end) =
        send_until_reply(control, station, TransmitTypes::RReport, &mut budget).await?
    {
        return Ok(end);
    }

    if let Some(end) = send_until_reply(control, station, TransmitTypes::R73, &mut budget).await? {
        return Ok(end);
    }

    // RRR/RR73 owes a closing 73. A received 73 already closes the exchange.
    if station.reply() != Some(Reply::Farewell) {
        if !station.active() {
            return Ok(Outcome::Interrupted);
        }
        if budget.transmissions >= 8
            || control.now().duration_since(budget.started) >= Duration::from_secs(180)
        {
            return Ok(Outcome::AttemptLimit);
        }
        control.transmit(station, TransmitTypes::Farewell).await?;
    }
    Ok(Outcome::Completed)
}

struct Budget {
    started: Instant,
    transmissions: usize,
}

async fn send_until_reply<C: Control>(
    control: &mut C,
    station: &C::Conversation,
    message: TransmitTypes,
    budget: &mut Budget,
) -> Result<Option<Outcome>, C::Error> {
    let (expected, attempts) = match message {
        TransmitTypes::Reply => (Reply::Locator, 4),
        TransmitTypes::Report => (Reply::Report, 3),
        TransmitTypes::RReport => (Reply::RogerReport, 3),
        TransmitTypes::R73 => (Reply::Acknowledgment, 3),
        TransmitTypes::Farewell => (Reply::Farewell, 1),
    };
    for _ in 0..attempts {
        if !station.active() {
            return Ok(Some(Outcome::Interrupted));
        }
        if station.reply().is_some_and(|reply| reply >= expected) {
            return Ok(None);
        }
        if budget.transmissions >= 8
            || control.now().duration_since(budget.started) >= Duration::from_secs(180)
        {
            return Ok(Some(Outcome::AttemptLimit));
        }

        control.transmit(station, message).await?;
        budget.transmissions += 1;

        // Keep the final allowed transmission's reply window open too.
        control.wait_for_reply(station).await;
    }
    if !station.active() {
        return Ok(Some(Outcome::Interrupted));
    }
    if station.reply().is_some_and(|reply| reply >= expected) {
        return Ok(None);
    }
    Ok(Some(if message == TransmitTypes::Reply {
        Outcome::Unanswered
    } else {
        Outcome::AttemptLimit
    }))
}

#[cfg(test)]
mod tests;
