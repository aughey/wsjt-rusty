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
    /// Optional early delivery. Results are window-relative until AudioStream adjusts them.
    fn decode_with_observer(
        &mut self,
        audio: &[f32],
        observer: &mut dyn FnMut(&Decode),
    ) -> Vec<Decode> {
        let results = self.decode(audio);
        for result in &results {
            observer(result);
        }
        results
    }
    /// Optional candidate priority, in the mode's audio-frequency convention.
    /// Implementations must retain their normal search range and passes.
    fn set_priority_frequencies(&mut self, _frequencies: &[f32]) {}
}
pub struct Ft8Decoder {
    settings: Settings,
    messages: wsjt_ft8::message::Messages,
    priorities: Vec<f32>,
}
impl Ft8Decoder {
    pub fn new(settings: Settings) -> Result<Self, Box<dyn Error>> {
        settings.validate(Mode::Ft8)?;
        Ok(Self {
            settings,
            messages: Default::default(),
            priorities: Vec::new(),
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
        wsjt_ft8::decode_with_context_and_observer(
            audio,
            self.settings.low_hz,
            self.settings.high_hz,
            self.settings.passes,
            &mut self.messages,
            &self.priorities,
            &mut |_| {},
        )
        .into_iter()
        .map(ft8_result)
        .collect()
    }
    fn decode_with_observer(
        &mut self,
        audio: &[f32],
        observer: &mut dyn FnMut(&Decode),
    ) -> Vec<Decode> {
        wsjt_ft8::decode_with_context_and_observer(
            audio,
            self.settings.low_hz,
            self.settings.high_hz,
            self.settings.passes,
            &mut self.messages,
            &self.priorities,
            &mut |r| observer(&ft8_result(r.clone())),
        )
        .into_iter()
        .map(ft8_result)
        .collect()
    }
    fn set_priority_frequencies(&mut self, frequencies: &[f32]) {
        self.priorities = frequencies
            .iter()
            .copied()
            .filter(|f| f.is_finite() && *f >= self.settings.low_hz && *f <= self.settings.high_hz)
            .take(100)
            .collect();
    }
}
fn ft8_result(r: wsjt_ft8::Decode) -> Decode {
    Decode {
        mode: Mode::Ft8,
        start_seconds: r.start_seconds as f64,
        frequency_hz: r.frequency_hz,
        frequency_reference: "lowest_tone",
        snr_db: r.snr_db,
        message: r.message,
        payload: r.payload,
        wspr: None,
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
    pub fn set_priority_frequencies(&mut self, frequencies: &[f32]) {
        for worker in &mut self.workers {
            worker.decoder.set_priority_frequencies(frequencies);
        }
    }
    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<Decode>, Box<dyn Error>> {
        self.push_impl(samples, false, &mut |_| {}, true)
    }
    /// Early callbacks use stream-relative times and normal overlap deduplication.
    /// The batch contains these same emissions, sorted, without rewriting hash text.
    pub fn push_with_observer(
        &mut self,
        samples: &[f32],
        observer: &mut dyn FnMut(&Decode),
    ) -> Result<Vec<Decode>, Box<dyn Error>> {
        self.push_impl(samples, true, observer, true)
    }
    /// Retain normal bounded/overlapping history without running automatic windows.
    /// The caller must schedule decode_pending/finish or preserve recovery in a
    /// separate receiver. Regular push behavior and coverage remain unchanged.
    pub fn push_buffered(&mut self, samples: &[f32]) -> Result<(), Box<dyn Error>> {
        self.push_impl(samples, false, &mut |_| {}, false)
            .map(|_| ())
    }
    fn push_impl(
        &mut self,
        samples: &[f32],
        incremental: bool,
        observer: &mut dyn FnMut(&Decode),
        automatic: bool,
    ) -> Result<Vec<Decode>, Box<dyn Error>> {
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
                    if automatic {
                        worker.decode_window(0, window, incremental, &mut results, observer);
                    }
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
        self.pending_impl(maximum_samples, false, &mut |_| {})
    }
    pub fn decode_pending_with_observer(
        &mut self,
        maximum_samples: usize,
        observer: &mut dyn FnMut(&Decode),
    ) -> Vec<Decode> {
        self.pending_impl(maximum_samples, true, observer)
    }
    fn pending_impl(
        &mut self,
        maximum_samples: usize,
        incremental: bool,
        observer: &mut dyn FnMut(&Decode),
    ) -> Vec<Decode> {
        if self.finished {
            return Vec::new();
        }
        let mut results = Vec::new();
        for worker in &mut self.workers {
            let length = worker.buffer.len().min(maximum_samples);
            if length >= worker.decoder.minimum_samples() {
                let offset = worker.buffer.len() - length;
                worker.decode_window(offset, length, incremental, &mut results, observer);
            }
        }
        sort(&mut results);
        results
    }

    /// Flush a final partial window once; repeated calls are harmless.
    pub fn finish(&mut self) -> Vec<Decode> {
        self.finish_impl(false, &mut |_| {})
    }
    pub fn finish_with_observer(&mut self, observer: &mut dyn FnMut(&Decode)) -> Vec<Decode> {
        self.finish_impl(true, observer)
    }
    fn finish_impl(&mut self, incremental: bool, observer: &mut dyn FnMut(&Decode)) -> Vec<Decode> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut results = Vec::new();
        for worker in &mut self.workers {
            if worker.buffer.len() >= worker.decoder.minimum_samples() {
                worker.decode_window(0, worker.buffer.len(), incremental, &mut results, observer);
            }
            worker.buffer.clear();
        }
        sort(&mut results);
        results
    }
}
impl Worker {
    fn decode_window(
        &mut self,
        offset: usize,
        length: usize,
        incremental: bool,
        output: &mut Vec<Decode>,
        observer: &mut dyn FnMut(&Decode),
    ) {
        let retained_origin = self.origin as f64 / SAMPLE_RATE as f64;
        let origin = retained_origin + offset as f64 / SAMPLE_RATE as f64;
        self.recent
            .retain(|r| r.start_seconds >= retained_origin - 10.0);
        let recent = &mut self.recent;
        let mut collect = |r: &Decode| {
            let mut r = r.clone();
            r.start_seconds += origin;
            let tolerance = if r.mode == Mode::Wspr { 2.0 } else { 0.35 };
            if recent.iter().any(|old| {
                old.payload == r.payload
                    && (old.start_seconds - r.start_seconds).abs() < tolerance
                    && (old.frequency_hz - r.frequency_hz).abs() < 4.0
            }) {
                return;
            }
            recent.push_back(r.clone());
            if incremental {
                observer(&r);
            }
            output.push(r);
        };
        let decoded = if incremental {
            self.decoder
                .decode_with_observer(&self.buffer[offset..offset + length], &mut collect)
        } else {
            self.decoder.decode(&self.buffer[offset..offset + length])
        };
        // Also accept returned results from adapters which do not notify every result.
        for r in &decoded {
            collect(r);
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
    fn buffered_ingestion_bounds_history_and_preserves_early_origin() {
        let mut stream = AudioStream::new(vec![Box::new(MarkerDecoder)]).unwrap();
        stream.push_buffered(&vec![0.0; 120 * 12000]).unwrap();
        assert!(stream.workers[0].buffer.len() < 24000);
        assert_eq!(stream.workers[0].origin, 119 * 12000);
        let mut tail = vec![0.0; 12000];
        tail[6000] = 1200.0;
        stream.push_buffered(&tail).unwrap();
        let decoded = stream.decode_pending(24000);
        assert_eq!(decoded.len(), 1);
        assert!((decoded[0].start_seconds - 120.5).abs() < 1e-10);
        assert!(stream.finish().is_empty());
        assert!(stream.push_buffered(&tail).is_err());
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
