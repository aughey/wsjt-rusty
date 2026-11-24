//! RustFFT visualization of audio snapshots. This is not a decoder input path.
use rustfft::{Fft, FftPlanner, num_complex::Complex32};
use std::sync::Arc;

pub const FFT_SIZE: usize = 16384;
pub const DISPLAY_RANGE_DB: f32 = 50.0;

/// Absolute time-domain peak, independent of FFT normalization and passband.
pub fn input_peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .copied()
        .filter(|s| s.is_finite())
        .map(f32::abs)
        .fold(0.0, f32::max)
}

/// Robust display leveling: signals occupying a minority of the passband do
/// not set the background. Reuses its scratch space; never alters decoder PCM.
#[derive(Default)]
pub struct LevelScale {
    scratch: Vec<f32>,
    background: Option<f32>,
}

impl LevelScale {
    pub fn floor(&mut self, levels: &[f32], manual: f32, automatic: bool) -> f32 {
        if !automatic || levels.is_empty() {
            self.background = None;
            return manual;
        }
        self.scratch.clear();
        self.scratch
            .extend(levels.iter().copied().filter(|v| v.is_finite()));
        if self.scratch.is_empty() {
            return manual;
        }
        let middle = self.scratch.len() / 2;
        let (_, &mut median, _) = self.scratch.select_nth_unstable_by(middle, f32::total_cmp);
        // Ignore padded startup silence; acquire the first real noise floor at once.
        if median > -150.0 {
            self.background = Some(match self.background {
                Some(old) => old + (median - old) * if median > old { 0.25 } else { 0.03 },
                None => median,
            });
        }
        self.background
            .map_or(manual, |noise| (noise - 8.0).clamp(-140.0, -10.0))
    }
}

/// Hann-windowed amplitude spectrum with coherent-gain normalization. A
/// bin-centered full-scale sine reads 0 dBFS; these are FFT bin levels, not SNR.
pub struct Spectrum {
    fft: Arc<dyn Fft<f32>>,
    scratch: Vec<Complex32>,
    buffer: Vec<Complex32>,
    window: Vec<f32>,
    scale: f32,
    bins: Vec<(usize, usize)>,
}

impl Spectrum {
    pub fn new(rate: f32, low_hz: f32, high_hz: f32, width: usize) -> Result<Self, &'static str> {
        if !rate.is_finite()
            || !low_hz.is_finite()
            || !high_hz.is_finite()
            || low_hz <= 0.0
            || high_hz <= low_hz
            || high_hz > rate / 2.0
            || width == 0
        {
            return Err("invalid spectrum passband or sample rate");
        }
        let fft = FftPlanner::<f32>::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let scale = (2.0 / window.iter().sum::<f32>()).powi(2);
        let bin_hz = rate / FFT_SIZE as f32;
        // Pool peaks over each pixel's bin interval so a narrow signal is not
        // lost when the screen has fewer columns than the FFT has bins.
        let bins = (0..width)
            .map(|x| {
                let left = low_hz + (high_hz - low_hz) * x as f32 / width as f32;
                let right = low_hz + (high_hz - low_hz) * (x + 1) as f32 / width as f32;
                let first = ((left / bin_hz).floor() as usize).max(1);
                let last = ((right / bin_hz).ceil() as usize).min(FFT_SIZE / 2);
                (first, last)
            })
            .collect();
        Ok(Self {
            scratch: vec![Complex32::default(); fft.get_inplace_scratch_len()],
            fft,
            buffer: vec![Complex32::default(); FFT_SIZE],
            window,
            scale,
            bins,
        })
    }

    /// Returns one level per horizontal pixel, with no allocations per frame.
    pub fn levels(&mut self, samples: &[f32], output: &mut [f32]) -> Result<(), &'static str> {
        if samples.len() != FFT_SIZE || output.len() != self.bins.len() {
            return Err("spectrum input or output has the wrong length");
        }
        for ((value, &sample), &weight) in self.buffer.iter_mut().zip(samples).zip(&self.window) {
            *value = Complex32::new(
                if sample.is_finite() {
                    sample * weight
                } else {
                    0.0
                },
                0.0,
            );
        }
        self.fft
            .process_with_scratch(&mut self.buffer, &mut self.scratch);
        for (value, &(first, last)) in output.iter_mut().zip(&self.bins) {
            let peak = self.buffer[first..=last]
                .iter()
                .map(|v| v.norm_sqr())
                .fold(0.0_f32, f32::max);
            *value = 10.0 * (peak * self.scale).max(1e-16).log10();
        }
        Ok(())
    }
}

/// Dark navy background with green/yellow signals and pale yellow peaks.
pub fn color(db: f32, floor: f32) -> [u8; 4] {
    const STOPS: [[f32; 3]; 5] = [
        [11.0, 19.0, 37.0],
        [25.0, 67.0, 80.0],
        [126.0, 157.0, 51.0],
        [241.0, 210.0, 58.0],
        [255.0, 250.0, 207.0],
    ];
    let position = ((db - floor) / DISPLAY_RANGE_DB).clamp(0.0, 1.0) * 4.0;
    let i = (position.floor() as usize).min(3);
    let fraction = position - i as f32;
    let mut out = [0, 0, 0, 255];
    for (channel, value) in out[..3].iter_mut().enumerate() {
        *value = (STOPS[i][channel] + (STOPS[i + 1][channel] - STOPS[i][channel]) * fraction)
            .round() as u8;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_peak_includes_negative_full_scale_samples_without_fft_scaling() {
        assert_eq!(input_peak(&[]), 0.0);
        assert_eq!(input_peak(&[0.0; 16]), 0.0);
        assert_eq!(input_peak(&[0.1, -0.75, 0.5, f32::NAN]), 0.75);
        assert_eq!(input_peak(&[-1.0, 0.1]), 1.0);
        assert_eq!(input_peak(&[1.25]), 1.25);
    }

    #[test]
    fn automatic_floor_follows_background_without_being_set_by_narrow_signals() {
        let mut scale = LevelScale::default();
        assert_eq!(scale.floor(&[-160.0; 100], -100.0, true), -100.0);
        let mut spectrum = vec![-45.0; 100];
        spectrum[..20].fill(-10.0);
        assert_eq!(scale.floor(&spectrum, -100.0, true), -53.0);
        assert_eq!(scale.floor(&spectrum, -80.0, false), -80.0);
        spectrum.fill(-75.0);
        assert_eq!(scale.floor(&spectrum, -100.0, true), -83.0);
    }

    #[test]
    fn tones_map_to_audio_frequency_and_calibrated_bin_levels_at_device_rates() {
        for rate in [12000.0, 44100.0, 48000.0, 96000.0] {
            let mut spectrum = Spectrum::new(rate, 100.0, 4000.0, 1200).unwrap();
            for target in [110.0, 1000.0, 3990.0] {
                let bin = (target * FFT_SIZE as f32 / rate).round();
                let frequency = bin * rate / FFT_SIZE as f32;
                let samples: Vec<f32> = (0..FFT_SIZE)
                    .map(|i| 0.5 * (std::f32::consts::TAU * bin * i as f32 / FFT_SIZE as f32).sin())
                    .collect();
                let mut levels = vec![0.0; 1200];
                spectrum.levels(&samples, &mut levels).unwrap();
                let (x, &peak) = levels
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(b.1))
                    .unwrap();
                let display_hz = 100.0 + (x as f32 + 0.5) * 3900.0 / 1200.0;
                assert!((display_hz - frequency).abs() < rate / FFT_SIZE as f32 + 4.0);
                assert!((peak - 20.0 * 0.5_f32.log10()).abs() < 0.01);
            }
        }
    }

    #[test]
    fn silence_and_out_of_band_tone_do_not_light_the_passband() {
        let mut spectrum = Spectrum::new(48000.0, 100.0, 4000.0, 1200).unwrap();
        let mut levels = vec![0.0; 1200];
        spectrum.levels(&vec![0.0; FFT_SIZE], &mut levels).unwrap();
        assert!(levels.iter().all(|&db| db <= -120.0));
        let input: Vec<f32> = (0..FFT_SIZE)
            .map(|i| (std::f32::consts::TAU * 2048.0 * i as f32 / FFT_SIZE as f32).sin())
            .collect();
        spectrum.levels(&input, &mut levels).unwrap(); // bin 2048 = 6000 Hz
        assert!(levels.iter().all(|&db| db < -90.0));
        assert!(spectrum.levels(&[], &mut levels).is_err());
        assert!(Spectrum::new(8000.0, 100.0, 4100.0, 1200).is_err());
    }
}

/// Keep FFT snapshots containing our own transmit audio out of display/history.
#[derive(Default)]
pub struct ReceiveSnapshotGate {
    receive_after_ms: f64,
}
impl ReceiveSnapshotGate {
    pub fn allow(&mut self, audio_ms: f64, window_ms: f64, transmitting: bool) -> bool {
        if transmitting {
            self.receive_after_ms = audio_ms + window_ms;
            false
        } else {
            audio_ms >= self.receive_after_ms
        }
    }
}

#[cfg(test)]
mod transmit_gate_tests {
    use super::*;
    #[test]
    fn transmit_rows_and_fft_tails_are_blank_until_receive_only_snapshot() {
        let mut gate = ReceiveSnapshotGate::default();
        assert!(gate.allow(1000.0, 350.0, false));
        assert!(!gate.allow(1100.0, 350.0, true));
        assert!(!gate.allow(2000.0, 350.0, true));
        assert!(!gate.allow(2200.0, 350.0, false));
        assert!(gate.allow(2350.0, 350.0, false));
    }
}
