// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from WSJT-X, Copyright (C) 2001-2023 Joe Taylor, K1JT, and contributors.
//! FT8 receive path ported from WSJT-X sync8, ft8_downsample, sync8d, ft8b.
//! The coarse search is extended over recording time instead of a UTC slot.
pub mod ldpc;
pub mod message;
mod message_tables;
mod tables;
pub mod transmit;
pub mod waveform;

use message::Messages;
use rustfft::Fft;
use rustfft::{FftPlanner, num_complex::Complex32 as C};
use serde::Serialize;
const COSTAS: [usize; 7] = [3, 1, 4, 0, 6, 5, 2];
const GRAY: [usize; 8] = [0, 1, 3, 2, 5, 6, 4, 7];

const LONG_FFT_SIZE: usize = 192000;
// Covers candidate-start seconds in a 30-second receive window, while keeping
// batch callers with longer recordings bounded (about 47 MiB at capacity).
const PRIORITY_SPECTRA: usize = 32;

struct Spectra {
    entries: Vec<(isize, Vec<C>)>,
    capacity: usize,
    scratch: Vec<C>,
    #[cfg(test)]
    transforms: usize,
}

impl Spectra {
    fn new(capacity: usize, fft: &dyn Fft<f32>) -> Self {
        Self {
            entries: Vec::new(),
            capacity,
            scratch: vec![C::default(); fft.get_inplace_scratch_len()],
            #[cfg(test)]
            transforms: 0,
        }
    }

    fn get(&mut self, second: isize, audio: &[f32], fft: &dyn Fft<f32>) -> &[C] {
        if let Some(index) = self.entries.iter().position(|(key, _)| *key == second) {
            // Most-recently-used last. Move the Vec handle, not its FFT data.
            let entry = self.entries.remove(index);
            self.entries.push(entry);
        } else {
            let mut spectrum = if self.entries.len() == self.capacity {
                self.entries.remove(0).1
            } else {
                vec![C::default(); LONG_FFT_SIZE]
            };
            spectrum.fill(C::default());
            let start = ((second as f32 - 0.5) * 12000.0).round() as isize;
            for (i, v) in spectrum.iter_mut().take(180000).enumerate() {
                let p = start + i as isize;
                if p >= 0 && p < audio.len() as isize {
                    v.re = audio[p as usize];
                }
            }
            fft.process_with_scratch(&mut spectrum, &mut self.scratch);
            #[cfg(test)]
            {
                self.transforms += 1;
            }
            self.entries.push((second, spectrum));
        }
        &self.entries.last().expect("spectrum inserted").1
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct Decode {
    pub start_seconds: f32,
    pub frequency_hz: f32,
    pub snr_db: f32,
    pub message: String,
    pub payload: String,
}
#[derive(Clone, Copy)]
struct Candidate {
    time: f32,
    frequency: f32,
    sync: f32,
}

fn candidates(audio: &[f32], low: f32, high: f32, planner: &mut FftPlanner<f32>) -> Vec<Candidate> {
    let bins = (high / 3.125).ceil() as usize + 16;
    let rows = audio.len().saturating_sub(1920) / 480 + 1;
    let fft = planner.plan_fft_forward(3840);
    let mut buffer = vec![C::default(); 3840];
    let mut spectrum = vec![0.0f32; rows * bins];
    for row in 0..rows {
        buffer.fill(C::default());
        for (dst, &v) in buffer
            .iter_mut()
            .zip(audio.iter().skip(row * 480).take(1920))
        {
            dst.re = v;
        }
        fft.process(&mut buffer);
        for i in 0..bins {
            spectrum[row * bins + i] = buffer[i].norm_sqr();
        }
    }
    let first = (low / 3.125).ceil().max(1.0) as usize;
    let last = (high / 3.125).floor() as usize;
    // Full three-block Costas correlation, using the same signal/background ratio as sync8.
    let starts = rows.saturating_sub(168) + 64;
    let width = last - first + 1;
    let mut scores = vec![0.0f32; starts * width];
    for t in 0..starts {
        for f in first..=last {
            let mut signal = 0.0;
            let mut background = 0.0;
            let mut present = 0;
            for block in [0, 36, 72] {
                for (symbol, &tone) in COSTAS.iter().enumerate() {
                    let row = t as isize - 64 + (4 * (block + symbol)) as isize;
                    if row < 0 || row >= rows as isize {
                        continue;
                    }
                    present += 1;
                    let offset = row as usize * bins + f;
                    signal += spectrum[offset + 2 * tone];
                    for k in 0..7 {
                        background += spectrum[offset + 2 * k];
                    }
                }
            }
            if present < 14 {
                continue;
            }
            scores[t * width + f - first] = 6.0 * signal / (background - signal).max(1e-20);
        }
    }
    let mut result = Vec::new();
    for t in 0..starts {
        for f in 0..width {
            let score = scores[t * width + f];
            if score < 2.0 {
                continue;
            }
            let mut peak = true;
            'neighbors: for dt in -2isize..=2 {
                for df in -1isize..=1 {
                    let tt = t as isize + dt;
                    let ff = f as isize + df;
                    if tt >= 0
                        && tt < starts as isize
                        && ff >= 0
                        && ff < width as isize
                        && scores[tt as usize * width + ff as usize] > score
                    {
                        peak = false;
                        break 'neighbors;
                    }
                }
            }
            if peak {
                result.push(Candidate {
                    time: (t as f32 - 64.0) * 0.04,
                    frequency: (f + first) as f32 * 3.125,
                    sync: score,
                });
            }
        }
    }
    // Bound work per second while retaining many stations across the passband.
    result.sort_by(|a, b| b.sync.total_cmp(&a.sync));
    let mut counts = vec![0usize; audio.len() / 12000 + 1];
    result.retain(|c| {
        let n = &mut counts[c.time as usize];
        *n += 1;
        *n <= 160
    });
    result.sort_by(|a, b| a.time.total_cmp(&b.time));
    result
}
fn downsample(spectrum: &[C], frequency: f32, planner: &mut FftPlanner<f32>) -> Vec<C> {
    let center = (frequency / 0.0625).round() as usize;
    let bottom = ((frequency - 9.375) / 0.0625).round().max(1.0) as usize;
    let top = ((frequency + 53.125) / 0.0625).round() as usize;
    let count = top - bottom + 1;
    let mut data = vec![C::default(); 3200];
    for i in 0..count {
        let mut value = spectrum[bottom + i];
        if i <= 100 {
            value *= 0.5 * (1.0 + (std::f32::consts::PI * (100 - i) as f32 / 100.0).cos());
        }
        if count - 1 - i <= 100 {
            value *=
                0.5 * (1.0 + (std::f32::consts::PI * (100 - (count - 1 - i)) as f32 / 100.0).cos());
        }
        data[(i + 3200 - (center - bottom)) % 3200] = value;
    }
    planner.plan_fft_inverse(3200).process(&mut data);
    for v in &mut data {
        *v /= (192000.0f32 * 3200.0).sqrt();
    }
    data
}
fn sync(data: &[C], start: isize, offset: f32) -> f32 {
    let mut score = 0.0;
    for block in [0, 36, 72] {
        for (i, &tone) in COSTAS.iter().enumerate() {
            let pos = start + ((block + i) * 32) as isize;
            if pos < 0 || pos + 32 > data.len() as isize {
                continue;
            }
            let step = -std::f32::consts::TAU * (tone as f32 / 32.0 + offset / 200.0);
            let z = data[pos as usize..pos as usize + 32]
                .iter()
                .enumerate()
                .map(|(j, &v)| v * C::from_polar(1.0, step * j as f32))
                .sum::<C>();
            score += z.norm_sqr();
        }
    }
    score
}
fn metrics(symbols: &[[C; 8]; 79]) -> Vec<[f32; 174]> {
    let mut all = vec![[0.0f32; 174]; 5];
    for nsym in 1..=3 {
        let nt = 1 << (3 * nsym);
        for half in 0..2 {
            for k in (0..29).step_by(nsym) {
                let ks = if half == 0 { 7 + k } else { 43 + k };
                let mut values = vec![0.0f32; nt];
                for (v, value) in values.iter_mut().enumerate() {
                    let mut z = C::default();
                    for s in 0..nsym {
                        z += symbols[ks + s][GRAY[(v >> (3 * (nsym - s - 1))) & 7]];
                    }
                    *value = z.norm();
                }
                for bit in 0..3 * nsym {
                    let pos = half * 87 + k * 3 + bit;
                    if pos >= 174 {
                        continue;
                    }
                    let mut one = 0f32;
                    let mut zero = 0f32;
                    for (v, &value) in values.iter().enumerate() {
                        if v & (1 << (3 * nsym - 1 - bit)) != 0 {
                            one = one.max(value);
                        } else {
                            zero = zero.max(value);
                        }
                    }
                    all[nsym - 1][pos] = one - zero;
                    if nsym == 1 {
                        all[3][pos] = (one - zero) / one.max(zero).max(1e-20);
                    }
                }
            }
        }
    }
    let (inputs, output) = all.split_at_mut(4);
    for (i, value) in output[0].iter_mut().enumerate() {
        *value = (0..3)
            .map(|j| inputs[j][i])
            .max_by(|a, b| a.abs().total_cmp(&b.abs()))
            .unwrap();
    }
    for metric in &mut all {
        let mean = metric.iter().sum::<f32>() / 174.0;
        let variance = metric.iter().map(|v| v * v).sum::<f32>() / 174.0 - mean * mean;
        let scale = 2.83 / variance.max(1e-20).sqrt();
        for v in metric {
            *v *= scale;
        }
    }
    all
}

pub fn decode(audio: &[f32], low: f32, high: f32) -> Vec<Decode> {
    decode_with_passes(audio, low, high, 2)
}

pub fn decode_with_passes(audio: &[f32], low: f32, high: f32, passes: usize) -> Vec<Decode> {
    decode_with_context(audio, low, high, passes, &mut Messages::default())
}

/// Receive context retains learned callsigns across successive audio windows.
pub fn decode_with_context(
    audio: &[f32],
    low: f32,
    high: f32,
    passes: usize,
    messages: &mut Messages,
) -> Vec<Decode> {
    decode_with_context_and_observer(audio, low, high, passes, messages, &[], &mut |_| {})
}

/// Notify validated results as found, before remaining candidates/passes finish.
/// Priority frequencies only reorder candidates; all candidates and passes remain.
/// Observer text reflects hash context at emission, and is never rewritten.
pub fn decode_with_context_and_observer(
    audio: &[f32],
    low: f32,
    high: f32,
    passes: usize,
    messages: &mut Messages,
    priority_frequencies: &[f32],
    observer: &mut dyn FnMut(&Decode),
) -> Vec<Decode> {
    let mut reported: Vec<Decode> = Vec::new();
    let mut residual = audio.to_vec();
    let mut results: Vec<Decode> = Vec::new();
    let mut subtractor = crate::waveform::Subtractor::default();
    for pass in 0..passes {
        eprintln!("Decode pass {} of {}", pass + 1, passes);
        let found = decode_once(
            &residual,
            low,
            high,
            messages,
            priority_frequencies,
            &mut |result| {
                if reported.iter().any(|r| {
                    r.payload == result.payload
                        && (r.start_seconds - result.start_seconds).abs() < 0.3
                        && (r.frequency_hz - result.frequency_hz).abs() < 6.0
                }) {
                    return;
                }
                observer(result);
                reported.push(result.clone());
            },
        );
        let mut added = 0;
        for result in found {
            if results.iter().any(|r| {
                r.payload == result.payload
                    && (r.start_seconds - result.start_seconds).abs() < 0.3
                    && (r.frequency_hz - result.frequency_hz).abs() < 6.0
            }) {
                continue;
            }
            if pass + 1 < passes {
                let payload: [u8; 77] = result
                    .payload
                    .bytes()
                    .map(|b| b - b'0')
                    .collect::<Vec<_>>()
                    .try_into()
                    .unwrap();
                subtractor.subtract(
                    &mut residual,
                    &payload,
                    result.frequency_hz,
                    result.start_seconds,
                );
            }
            results.push(result);
            added += 1;
        }
        if added == 0 {
            break;
        }
    }
    // Resolve hashes again after all callsigns in the recording have been learned.
    for result in &mut results {
        let payload: [u8; 77] = result
            .payload
            .bytes()
            .map(|b| b - b'0')
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        if let Some(text) = messages.unpack(&payload) {
            result.message = text;
        }
    }
    results.sort_by(|a, b| {
        a.start_seconds
            .total_cmp(&b.start_seconds)
            .then(a.frequency_hz.total_cmp(&b.frequency_hz))
    });
    results
}

fn decode_once(
    audio: &[f32],
    low: f32,
    high: f32,
    messages: &mut Messages,
    priorities: &[f32],
    observer: &mut dyn FnMut(&Decode),
) -> Vec<Decode> {
    let mut planner = FftPlanner::<f32>::new();
    let mut candidates = candidates(audio, low, high, &mut planner);
    if !priorities.is_empty() {
        let mut ordered: Vec<_> = candidates
            .into_iter()
            .map(|c| {
                let priority = priorities
                    .iter()
                    .any(|&f| f.is_finite() && (f - c.frequency).abs() <= 60.0);
                (priority, c)
            })
            .collect();
        // Stable ordering keeps the existing time order outside priority regions.
        ordered.sort_by(|(pa, a), (pb, b)| {
            pb.cmp(pa).then_with(|| {
                if *pa {
                    b.sync.total_cmp(&a.sync)
                } else {
                    std::cmp::Ordering::Equal
                }
            })
        });
        candidates = ordered.into_iter().map(|(_, c)| c).collect();
    }
    eprintln!("Searching {} Costas candidates", candidates.len());
    let mut results: Vec<Decode> = Vec::new();
    let long_fft = planner.plan_fft_forward(LONG_FFT_SIZE);
    // Priority sync sorting revisits seconds. A single cached spectrum would
    // rebuild a large FFT for almost every candidate in a crowded queue.
    let mut spectra = Spectra::new(
        if priorities.is_empty() {
            1
        } else {
            PRIORITY_SPECTRA
        },
        &*long_fft,
    );
    let symbol_fft = planner.plan_fft_forward(32);
    let mut last_report = -1;
    for candidate in candidates {
        let second = candidate.time.floor() as isize;
        if second / 15 != last_report {
            last_report = second / 15;
            eprintln!("At {:.0}s: {} messages", candidate.time, results.len());
        }
        if results.iter().any(|r| {
            (r.start_seconds - candidate.time).abs() < 0.12
                && (r.frequency_hz - candidate.frequency).abs() < 4.0
        }) {
            continue;
        }
        let origin = second as f32 - 0.5;
        let spectrum = spectra.get(second, audio, &*long_fft);
        let mut frequency = candidate.frequency;
        let mut data = downsample(spectrum, frequency, &mut planner);
        let initial = ((candidate.time - origin) * 200.0).round() as isize;
        let mut start = (initial - 10..=initial + 10)
            .max_by(|&a, &b| sync(&data, a, 0.0).total_cmp(&sync(&data, b, 0.0)))
            .unwrap();
        let tweak = (-5..=5)
            .max_by(|&a, &b| {
                sync(&data, start, a as f32 * 0.5).total_cmp(&sync(&data, start, b as f32 * 0.5))
            })
            .unwrap() as f32
            * 0.5;
        frequency += tweak;
        data = downsample(spectrum, frequency, &mut planner);
        start = (start - 4..=start + 4)
            .max_by(|&a, &b| sync(&data, a, 0.0).total_cmp(&sync(&data, b, 0.0)))
            .unwrap();
        let mut symbols = [[C::default(); 8]; 79];
        for (k, symbol) in symbols.iter_mut().enumerate() {
            let pos = start + k as isize * 32;
            if pos < 0 || pos + 32 > data.len() as isize {
                continue;
            }
            let mut buffer = data[pos as usize..pos as usize + 32].to_vec();
            symbol_fft.process(&mut buffer);
            symbol.copy_from_slice(&buffer[..8]);
        }
        let mut matches = 0;
        for block in [0, 36, 72] {
            for (i, &tone) in COSTAS.iter().enumerate() {
                let best = (0..8)
                    .max_by(|&a, &b| {
                        symbols[block + i][a]
                            .norm_sqr()
                            .total_cmp(&symbols[block + i][b].norm_sqr())
                    })
                    .unwrap();
                if best == tone {
                    matches += 1;
                }
            }
        }
        if matches < 7 {
            continue;
        }
        for metric in metrics(&symbols) {
            let Some(payload) = ldpc::decode(&metric) else {
                continue;
            };
            let codeword = ldpc::encode(&payload);
            if codeword
                .iter()
                .zip(metric.iter())
                .filter(|(b, l)| (2.0 * **b as f32 - 1.0) * **l < 0.0)
                .count()
                > 36
            {
                continue;
            }
            let Some(message) = messages.unpack(&payload) else {
                continue;
            };
            let time = origin + start as f32 / 200.0;
            if results.iter().any(|r| {
                r.message == message
                    && (r.start_seconds - time).abs() < 0.3
                    && (r.frequency_hz - frequency).abs() < 6.0
            }) {
                break;
            }
            let cw = ldpc::encode(&payload);
            let mut signal = 0.0;
            let mut noise = 0.0;
            let mut bit = 0;
            for (k, symbol) in symbols.iter().enumerate() {
                let tone = if k < 7 {
                    COSTAS[k]
                } else if (36..43).contains(&k) {
                    COSTAS[k - 36]
                } else if k >= 72 {
                    COSTAS[k - 72]
                } else {
                    let v = 4 * cw[bit] + 2 * cw[bit + 1] + cw[bit + 2];
                    bit += 3;
                    GRAY[v as usize]
                };
                signal += symbol[tone].norm_sqr();
                noise += symbol[(tone + 4) % 7].norm_sqr();
            }
            let snr = 10.0 * (signal / noise.max(1e-20) - 1.0).max(0.001).log10() - 27.0;
            if matches <= 10 && snr < -25.0 {
                continue;
            }
            let snr = snr.max(-25.0);
            let result = Decode {
                start_seconds: time,
                frequency_hz: frequency,
                snr_db: snr,
                message,
                payload: payload.iter().map(|&b| char::from(b'0' + b)).collect(),
            };
            observer(&result);
            results.push(result);
            break;
        }
    }
    results.sort_by(|a, b| {
        a.start_seconds
            .total_cmp(&b.start_seconds)
            .then(a.frequency_hz.total_cmp(&b.frequency_hz))
    });
    results
}

#[cfg(test)]
mod spectrum_tests {
    use super::*;

    #[test]
    fn reordered_candidates_reuse_exact_spectra_with_bounded_storage() {
        let audio: Vec<f32> = (0..360000).map(|i| (i as f32 * 0.173).sin()).collect();
        let fft = FftPlanner::new().plan_fft_forward(LONG_FFT_SIZE);
        let mut cached = Spectra::new(PRIORITY_SPECTRA, &*fft);
        // Jump between all 32 start seconds as priority/sync sorting does,
        // including the partially captured leading and trailing buffers.
        for round in 0..3 {
            for i in 0..PRIORITY_SPECTRA {
                let second = ((i * 13 + round * 7) % PRIORITY_SPECTRA) as isize - 3;
                let actual = cached.get(second, &audio, &*fft);
                let mut expected = vec![C::default(); LONG_FFT_SIZE];
                for (i, value) in expected[..180000].iter_mut().enumerate() {
                    let sample = second * 12000 - 6000 + i as isize;
                    if sample >= 0
                        && let Some(&sample) = audio.get(sample as usize)
                    {
                        value.re = sample;
                    }
                }
                fft.process(&mut expected);
                assert_eq!(actual, expected);
            }
        }
        assert_eq!(cached.transforms, PRIORITY_SPECTRA);
        for second in 29..70 {
            cached.get(second, &audio, &*fft);
            assert_eq!(cached.entries.len(), PRIORITY_SPECTRA);
        }
        assert_eq!(cached.transforms, PRIORITY_SPECTRA + 41);
        cached.get(69, &audio, &*fft);
        assert_eq!(cached.transforms, PRIORITY_SPECTRA + 41);
    }
}
