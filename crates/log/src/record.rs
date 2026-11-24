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
    #[serde(default)]
    pub evidence: Vec<Evidence>,
}

impl ContactRecord {
    pub fn validate(&self) -> Result<(), crate::LogError> {
        let invalid = || crate::LogError("Invalid contact record".into());
        if self.id.is_empty()
            || self.id.len() > 4096
            || self.target.len() > 128
            || self.station_call.len() > 128
            || self.revision > 9_007_199_254_740_991
            || self.evidence.len() > 256
            || self.end_ms < self.start_ms
        {
            return Err(invalid());
        }
        crate::timestamp(self.start_ms).map_err(|_| invalid())?;
        crate::timestamp(self.end_ms).map_err(|_| invalid())?;
        if let Some(ms) = self.frequency_read_ms {
            crate::timestamp(ms).map_err(|_| invalid())?;
        }
        for event in &self.evidence {
            crate::timestamp(event.start_ms).map_err(|_| invalid())?;
            crate::timestamp(event.end_ms).map_err(|_| invalid())?;
            if event.end_ms < event.start_ms
                || !event.frequency_hz.is_finite()
                || event.snr_db.is_some_and(|v| !v.is_finite())
                || event.message.len() > 4096
                || event.payload.as_ref().is_some_and(|p| p.len() > 256)
            {
                return Err(invalid());
            }
            if let Some(ms) = event.received_at_ms {
                crate::timestamp(ms).map_err(|_| invalid())?;
            }
        }
        Ok(())
    }
    /// Keep bounded partner evidence after CQ replacement, even when it cannot
    /// resume TX. Old recovery arrival does not refresh the evidence's frame time.
    pub fn accepts_replaced_partner_evidence(&self, event: &Evidence) -> bool {
        if self.outcome != "Attempt replaced by fresh CQ"
            || event.direction != "RX"
            || event.start_ms > self.start_ms + ft8_conversations::manager::ATTEMPT_TIMEOUT_MS
            || event.start_ms < self.start_ms
            || ((event.start_ms / 15_000.0).round() as i64).rem_euclid(2)
                != ((self.start_ms / 15_000.0).round() as i64).rem_euclid(2)
        {
            return false;
        }
        let words: Vec<_> = event.message.split_whitespace().collect();
        let from_partner = words.get(1) == Some(&self.target.as_str())
            || (event.message.starts_with("CQ ") && words.iter().any(|w| *w == self.target));
        from_partner
            && self
                .evidence
                .iter()
                .any(|e| e.direction == "TX audio" && e.end_ms <= event.start_ms)
    }

    /// Reconcile a late sign-off with this attempt's received and sent evidence.
    /// This updates logging only and never authorizes another transmission.
    pub fn accepts_late_completion(&self, event: &Evidence) -> bool {
        use ft8_conversations::{Contact, QsoState};
        if self.outcome == "Completed" || event.direction != "RX" {
            return false;
        }
        let Some(payload) = event.payload.as_deref() else {
            return false;
        };
        let words: Vec<_> = event.message.split_whitespace().collect();
        if words.len() != 3
            || words[0] != self.station_call
            || words[1] != self.target
            || !matches!(words[2], "RR73" | "73")
            || ft8_conversations::classify(&event.message, payload)
                != ft8_conversations::MessageKind::Signoff
        {
            return false;
        }
        let mut contact = Contact::new(self.target.clone(), self.start_ms);
        for heard in self.evidence.iter().filter(|e| e.direction == "RX") {
            if let Some(payload) = heard.payload.as_deref() {
                contact.hear(&self.station_call, heard.start_ms, &heard.message, payload);
            }
        }
        if !contact.is_established() {
            return false;
        }
        // Bound attribution to three minutes after successful advanced-stage audio.
        // Initial calls, failed playback and a bare sign-off do not establish a QSO.
        let sent = self.evidence.iter().rev().find(|e| {
            if e.direction != "TX audio" {
                return false;
            }
            let words: Vec<_> = e.message.split_whitespace().collect();
            words.len() == 3
                && words[0] == self.target
                && words[1] == self.station_call
                && (words[2] == "RR73"
                    || words[2].strip_prefix('R').is_some_and(|report| {
                        report.starts_with(['+', '-']) && report.parse::<i32>().is_ok()
                    }))
        });
        let Some(sent) = sent else { return false };
        if event.start_ms < sent.end_ms || event.start_ms > sent.start_ms + 180_000.0 {
            return false;
        }
        contact.hear(&self.station_call, event.start_ms, &event.message, payload);
        contact.last_directed_slot == Some((event.start_ms / 15_000.0).round() as i64)
            && matches!(
                contact.state(),
                QsoState::SendingFarewell | QsoState::Closed
            )
    }

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
                .map(|p| ft8_conversations::classify(&heard.message, p));
            if let Some(kind) = kind
                && (words.get(1) == Some(&self.target.as_str())
                    || (kind == ft8_conversations::MessageKind::Cq
                        && words.iter().any(|&w| w == self.target)))
                && let Some(location) = grid_text(&heard.message, kind)
            {
                self.grid = Some(location);
                break;
            }
        }
        true
    }
}

/// Latest completed exchange per station, queried from the durable log.
#[derive(Clone, Debug, Serialize, Deserialize)]
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

// Preserve decoded grid text; coordinate calculations belong to UI adapters.
fn grid_text(text: &str, kind: ft8_conversations::MessageKind) -> Option<String> {
    if !matches!(
        kind,
        ft8_conversations::MessageKind::Cq | ft8_conversations::MessageKind::Reply
    ) {
        return None;
    }
    let grid = text.split_whitespace().last()?.to_ascii_uppercase();
    let b = grid.as_bytes();
    if !matches!(b.len(), 4 | 6)
        || !b[..2].iter().all(|c| (b'A'..=b'R').contains(c))
        || !b[2..4].iter().all(u8::is_ascii_digit)
        || (b.len() == 6 && !b[4..].iter().all(|c| (b'A'..=b'X').contains(c)))
    {
        return None;
    }
    Some(grid)
}
