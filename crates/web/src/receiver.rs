//! Browser-session timing around the existing transport-independent decoder.
use serde::Serialize;
use wsjt_dsp::StreamingDownsampler;
use wsjt_rusty::decoder::{
    AudioDecoder, AudioStream, Decode, Ft8Decoder, Mode, SAMPLE_RATE, Settings,
};

/// FT8 has 79 symbols of 160 ms, including its synchronization symbols.
pub const FT8_DURATION_SECONDS: f64 = 79.0 * 0.160;

#[derive(Serialize)]
pub struct TimedDecode {
    pub start_utc_ms: f64,
    pub end_utc_ms: f64,
    #[serde(flatten)]
    pub decode: Decode,
}

#[derive(Serialize)]
pub struct Update {
    pub capture_start_ms: f64,
    pub capture_end_ms: f64,
    pub received_seconds: f64,
    pub finished: bool,
    pub next_attempt_utc_ms: f64,
    pub decodes: Vec<TimedDecode>,
}

// Observe the actual decoder call, including early, recovery and EOF attempts.
// The callback is transport-owned; the receive libraries remain browser-free.
struct ReportingDecoder {
    decoder: Ft8Decoder,
    activity: fn(bool),
}

impl AudioDecoder for ReportingDecoder {
    fn mode(&self) -> Mode {
        self.decoder.mode()
    }
    fn minimum_samples(&self) -> usize {
        self.decoder.minimum_samples()
    }
    fn window_samples(&self) -> usize {
        self.decoder.window_samples()
    }
    fn hop_samples(&self) -> usize {
        self.decoder.hop_samples()
    }
    fn decode(&mut self, audio: &[f32]) -> Vec<Decode> {
        (self.activity)(true);
        let result = self.decoder.decode(audio);
        (self.activity)(false);
        result
    }
}

/// Sample counts provide elapsed time; UTC is anchored once at the first sample.
/// Wall-clock changes during capture do not move already captured samples.
pub struct Receiver {
    stream: AudioStream,
    resampler: StreamingDownsampler,
    rate: u32,
    start_utc_ms: f64,
    received: u64,
    finished: bool,
    next_attempt_utc_ms: f64,
}

impl Receiver {
    pub fn new(rate: u32, start_utc_ms: f64) -> Result<Self, String> {
        Self::with_decode_activity(rate, start_utc_ms, |_| {})
    }

    pub fn with_decode_activity(
        rate: u32,
        start_utc_ms: f64,
        activity: fn(bool),
    ) -> Result<Self, String> {
        if !start_utc_ms.is_finite() {
            return Err("invalid capture clock".into());
        }
        Ok(Self {
            stream: AudioStream::new(vec![Box::new(ReportingDecoder {
                decoder: Ft8Decoder::new(Settings::defaults(Mode::Ft8))
                    .map_err(|e| e.to_string())?,
                activity,
            })])
            .map_err(|e| e.to_string())?,
            resampler: StreamingDownsampler::new(rate, SAMPLE_RATE as u32)?,
            rate,
            start_utc_ms,
            received: 0,
            finished: false,
            next_attempt_utc_ms: next_attempt(start_utc_ms),
        })
    }

    pub fn push(&mut self, offset: u64, samples: &[f32]) -> Result<Update, String> {
        if self.finished {
            return Err("capture has already finished".into());
        }
        if offset != self.received {
            return Err("audio discontinuity: refusing to join samples across a gap".into());
        }
        if samples.iter().any(|s| !s.is_finite()) {
            return Err("audio contains non-finite samples".into());
        }
        let mono = self.resampler.push(samples)?;
        let mut decodes = self.stream.push(&mono).map_err(|e| e.to_string())?;
        self.received += samples.len() as u64;
        let end_ms = self.start_utc_ms + self.received as f64 / f64::from(self.rate) * 1000.0;
        if end_ms >= self.next_attempt_utc_ms {
            // Include timing margin around the nominal 0.5–13.14 s transmission.
            // The regular 30-second windows still recover off-clock signals.
            decodes.extend(self.stream.decode_pending(16 * SAMPLE_RATE));
            self.next_attempt_utc_ms = next_attempt(end_ms);
        }
        Ok(self.update(decodes))
    }

    pub fn finish(&mut self) -> Result<Update, String> {
        if self.finished {
            return Ok(self.update(Vec::new()));
        }
        let tail = self.resampler.finish();
        let mut decodes = self.stream.push(&tail).map_err(|e| e.to_string())?;
        decodes.extend(self.stream.finish());
        self.finished = true;
        Ok(self.update(decodes))
    }

    fn update(&self, decodes: Vec<Decode>) -> Update {
        let received_seconds = self.received as f64 / f64::from(self.rate);
        Update {
            capture_start_ms: self.start_utc_ms,
            capture_end_ms: self.start_utc_ms + received_seconds * 1000.0,
            received_seconds,
            finished: self.finished,
            next_attempt_utc_ms: self.next_attempt_utc_ms,
            decodes: decodes
                .into_iter()
                .map(|decode| {
                    let start_utc_ms = self.start_utc_ms + decode.start_seconds * 1000.0;
                    TimedDecode {
                        start_utc_ms,
                        end_utc_ms: start_utc_ms + FT8_DURATION_SECONDS * 1000.0,
                        decode,
                    }
                })
                .collect(),
        }
    }
}

/// First slot-end attempt strictly after this sample-clock UTC time.
fn next_attempt(utc_ms: f64) -> f64 {
    ((utc_ms - 13_500.0) / 15_000.0).floor() * 15_000.0 + 28_500.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_brackets_early_recovery_and_stop_attempts() {
        thread_local! { static EVENTS: std::cell::RefCell<Vec<bool>> = const { std::cell::RefCell::new(Vec::new()) }; }
        let mut receiver = Receiver::with_decode_activity(12000, 0.0, |active| {
            EVENTS.with_borrow_mut(|events| events.push(active));
        })
        .unwrap();
        receiver.push(0, &vec![0.0; 12000]).unwrap();
        EVENTS.with_borrow(|events| assert!(events.is_empty()));
        receiver.push(12000, &vec![0.0; 150000]).unwrap();
        EVENTS.with_borrow(|events| assert_eq!(*events, [true, false]));
        receiver.push(162000, &vec![0.0; 198000]).unwrap();
        // The 30-second recovery window and the next early attempt both run.
        EVENTS.with_borrow(|events| assert_eq!(*events, [true, false, true, false, true, false]));
        receiver.finish().unwrap();
        EVENTS.with_borrow(|events| assert_eq!(events.len(), 8));
        receiver.finish().unwrap();
        EVENTS.with_borrow(|events| assert_eq!(events.len(), 8));
    }

    #[test]
    fn slot_attempts_follow_utc_independently_of_capture_start() {
        for time in [0.0, 123.0, 13_499.0, 13_500.0, 14_000.0, 29_000.0] {
            let next = next_attempt(time);
            assert!(next > time && next <= time + 15_000.0);
            assert_eq!(next.rem_euclid(15_000.0), 13_500.0);
        }
    }

    #[test]
    fn clock_and_discontinuity_contract() {
        let mut receiver = Receiver::new(48000, 1_700_000_000_000.0).unwrap();
        let update = receiver.push(0, &vec![0.0; 48001]).unwrap();
        assert_eq!(update.capture_start_ms, 1_700_000_000_000.0);
        assert!((update.received_seconds - 48001.0 / 48000.0).abs() < 1e-10);
        assert!((update.capture_end_ms - update.capture_start_ms - 1000.020833).abs() < 0.001);
        assert!(receiver.push(48002, &[0.0]).is_err());
        assert!(receiver.push(48001, &[f32::NAN]).is_err());
        assert!(receiver.finish().unwrap().finished);
        assert!(receiver.finish().unwrap().decodes.is_empty());
        assert!(receiver.push(48001, &[0.0]).is_err());
        assert!(Receiver::new(8000, 0.0).is_err());
        assert!(Receiver::new(12000, f64::NAN).is_err());
    }

    #[test]
    fn live_fixture_emits_before_stop_and_preserves_payloads_and_utc() {
        let mut audio = wsjt_rusty::audio::read(std::path::Path::new(
            "../../tests/fixtures/ft8-210703-133430.wav",
        ))
        .unwrap();
        audio.resize(30 * SAMPLE_RATE, 0.0);
        let mut expected = AudioStream::new(vec![Box::new(
            Ft8Decoder::new(Settings::defaults(Mode::Ft8)).unwrap(),
        )])
        .unwrap();
        let mut oracle = expected.push(&audio).unwrap();
        oracle.extend(expected.finish());
        assert!(!oracle.is_empty());
        // UTC slot aligned with the recorded FT8 period, plus a nonzero epoch.
        let start = 1_700_000_010_000.0;
        let mut receiver = Receiver::new(12000, start).unwrap();
        let mut actual = Vec::new();
        let mut offset = 0;
        let mut early_count = 0;
        for chunk in audio.chunks(731) {
            actual.extend(receiver.push(offset, chunk).unwrap().decodes);
            offset += chunk.len() as u64;
            if offset < 15 * SAMPLE_RATE as u64 {
                early_count = actual.len();
            }
        }
        assert!(
            early_count > 0,
            "must emit before the next UTC slot, not just before Stop"
        );
        assert!(!actual.is_empty(), "must decode before finish");
        actual.extend(receiver.finish().unwrap().decodes);
        actual.sort_by(|a, b| {
            a.decode
                .start_seconds
                .total_cmp(&b.decode.start_seconds)
                .then(a.decode.frequency_hz.total_cmp(&b.decode.frequency_hz))
        });
        assert_eq!(actual.len(), oracle.len());
        for (a, b) in actual.iter().zip(oracle) {
            assert_eq!(a.decode.payload, b.payload);
            assert_eq!(a.decode.message, b.message);
            assert!((a.start_utc_ms - (start + b.start_seconds * 1000.0)).abs() < 0.001);
            assert!((a.end_utc_ms - a.start_utc_ms - 12640.0).abs() < 0.001);
        }
    }
}
