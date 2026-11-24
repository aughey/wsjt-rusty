//! Bounded cross-lane receive history. Deduplication never rewrites emitted text.
use crate::{conversations::Conversations, receiver::TimedDecode};

#[derive(Default)]
pub struct History {
    pub decodes: Vec<TimedDecode>,
    pub conversations: Conversations,
}

impl History {
    pub fn ingest(&mut self, now_ms: f64, incoming: Vec<TimedDecode>) {
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
        self.decodes.extend(fresh);
        self.decodes.sort_by(|a, b| {
            b.start_utc_ms
                .total_cmp(&a.start_utc_ms)
                .then(a.decode.frequency_hz.total_cmp(&b.decode.frequency_hz))
        });
        self.decodes.truncate(500);
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
        history.ingest(14000.0, vec![row(0.0, "CQ K1ABC FN42")]);
        history.ingest(14000.0, vec![row(100.0, "changed")]);
        assert_eq!(history.decodes.len(), 1);
        assert_eq!(history.decodes[0].decode.message, "CQ K1ABC FN42");
        history.ingest(29000.0, vec![row(15000.0, "CQ K1ABC FN42")]);
        assert_eq!(history.decodes.len(), 2);
    }
}
