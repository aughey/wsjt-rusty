//! General signal processing primitives, independent of file formats and modes.

/// Low-pass resample real samples to an equal or lower sample rate.
///
/// Uses a windowed-sinc filter with a cutoff at 90% of the output Nyquist
/// frequency. At boundaries, only available input samples contribute and the
/// filter weights are normalized. The output length is rounded down.
/// Rates must be nonzero; upsampling is not supported.
pub fn resample_down(
    samples: &[f32],
    input_rate: u32,
    output_rate: u32,
) -> Result<Vec<f32>, &'static str> {
    if output_rate == 0 || input_rate < output_rate {
        return Err("sample rates must be nonzero and output rate must not exceed input rate");
    }
    if input_rate == output_rate {
        return Ok(samples.to_vec());
    }
    // Windowed sinc; support arbitrary input rates with anti-alias filtering.
    let ratio = input_rate as f64 / output_rate as f64;
    let radius = (32.0 * ratio).ceil() as isize;
    let cutoff = 0.45 / ratio;
    let mut out = Vec::with_capacity((samples.len() as f64 / ratio) as usize);
    for i in 0..(samples.len() as f64 / ratio) as usize {
        let center = i as f64 * ratio;
        let mut sum = 0.0;
        let mut weight = 0.0;
        for j in center.floor() as isize - radius..=center.floor() as isize + radius {
            if j < 0 || j >= samples.len() as isize {
                continue;
            }
            let x = j as f64 - center;
            let w = sinc_weight(x, radius as f64, cutoff);
            sum += samples[j as usize] as f64 * w;
            weight += w;
        }
        out.push((sum / weight) as f32);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::resample_down;

    #[test]
    fn fractional_rate_preserves_dc_and_validates_rates() {
        let input = vec![0.375; 4410];
        let output = resample_down(&input, 44100, 16000).unwrap();
        assert_eq!(output.len(), 1600);
        assert!(output.iter().all(|&x| (x - 0.375).abs() < 1e-6));
        assert_eq!(resample_down(&input, 16000, 16000).unwrap(), input);
        assert!(resample_down(&[], 44100, 16000).unwrap().is_empty());
        for (input_rate, output_rate) in [(0, 0), (0, 12000), (12000, 0), (12000, 48000)] {
            assert!(resample_down(&input, input_rate, output_rate).is_err());
        }
    }
}

/// Stateful version of [`resample_down`], retaining filter history and fractional
/// phase across arbitrary chunks. Output waits for the filter's right-hand support
/// (about 32 output samples); `finish` supplies the final boundary samples.
/// Input history is bounded by the filter support, independent of stream duration.
pub struct StreamingDownsampler {
    input_rate: u32,
    output_rate: u32,
    buffer: std::collections::VecDeque<f32>,
    origin: u64,
    received: u64,
    next: u64,
    finished: bool,
}

impl StreamingDownsampler {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self, &'static str> {
        if output_rate == 0 || input_rate < output_rate {
            return Err("sample rates must be nonzero and output rate must not exceed input rate");
        }
        Ok(Self {
            input_rate,
            output_rate,
            buffer: Default::default(),
            origin: 0,
            received: 0,
            next: 0,
            finished: false,
        })
    }

    pub fn push(&mut self, samples: &[f32]) -> Result<Vec<f32>, &'static str> {
        if self.finished {
            return Err("cannot resample after finish");
        }
        if self.input_rate == self.output_rate {
            self.received += samples.len() as u64;
            self.next = self.received;
            return Ok(samples.to_vec());
        }
        let mut output = Vec::new();
        for chunk in samples.chunks(4096) {
            self.buffer.extend(chunk);
            self.received += chunk.len() as u64;
            self.produce(false, &mut output);
        }
        Ok(output)
    }

    pub fn finish(&mut self) -> Vec<f32> {
        if self.finished {
            return Vec::new();
        }
        self.finished = true;
        let mut output = Vec::new();
        if self.input_rate != self.output_rate {
            self.produce(true, &mut output);
        }
        self.buffer.clear();
        output
    }

    fn produce(&mut self, finishing: bool, output: &mut Vec<f32>) {
        let ratio = self.input_rate as f64 / self.output_rate as f64;
        let radius = (32.0 * ratio).ceil() as i64;
        let cutoff = 0.45 / ratio;
        let count = self.received * u64::from(self.output_rate) / u64::from(self.input_rate);
        while self.next < count {
            let center = self.next as f64 * ratio;
            if !finishing && center.floor() as i64 + radius >= self.received as i64 {
                break;
            }
            let mut sum = 0.0;
            let mut weight = 0.0;
            for j in center.floor() as i64 - radius..=center.floor() as i64 + radius {
                if j < 0 || j >= self.received as i64 {
                    continue;
                }
                let w = sinc_weight(j as f64 - center, radius as f64, cutoff);
                sum += self.buffer[(j as u64 - self.origin) as usize] as f64 * w;
                weight += w;
            }
            output.push((sum / weight) as f32);
            self.next += 1;
        }
        let retain_from = ((self.next as f64 * ratio).floor() as i64 - radius).max(0) as u64;
        let discard = retain_from
            .saturating_sub(self.origin)
            .min(self.buffer.len() as u64);
        self.buffer.drain(..discard as usize);
        self.origin += discard;
    }
}

fn sinc_weight(x: f64, radius: f64, cutoff: f64) -> f64 {
    let z = 2.0 * std::f64::consts::PI * cutoff * x;
    let sinc = if z.abs() < 1e-10 { 1.0 } else { z.sin() / z };
    let window = 0.5 + 0.5 * (std::f64::consts::PI * x / (radius + 1.0)).cos();
    sinc * window
}

#[cfg(test)]
mod streaming_tests {
    use super::*;

    #[test]
    fn arbitrary_chunks_match_batch_filter_and_keep_bounded_history() {
        for rate in [12000, 44100, 48000] {
            let input: Vec<_> = (0..rate / 2 + 37)
                .map(|i| {
                    let t = i as f32 / rate as f32;
                    (2.0 * std::f32::consts::PI * 1000.0 * t).sin()
                        + 0.3 * (2.0 * std::f32::consts::PI * 8000.0 * t).sin()
                })
                .collect();
            let expected = resample_down(&input, rate, 12000).unwrap();
            for chunk_size in [1, 127, 4096, 50000] {
                let mut resampler = StreamingDownsampler::new(rate, 12000).unwrap();
                let mut actual = Vec::new();
                for chunk in input.chunks(chunk_size) {
                    actual.extend(resampler.push(chunk).unwrap());
                    assert!(resampler.buffer.len() < 300);
                }
                actual.extend(resampler.finish());
                assert_eq!(actual.len(), expected.len());
                assert!(
                    actual
                        .iter()
                        .zip(&expected)
                        .all(|(a, b)| (a - b).abs() < 1e-6)
                );
                assert!(resampler.finish().is_empty());
                assert!(resampler.push(&[0.0]).is_err());
            }
        }
        assert!(StreamingDownsampler::new(8000, 12000).is_err());
    }
}
