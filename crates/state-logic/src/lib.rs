use std::time::Duration;

pub trait Conversation {
    fn active_cq(&self, now: &std::time::Instant, window: &Duration) -> bool;
}

pub enum TransmitTypes {
    //CQ,
    Reply,
    //Report,
    RReport,
    //RRR,
    R73,
}

// This experimental executor is intentionally usable on a single-thread runtime;
// implementations need not return Send futures.
#[allow(async_fn_in_trait)]
pub trait Control {
    type Conversation: Conversation;

    fn conversations(&self) -> impl Iterator<Item = Self::Conversation>;
    async fn wait_for_xmit_time(&self, offset_before: &Duration);
    async fn transmit(&mut self, conversation: Self::Conversation, tt: TransmitTypes);
    fn now(&self) -> std::time::Instant;
}

pub async fn wait_for_station_sending_cq<C: Control>(control: &mut C) -> C::Conversation {
    let cq_window = Duration::from_secs_f32(5.0);

    loop {
        // We wait until we're ready to transmit before checking.
        control.wait_for_xmit_time(&Duration::ZERO).await;

        let Some(best_cq) = control
            .conversations()
            .find(|c| c.active_cq(&control.now(), &cq_window))
        else {
            continue;
        };

        return best_cq;
    }
}

pub async fn contact_stations<C: Control>(control: &mut C) {
    loop {
        // No active station.  Find an active station.
        let best_cq = wait_for_station_sending_cq(control).await;

        // Send reply to this station
        control.transmit(best_cq, TransmitTypes::Reply).await;
    }
}
