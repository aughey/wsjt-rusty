//! Bounded cross-lane receive history. Deduplication never rewrites emitted text.
use crate::{conversations::Conversations, receiver::TimedDecode};
use std::collections::{BTreeMap, BTreeSet};

type Key = (String, u64, u32);
fn key(row: &TimedDecode) -> Key {
    (
        row.decode.payload.clone(),
        row.start_utc_ms.to_bits(),
        row.decode.frequency_hz.to_bits(),
    )
}

#[derive(Default)]
pub struct History {
    pub decodes: Vec<TimedDecode>,
    pub conversations: Conversations,
    arrivals: BTreeMap<Key, f64>,
}
/// First-seen events, including events older than the retained display limit.
pub struct Ingested {
    pub fresh: Vec<TimedDecode>,
}

impl History {
    pub fn ingest(&mut self, now_ms: f64, incoming: Vec<TimedDecode>) {
        self.ingest_at(now_ms, now_ms, incoming);
    }
    /// Decode arrival is retained independently of frame time and processed time.
    /// Cross-lane duplicates never refresh the first arrival or rendered text.
    pub fn ingest_at(
        &mut self,
        now_ms: f64,
        arrival_ms: f64,
        incoming: Vec<TimedDecode>,
    ) -> Ingested {
        let mut fresh: Vec<TimedDecode> = Vec::new();
        for row in incoming {
            if self.decodes.iter().chain(&fresh).any(|old| {
                old.decode.payload == row.decode.payload
                    && (old.start_utc_ms - row.start_utc_ms).abs() < 350.0
                    && (old.decode.frequency_hz - row.decode.frequency_hz).abs() < 4.0
            }) {
                continue;
            }
            fresh.push(row);
        }
        for row in &fresh {
            self.arrivals.insert(key(row), arrival_ms);
        }
        if fresh.is_empty() {
            if now_ms > self.conversations.now_ms {
                self.conversations.update(now_ms, []);
            }
            return Ingested { fresh };
        }
        self.conversations.update(
            now_ms,
            fresh.iter().map(|r| {
                (
                    r.start_utc_ms,
                    r.end_utc_ms,
                    r.decode.message.as_str(),
                    r.decode.payload.as_str(),
                    r.decode.snr_db,
                )
            }),
        );
        self.decodes.extend(fresh.iter().cloned());
        self.decodes.sort_by(|a, b| {
            b.start_utc_ms
                .total_cmp(&a.start_utc_ms)
                .then(a.decode.frequency_hz.total_cmp(&b.decode.frequency_hz))
        });
        self.decodes.truncate(500);
        let kept: BTreeSet<_> = self.decodes.iter().map(key).collect();
        self.arrivals.retain(|key, _| kept.contains(key));
        Ingested { fresh }
    }
    pub fn arrived_at(&self, row: &TimedDecode) -> Option<f64> {
        self.decodes
            .iter()
            .find(|old| {
                old.decode.payload == row.decode.payload
                    && (old.start_utc_ms - row.start_utc_ms).abs() < 350.0
                    && (old.decode.frequency_hz - row.decode.frequency_hz).abs() < 4.0
            })
            .and_then(|first| self.arrivals.get(&key(first)).copied())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wsjt_rusty::decoder::{AudioDecoder, Ft8Decoder, Mode, Settings};

    #[test]
    fn duplicate_lanes_preserve_text_but_later_periods_are_separate() {
        let audio = wsjt_rusty::ft8::transmit::audio("CQ K1ABC FN42", 1200.0).unwrap();
        let decode = Ft8Decoder::new(Settings::defaults(Mode::Ft8))
            .unwrap()
            .decode(&audio)
            .remove(0);
        let row = |ms, text: &str| {
            let mut decode = decode.clone();
            decode.message = text.into();
            TimedDecode {
                start_utc_ms: ms,
                end_utc_ms: ms + 12640.0,
                decode,
            }
        };
        let mut history = History::default();
        history.ingest_at(14000.0, 13500.0, vec![row(0.0, "CQ K1ABC FN42")]);
        history.ingest_at(14000.0, 16000.0, vec![row(100.0, "changed")]);
        assert_eq!(history.decodes.len(), 1);
        assert_eq!(history.decodes[0].decode.message, "CQ K1ABC FN42");
        assert_eq!(history.arrived_at(&history.decodes[0]), Some(13500.0));
        history.ingest(29000.0, vec![row(15000.0, "CQ K1ABC FN42")]);
        assert_eq!(history.decodes.len(), 2);
    }
}
