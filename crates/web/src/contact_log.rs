//! Local operator attempts. Received decodes and completed local audio are distinct.
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Evidence {
    pub start_ms: f64,
    pub end_ms: f64,
    pub direction: String,
    pub message: String,
    pub payload: Option<String>,
    pub snr_db: Option<f32>,
    pub frequency_hz: f32,
    /// When the UI first received this decode, distinct from its radio-frame time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub received_at_ms: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ContactRecord {
    pub id: String,
    pub revision: u64,
    pub station_call: String,
    pub station_grid: String,
    pub target: String,
    pub grid: Option<String>,
    pub start_ms: f64,
    pub end_ms: f64,
    pub outcome: String,
    #[serde(default)]
    pub dial_hz: Option<u64>,
    #[serde(default)]
    pub frequency_read_ms: Option<f64>,
    pub evidence: Vec<Evidence>,
}

impl ContactRecord {
    pub fn record_arrival(&mut self, start_ms: f64, text: &str, received_ms: f64) -> bool {
        let mut changed = false;
        for event in &mut self.evidence {
            if event.direction == "RX"
                && event.message == text
                && (event.start_ms - start_ms).abs() < 350.0
                && event.received_at_ms.is_none()
            {
                event.received_at_ms = Some(received_ms);
                changed = true;
            }
        }
        changed
    }
    pub fn hear(&mut self, event: Evidence) -> bool {
        if self.evidence.iter().any(|old| {
            old.direction == event.direction
                && old.message == event.message
                && (old.start_ms - event.start_ms).abs() < 350.0
                && (old.frequency_hz - event.frequency_hz).abs() < 4.0
        }) {
            return false;
        }
        self.end_ms = self.end_ms.max(event.end_ms);
        self.evidence.push(event);
        self.evidence
            .sort_by(|a, b| a.start_ms.total_cmp(&b.start_ms));
        for heard in self.evidence.iter().rev().filter(|e| e.direction == "RX") {
            let words: Vec<_> = heard.message.split_whitespace().collect();
            let kind = heard
                .payload
                .as_deref()
                .map(|p| crate::message_kind::classify(&heard.message, p));
            if let Some(kind) = kind
                && (words.get(1) == Some(&self.target.as_str())
                    || (kind == crate::message_kind::MessageKind::Cq
                        && words.iter().any(|&w| w == self.target)))
                && let Some(location) = crate::geography::decoded_location(&heard.message, kind)
            {
                self.grid = Some(location.grid);
                break;
            }
        }
        true
    }
}

/// Latest completed exchange per station, queried from the durable log.
#[derive(Clone, Debug, Deserialize)]
pub struct CompletedContact {
    pub target: String,
    pub end_ms: f64,
}

/// Zero days disables exclusion; None excludes all completed contacts.
pub fn exclusion_since(days: Option<u32>, now_ms: f64) -> f64 {
    match days {
        None => f64::NEG_INFINITY,
        Some(0) => f64::INFINITY,
        Some(days) => now_ms - f64::from(days) * 86_400_000.0,
    }
}
