//! Shared received exchange stages for conversation and queued-station cards.
use crate::conversations::{ExchangeSlot, SlotState};
use leptos::prelude::*;

#[component]
pub fn ExchangeStages(slots: Signal<Vec<ExchangeSlot>>) -> impl IntoView {
    view! {
        <ol class="exchange-slots" aria-label="Exchange stages">
            {move || slots.get().into_iter().map(|slot| {
                let description = format!("{} · {}{}", slot.step.label(), slot.state.label(),
                    slot.evidence.as_ref().map(|e| format!(" · {} · {} received", e.latest.message, e.count)).unwrap_or_default());
                let mark = match slot.state {
                    SlotState::Received => "✓",
                    SlotState::Missing | SlotState::NotHeard => "▧",
                    SlotState::Expected => "›",
                    SlotState::Upcoming | SlotState::Optional => "·",
                };
                view! {
                    <li class=format!("exchange-slot {}", slot.state.class()) class:current=slot.current
                        aria-current=slot.current.then_some("step") aria-label=description>
                        <span class="exchange-stage-text">
                            <span class="exchange-label">{slot.step.short_label()}</span>
                            {slot.evidence.map(|evidence| view! {
                                <span class="exchange-message" title=evidence.latest.message.clone()>{evidence.latest.message.clone()}</span>
                            })}
                        </span>
                        <span class="exchange-mark" aria-hidden="true">{mark}</span>
                    </li>
                }
            }).collect_view()}
        </ol>
    }
}
