//! Browser-session timing around the existing transport-independent decoder.
use serde::Serialize;
use wsjt_dsp::StreamingDownsampler;
use wsjt_rusty::decoder::{AudioStream, Decode, Ft8Decoder, Mode, SAMPLE_RATE, Settings};

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
    pub decodes: Vec<TimedDecode>,
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
}

impl Receiver {
    pub fn new(rate: u32, start_utc_ms: f64) -> Result<Self, String> {
        if !start_utc_ms.is_finite() {
            return Err("invalid capture clock".into());
        }
        Ok(Self {
            stream: AudioStream::new(vec![Box::new(
                Ft8Decoder::new(Settings::defaults(Mode::Ft8)).map_err(|e| e.to_string())?,
            )])
            .map_err(|e| e.to_string())?,
            resampler: StreamingDownsampler::new(rate, SAMPLE_RATE as u32)?,
            rate,
            start_utc_ms,
            received: 0,
            finished: false,
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
        let decodes = self.stream.push(&mono).map_err(|e| e.to_string())?;
        self.received += samples.len() as u64;
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let start = 1_700_000_000_123.0;
        let mut receiver = Receiver::new(12000, start).unwrap();
        let mut actual = Vec::new();
        let mut offset = 0;
        for chunk in audio.chunks(731) {
            actual.extend(receiver.push(offset, chunk).unwrap().decodes);
            offset += chunk.len() as u64;
        }
        assert!(!actual.is_empty(), "must decode before finish");
        actual.extend(receiver.finish().unwrap().decodes);
        assert_eq!(actual.len(), oracle.len());
        for (a, b) in actual.iter().zip(oracle) {
            assert_eq!(a.decode.payload, b.payload);
            assert_eq!(a.decode.message, b.message);
            assert!((a.start_utc_ms - (start + b.start_seconds * 1000.0)).abs() < 0.001);
            assert!((a.end_utc_ms - a.start_utc_ms - 12640.0).abs() < 0.001);
        }
    }
}
