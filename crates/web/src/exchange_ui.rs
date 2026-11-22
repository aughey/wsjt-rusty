//! Shared received exchange stages for conversation and queued-station cards.
use crate::conversations::{ExchangeSlot, SlotState};
use leptos::prelude::*;

#[derive(Clone, Default)]
pub struct TransmitStage {
    pub status: String,
    pub message: String,
    pub transmitting: bool,
    pub sent: bool,
}

#[component]
pub fn ExchangeStages(
    slots: Signal<Vec<ExchangeSlot>>,
    #[prop(optional)] transmit: Option<Signal<Vec<TransmitStage>>>,
) -> impl IntoView {
    view! {
        <ol class="exchange-slots" aria-label="Exchange stages">
            {move || slots.get().into_iter().enumerate().map(|(index, slot)| {
                let tx = transmit.and_then(|stages| stages.get().get(index).cloned()).unwrap_or_default();
                let mut description = format!("{} · {}{}", slot.step.label(), slot.state.label(),
                    slot.evidence.as_ref().map(|e| format!(" · {} · {} received", e.latest.message, e.count)).unwrap_or_default());
                if !tx.status.is_empty() {
                    description.push_str(&format!(" · Local audio: {} · {}", tx.status, tx.message));
                }
                let mark = match slot.state {
                    SlotState::Received => "✓",
                    SlotState::Missing | SlotState::NotHeard => "▧",
                    SlotState::Expected => "›",
                    SlotState::Upcoming | SlotState::Optional => "·",
                };
                view! {
                    <li class=format!("exchange-slot {}", slot.state.class()) class:current=slot.current
                        class:transmitting=tx.transmitting class:sent=tx.sent
                        aria-current=slot.current.then_some("step") aria-label=description>
                        <span class="exchange-stage-text">
                            <span class="exchange-label">{slot.step.short_label()}</span>
                            {slot.evidence.map(|evidence| view! {
                                <span class="exchange-message" title=evidence.latest.message.clone()>{evidence.latest.message.clone()}</span>
                            })}
                        </span>
                        {(!tx.status.is_empty()).then(|| view! {
                            <span class="exchange-transmit">
                                <strong class="tx-step-status">{tx.status}</strong>
                                <span class="tx-step-message" title=tx.message.clone()>{tx.message.clone()}</span>
                            </span>
                        })}
                        <span class="exchange-mark" aria-hidden="true">{mark}</span>
                    </li>
                }
            }).collect_view()}
        </ol>
    }
}
