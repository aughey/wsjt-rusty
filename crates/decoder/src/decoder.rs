//! Protocol-independent decoding and bounded, overlapping stream windows.
use serde::Serialize;
use std::{collections::VecDeque, error::Error, fmt};
pub const SAMPLE_RATE: usize = 12000;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Ft8,
    Wspr,
}
impl fmt::Display for Mode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Ft8 => "FT8",
            Self::Wspr => "WSPR",
        })
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Decode {
    pub mode: Mode,
    pub start_seconds: f64,
    pub frequency_hz: f32,
    pub frequency_reference: &'static str,
    pub snr_db: f32,
    pub message: String,
    pub payload: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wspr: Option<WsprData>,
}
#[derive(Clone, Debug, Serialize)]
pub struct WsprData {
    pub callsign: String,
    pub locator: Option<String>,
    pub power_dbm: u8,
    pub message_type: u8,
    pub drift_hz: f32,
}
#[derive(Clone, Debug)]
pub struct Settings {
    pub low_hz: f32,
    pub high_hz: f32,
    pub passes: usize,
}
impl Settings {
    pub fn defaults(mode: Mode) -> Self {
        match mode {
            Mode::Ft8 => Self {
                low_hz: 100.0,
                high_hz: 4000.0,
                passes: 2,
            },
            Mode::Wspr => Self {
                low_hz: 1390.0,
                high_hz: 1610.0,
                passes: 3,
            },
        }
    }
    fn validate(&self, mode: Mode) -> Result<(), Box<dyn Error>> {
        let (minimum, maximum) = match mode {
            Mode::Ft8 => (10.0, 5900.0),
            Mode::Wspr => (200.0, 5800.0),
        };
        if !self.low_hz.is_finite()
            || !self.high_hz.is_finite()
            || self.low_hz < minimum
            || self.high_hz > maximum
            || self.low_hz >= self.high_hz
        {
            return Err("invalid frequency range (FT8: 10–5900 Hz; WSPR: 200–5800 Hz)".into());
        }
        if mode == Mode::Wspr && self.high_hz - self.low_hz > 300.0 {
            return Err("WSPR search width must be at most 300 Hz".into());
        }
        if !(1..=4).contains(&self.passes) {
            return Err("passes must be between 1 and 4".into());
        }
        Ok(())
    }
}
/// Implement this trait to add a protocol. Input is normalized mono f32 at 12 kHz.
/// Window sizes and hop lengths must guarantee coverage without UTC alignment.
pub trait AudioDecoder: Send {
    fn mode(&self) -> Mode;
    fn minimum_samples(&self) -> usize;
    fn window_samples(&self) -> usize;
    fn hop_samples(&self) -> usize;
    fn decode(&mut self, audio: &[f32]) -> Vec<Decode>;
}
pub struct Ft8Decoder {
    settings: Settings,
    messages: wsjt_ft8::message::Messages,
}
impl Ft8Decoder {
    pub fn new(settings: Settings) -> Result<Self, Box<dyn Error>> {
        settings.validate(Mode::Ft8)?;
        Ok(Self {
            settings,
            messages: Default::default(),
        })
    }
}
impl AudioDecoder for Ft8Decoder {
    fn mode(&self) -> Mode {
        Mode::Ft8
    }
    fn minimum_samples(&self) -> usize {
        13 * SAMPLE_RATE
    }
    fn window_samples(&self) -> usize {
        30 * SAMPLE_RATE
    }
    fn hop_samples(&self) -> usize {
        15 * SAMPLE_RATE
    }
    fn decode(&mut self, audio: &[f32]) -> Vec<Decode> {
        wsjt_ft8::decode_with_context(
            audio,
            self.settings.low_hz,
            self.settings.high_hz,
            self.settings.passes,
            &mut self.messages,
        )
        .into_iter()
        .map(|r| Decode {
            mode: Mode::Ft8,
            start_seconds: r.start_seconds as f64,
            frequency_hz: r.frequency_hz,
            frequency_reference: "lowest_tone",
            snr_db: r.snr_db,
            message: r.message,
            payload: r.payload,
            wspr: None,
        })
        .collect()
    }
}
pub struct WsprDecoder {
    settings: Settings,
    messages: wsjt_wspr::message::Messages,
}
impl WsprDecoder {
    pub fn new(settings: Settings) -> Result<Self, Box<dyn Error>> {
        settings.validate(Mode::Wspr)?;
        Ok(Self {
            settings,
            messages: Default::default(),
        })
    }
}
impl AudioDecoder for WsprDecoder {
    fn mode(&self) -> Mode {
        Mode::Wspr
    }
    fn minimum_samples(&self) -> usize {
        108 * SAMPLE_RATE
    }
    fn window_samples(&self) -> usize {
        132 * SAMPLE_RATE
    }
    fn hop_samples(&self) -> usize {
        16 * SAMPLE_RATE
    }
    fn decode(&mut self, audio: &[f32]) -> Vec<Decode> {
        wsjt_wspr::decode(
            audio,
            self.settings.low_hz,
            self.settings.high_hz,
            self.settings.passes,
            &mut self.messages,
        )
        .into_iter()
        .map(|r| Decode {
            mode: Mode::Wspr,
            start_seconds: r.start_seconds as f64,
            frequency_hz: r.frequency_hz,
            frequency_reference: "center",
            snr_db: r.snr_db,
            message: r.decoded.text,
            payload: r.payload.iter().map(|&b| char::from(b'0' + b)).collect(),
            wspr: Some(WsprData {
                callsign: r.decoded.callsign,
                locator: r.decoded.locator,
                power_dbm: r.decoded.power_dbm,
                message_type: r.decoded.message_type,
                drift_hz: r.drift_hz,
            }),
        })
        .collect()
    }
}
struct Worker {
    decoder: Box<dyn AudioDecoder>,
    buffer: Vec<f32>,
    origin: u64,
    recent: VecDeque<Decode>,
}
/// Fan audio chunks out to independent protocol windows. Audio history is bounded
/// by each decoder's window length. Decoding is synchronous, providing backpressure.
pub struct AudioStream {
    workers: Vec<Worker>,
    finished: bool,
}
impl AudioStream {
    pub fn new(decoders: Vec<Box<dyn AudioDecoder>>) -> Result<Self, Box<dyn Error>> {
        if decoders.is_empty() {
            return Err("at least one decoder is required".into());
        }
        for d in &decoders {
            if d.minimum_samples() == 0
                || d.minimum_samples() > d.window_samples()
                || d.hop_samples() == 0
                || d.hop_samples() > d.window_samples()
            {
                return Err("invalid decoder window sizes".into());
            }
        }
        Ok(Self {
            workers: decoders
                .into_iter()
                .map(|decoder| Worker {
                    decoder,
                    buffer: Vec::new(),
                    origin: 0,
                    recent: VecDeque::new(),
                })
                .collect(),
            finished: false,
        })
    }
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Decode>, Box<dyn Error>> {
        if self.finished {
            return Err("cannot push audio after finish".into());
        }
        if samples.iter().any(|s| !s.is_finite()) {
            return Err("audio contains non-finite samples".into());
        }
        let mut results = Vec::new();
        // Bound intermediate memory even when a caller supplies a very large chunk.
        for chunk in samples.chunks(4096) {
            for worker in &mut self.workers {
                worker.buffer.extend_from_slice(chunk);
                let window = worker.decoder.window_samples();
                while worker.buffer.len() >= window {
                    let decoded = worker.decoder.decode(&worker.buffer[..window]);
                    worker.collect(decoded, &mut results);
                    let hop = worker.decoder.hop_samples();
                    worker.buffer.drain(..hop);
                    worker.origin += hop as u64;
                }
            }
        }
        sort(&mut results);
        Ok(results)
    }
    /// Attempt the most recent audio without advancing normal overlap windows.
    /// Callers may schedule an early attempt using their own clock. Results share
    /// decoder context and deduplication with normal windows and the final flush.
    /// Workers with less than their minimum length are skipped.
    pub fn decode_pending(&mut self, maximum_samples: usize) -> Vec<Decode> {
        if self.finished {
            return Vec::new();
        }
        let mut results = Vec::new();
        for worker in &mut self.workers {
            let length = worker.buffer.len().min(maximum_samples);
            if length >= worker.decoder.minimum_samples() {
                let offset = worker.buffer.len() - length;
                let decoded = worker.decoder.decode(&worker.buffer[offset..]);
                worker.collect_at(decoded, offset, &mut results);
            }
        }
        sort(&mut results);
        results
    }

    /// Flush a final partial window once; repeated calls are harmless.
    pub fn finish(&mut self) -> Vec<Decode> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut results = Vec::new();
        for worker in &mut self.workers {
            if worker.buffer.len() >= worker.decoder.minimum_samples() {
                let decoded = worker.decoder.decode(&worker.buffer);
                worker.collect(decoded, &mut results);
            }
            worker.buffer.clear();
        }
        sort(&mut results);
        results
    }
}
impl Worker {
    fn collect(&mut self, decoded: Vec<Decode>, output: &mut Vec<Decode>) {
        self.collect_at(decoded, 0, output);
    }
    fn collect_at(&mut self, decoded: Vec<Decode>, offset: usize, output: &mut Vec<Decode>) {
        let retained_origin = self.origin as f64 / SAMPLE_RATE as f64;
        let origin = retained_origin + offset as f64 / SAMPLE_RATE as f64;
        while self
            .recent
            .front()
            .is_some_and(|r| r.start_seconds < retained_origin - 10.0)
        {
            self.recent.pop_front();
        }
        for mut r in decoded {
            r.start_seconds += origin;
            let tolerance = if r.mode == Mode::Wspr { 2.0 } else { 0.35 };
            if self.recent.iter().any(|old| {
                old.payload == r.payload
                    && (old.start_seconds - r.start_seconds).abs() < tolerance
                    && (old.frequency_hz - r.frequency_hz).abs() < 4.0
            }) {
                continue;
            }
            self.recent.push_back(r.clone());
            output.push(r);
        }
    }
}
fn sort(results: &mut [Decode]) {
    results.sort_by(|a, b| {
        a.start_seconds
            .total_cmp(&b.start_seconds)
            .then(a.frequency_hz.total_cmp(&b.frequency_hz))
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    struct MarkerDecoder;
    impl AudioDecoder for MarkerDecoder {
        fn mode(&self) -> Mode {
            Mode::Ft8
        }
        fn minimum_samples(&self) -> usize {
            12000
        }
        fn window_samples(&self) -> usize {
            24000
        }
        fn hop_samples(&self) -> usize {
            12000
        }
        fn decode(&mut self, audio: &[f32]) -> Vec<Decode> {
            audio
                .iter()
                .enumerate()
                .filter(|(_, v)| **v > 0.0)
                .map(|(i, v)| Decode {
                    mode: Mode::Ft8,
                    start_seconds: i as f64 / 12000.0,
                    frequency_hz: *v,
                    snr_db: 0.0,
                    message: "marker".into(),
                    payload: "1".into(),
                    frequency_reference: "lowest_tone",
                    wspr: None,
                })
                .collect()
        }
    }
    #[test]
    fn arbitrary_chunks_preserve_timestamps_and_remove_overlap_duplicates() {
        let mut audio = vec![0.0; 72000];
        for (index, frequency) in [
            (1000, 100.0),
            (18000, 200.0),
            (30000, 300.0),
            (69000, 400.0),
        ] {
            audio[index] = frequency;
        }
        let mut stream = AudioStream::new(vec![Box::new(MarkerDecoder)]).unwrap();
        let mut output = Vec::new();
        for chunk in audio.chunks(731) {
            output.extend(stream.push(chunk).unwrap());
            assert!(stream.workers[0].buffer.len() < 24000);
        }
        output.extend(stream.finish());
        assert_eq!(output.len(), 4);
        for (r, index) in output.iter().zip([1000, 18000, 30000, 69000]) {
            assert!((r.start_seconds - index as f64 / 12000.0).abs() < 1e-10);
        }
        assert!(stream.finish().is_empty());
        assert!(stream.push(&[0.0]).is_err());
    }
    #[test]
    fn early_attempt_retains_audio_context_origins_and_deduplication() {
        let mut stream = AudioStream::new(vec![Box::new(MarkerDecoder)]).unwrap();
        let mut audio = vec![0.0; 18000];
        audio[9000] = 100.0;
        audio[17000] = 200.0;
        assert!(stream.push(&audio).unwrap().is_empty());
        assert!(stream.decode_pending(100).is_empty());
        let early = stream.decode_pending(12000);
        assert_eq!(early.len(), 2);
        assert_eq!(early[0].start_seconds, 0.75);
        assert!((early[1].start_seconds - 17000.0 / 12000.0).abs() < 1e-10);
        assert_eq!(stream.workers[0].buffer.len(), audio.len());
        assert!(stream.decode_pending(12000).is_empty());
        assert!(stream.push(&vec![0.0; 12000]).unwrap().is_empty());
        assert!(stream.finish().is_empty());
        assert!(stream.decode_pending(12000).is_empty());
    }

    #[test]
    fn invalid_audio_and_settings_are_rejected() {
        let mut stream = AudioStream::new(vec![Box::new(MarkerDecoder)]).unwrap();
        assert!(stream.push(&[f32::NAN]).is_err());
        assert!(
            WsprDecoder::new(Settings {
                low_hz: 1000.0,
                high_hz: 2000.0,
                passes: 1
            })
            .is_err()
        );
        assert!(
            Ft8Decoder::new(Settings {
                low_hz: f32::NAN,
                high_hz: 2000.0,
                passes: 1
            })
            .is_err()
        );
    }
}
