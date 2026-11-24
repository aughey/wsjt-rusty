//! Browser presentation adapter for the standalone conversation tracker.
use crate::geography::{CardSort, Location, distance_key};
use crate::message_kind::MessageKind;
pub use ft8_conversations::{ExchangeSlot, ExchangeStep, SlotState, Stage};
use std::{collections::BTreeMap, ops::Deref};

#[derive(Clone, Debug)]
pub struct Conversation {
    inner: ft8_conversations::Conversation,
    pub locations: BTreeMap<String, (f64, Location)>,
}

impl Deref for Conversation {
    type Target = ft8_conversations::Conversation;
    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl Conversation {
    pub fn location(&self) -> Option<&Location> {
        self.locations.get(&self.caller).map(|(_, l)| l)
    }

    pub fn color(&self, now_ms: f64) -> &'static str {
        MessageKind::from(self.inner.message_kind(now_ms)).color()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Conversations {
    inner: ft8_conversations::Conversations,
    pub items: Vec<Conversation>,
    pub now_ms: f64,
}

impl Conversations {
    pub fn sort(&mut self, by: CardSort, mine: &str) {
        let now_ms = self.now_ms;
        self.inner.sort_by(|a, b| match by {
            CardSort::Signal => b
                .messages
                .last()
                .map(|h| h.snr_db)
                .unwrap_or(f32::NEG_INFINITY)
                .total_cmp(
                    &a.messages
                        .last()
                        .map(|h| h.snr_db)
                        .unwrap_or(f32::NEG_INFINITY),
                ),
            CardSort::State => {
                (a.status(now_ms) == "Stale", a.stage).cmp(&(b.status(now_ms) == "Stale", b.stage))
            }
            CardSort::Distance => {
                let location = |c: &ft8_conversations::Conversation| {
                    c.grids
                        .get(&c.caller)
                        .and_then(|(_, grid)| Location::parse(grid))
                };
                distance_key(location(a).as_ref(), mine)
                    .total_cmp(&distance_key(location(b).as_ref(), mine))
            }
        });
        self.refresh();
    }

    pub fn remember(&mut self, conversation: &ft8_conversations::Conversation) {
        self.inner.remember(conversation);
        self.refresh();
    }

    pub fn update<'a>(
        &mut self,
        now_ms: f64,
        messages: impl IntoIterator<Item = (f64, f64, &'a str, &'a str, f32)>,
    ) {
        self.inner.update(now_ms, messages);
        self.refresh();
    }

    fn refresh(&mut self) {
        self.now_ms = self.inner.now_ms;
        self.items = self
            .inner
            .items
            .iter()
            .map(|c| Conversation {
                locations: c
                    .grids
                    .iter()
                    .filter_map(|(call, (ms, grid))| {
                        Location::parse(grid).map(|l| (call.clone(), (*ms, l)))
                    })
                    .collect(),
                inner: c.clone(),
            })
            .collect();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presentation_preserves_sorting_locations_and_progress_colors() {
        let mut tracker = Conversations::default();
        let payload = format!("{}001", "0".repeat(74));
        tracker.update(
            13_000.0,
            [
                (0.0, 12_640.0, "CQ K1ABC FN42", payload.as_str(), -12.0),
                (0.0, 12_640.0, "CQ N0XYZ EN37", payload.as_str(), -10.0),
            ],
        );
        tracker.sort(CardSort::Distance, "EN37");
        assert_eq!(tracker.items[0].caller, "N0XYZ");
        assert_eq!(tracker.items[0].location().unwrap().grid, "EN37");
        assert_eq!(
            tracker.items[0].color(tracker.now_ms),
            MessageKind::Cq.color()
        );
        tracker.update(
            28_000.0,
            [(
                15_000.0,
                27_640.0,
                "N0XYZ W9XYZ EN50",
                payload.as_str(),
                -9.0,
            )],
        );
        assert_eq!(tracker.items[0].display_slot, 0);
        assert_eq!(tracker.items[0].locations["W9XYZ"].1.grid, "EN50");
        assert_eq!(
            tracker.items[0].color(tracker.now_ms),
            MessageKind::Reply.color()
        );
        tracker.sort(CardSort::State, "EN37");
        assert_eq!(tracker.items[0].caller, "K1ABC");
        tracker.sort(CardSort::Signal, "EN37");
        assert_eq!(tracker.items[0].caller, "N0XYZ");
        tracker.update(27_640.0 + ft8_conversations::STALE_MS, []);
        assert_eq!(
            tracker.items[0].color(tracker.now_ms),
            MessageKind::Other.color()
        );
    }
}
