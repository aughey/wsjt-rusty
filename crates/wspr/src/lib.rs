// SPDX-License-Identifier: GPL-3.0-or-later
// Rust port of WSJT-X wsprd.c, Copyright 2001-2015 Joe Taylor, K1JT,
// and 2014-2015 Steven Franke, K9AN. See README for adaptations.
pub mod fano;
pub mod message;
mod tables;
use message::Messages;
use rustfft::{FftPlanner, num_complex::Complex32 as C};
use tables::SYNC;
const RATE: f32 = 375.0;
const DF: f32 = RATE / 256.0;
const FRAME: usize = 162 * 256;
#[derive(Clone, Debug)]
pub struct Decode {
    pub start_seconds: f32,
    pub frequency_hz: f32,
    pub snr_db: f32,
    pub drift_hz: f32,
    pub payload: [u8; 50],
    pub decoded: message::Message,
}
#[derive(Clone, Copy, Debug)]
struct Candidate {
    frequency: f32,
    shift: isize,
    drift: f32,
    sync: f32,
    snr: f32,
}
fn downsample(audio: &[f32], center: f32) -> Vec<C> {
    // Same FFT channel extraction as readwavfile; retain all available audio.
    let n2 = audio.len().div_ceil(32).max(46080).next_multiple_of(1536);
    let n1 = n2 * 32;
    let mut planner = FftPlanner::new();
    let mut long = vec![C::default(); n1];
    for (v, &x) in long.iter_mut().zip(audio) {
        v.re = x;
    }
    planner.plan_fft_forward(n1).process(&mut long);
    let center = (center * n1 as f32 / 12000.0).round() as usize;
    let mut short = vec![C::default(); n2];
    for (i, v) in short.iter_mut().enumerate() {
        let offset = if i > n2 / 2 {
            i as isize - n2 as isize
        } else {
            i as isize
        };
        let index = center as isize + offset;
        if index >= 0 && index < n1 as isize / 2 {
            *v = long[index as usize];
        }
    }
    planner.plan_fft_inverse(n2).process(&mut short);
    for v in &mut short {
        *v /= n1 as f32;
    }
    short.truncate(audio.len().div_ceil(32));
    short
}
fn candidates(data: &[C], low: f32, high: f32, max_drift: i32) -> Vec<Candidate> {
    if data.len() < 512 {
        return Vec::new();
    }
    let rows = (data.len() - 512) / 128 + 1;
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(512);
    let mut ps = vec![[0.0f32; 512]; rows];
    let mut average = [0.0f32; 512];
    let window: [f32; 512] = std::array::from_fn(|i| (0.006147931 * i as f32).sin());
    let mut buffer = [C::default(); 512];
    for (row, power) in ps.iter_mut().enumerate() {
        for j in 0..512 {
            buffer[j] = data[row * 128 + j] * window[j];
        }
        fft.process(&mut buffer);
        for j in 0..512 {
            power[j] = buffer[(j + 256) % 512].norm_sqr();
            average[j] += power[j];
        }
    }
    let mut smooth = [0f32; 411];
    for (i, s) in smooth.iter_mut().enumerate() {
        *s = average[48 + i..55 + i].iter().sum();
    }
    let mut sorted = smooth;
    sorted.sort_by(f32::total_cmp);
    let noise = sorted[122];
    if noise <= 1e-30 {
        return Vec::new();
    }
    let minimum = 10f32.powf(-0.8);
    for p in &mut smooth {
        *p = (*p / noise - 1.0).max(0.1 * minimum);
    }
    let mut result = Vec::new();
    for j in 1..410 {
        let frequency = (j as f32 - 205.0) * DF / 2.0;
        if frequency < low
            || frequency > high
            || smooth[j] <= smooth[j - 1]
            || smooth[j] <= smooth[j + 1]
        {
            continue;
        }
        let mut best = Candidate {
            frequency,
            shift: 0,
            drift: 0.0,
            sync: -1.0,
            snr: 10.0 * smooth[j].log10() - 26.3,
        };
        let bin = (frequency / (DF / 2.0) + 256.0) as isize;
        // Search all possible starts with at least 150 symbols in this window,
        // including the original wsprd negative-start allowance.
        let last = (rows as isize - 300).max(22);
        for f in bin - 2..=bin + 2 {
            for lag in -10..last {
                for drift in -max_drift..=max_drift {
                    let mut score = 0.0;
                    let mut total = 0.0;
                    for (k, &sync) in SYNC.iter().enumerate() {
                        let row = lag + 2 * k as isize;
                        if row < 0 || row >= rows as isize {
                            continue;
                        }
                        let b = (f as f32 + (k as f32 - 81.0) / 81.0 * drift as f32 / DF) as isize;
                        if !(3..509).contains(&b) {
                            continue;
                        }
                        let p = &ps[row as usize];
                        let (p0, p1, p2, p3) = (
                            p[(b - 3) as usize].sqrt(),
                            p[(b - 1) as usize].sqrt(),
                            p[(b + 1) as usize].sqrt(),
                            p[(b + 3) as usize].sqrt(),
                        );
                        score += (2.0 * sync as f32 - 1.0) * (p1 + p3 - p0 - p2);
                        total += p0 + p1 + p2 + p3;
                    }
                    let sync = score / total.max(1e-30);
                    if sync > best.sync {
                        best = Candidate {
                            frequency: (f - 256) as f32 * DF / 2.0,
                            shift: 128 * (lag + 1),
                            drift: drift as f32,
                            sync,
                            snr: best.snr,
                        };
                    }
                }
            }
        }
        result.push(best);
    }
    result.sort_by(|a, b| b.snr.total_cmp(&a.snr));
    result.truncate(200);
    result
}
fn correlations(data: &[C], candidate: Candidate) -> ([[C; 4]; 162], [[C; 4]; 162]) {
    let mut symbols = [[C::default(); 4]; 162];
    let mut phases = [[C::default(); 4]; 162];
    for i in 0..162 {
        let frequency = candidate.frequency + candidate.drift * 0.5 * (i as f32 - 81.0) / 81.0;
        let mut oscillator = [C::new(1.0, 0.0); 4];
        let step: [C; 4] = std::array::from_fn(|tone| {
            C::from_polar(
                1.0,
                -std::f32::consts::TAU * (frequency + (tone as f32 - 1.5) * DF) / RATE,
            )
        });
        for j in 0..256 {
            let index = candidate.shift + (i * 256 + j) as isize;
            if index > 0 && index < data.len() as isize {
                for (tone, &osc) in oscillator.iter().enumerate() {
                    symbols[i][tone] += data[index as usize] * osc;
                }
            }
            for tone in 0..4 {
                oscillator[tone] *= step[tone];
            }
        }
        phases[i] = oscillator;
    }
    (symbols, phases)
}
fn sync(data: &[C], candidate: Candidate) -> f32 {
    let (symbols, _) = correlations(data, candidate);
    let mut signal = 0.0;
    let mut total = 0.0;
    for (i, p) in symbols.iter().enumerate() {
        let p = p.map(|z| z.norm());
        signal += (2.0 * SYNC[i] as f32 - 1.0) * (p[1] + p[3] - p[0] - p[2]);
        total += p.iter().sum::<f32>();
    }
    signal / total.max(1e-30)
}
fn refine(data: &[C], mut c: Candidate, refine_drift: bool) -> Candidate {
    for (radius, step) in [(128, 64), (32, 16)] {
        let initial = c;
        c.sync = f32::NEG_INFINITY;
        for offset in (-radius..=radius).step_by(step as usize) {
            let mut next = initial;
            next.shift += offset;
            next.sync = sync(data, next);
            if next.sync > c.sync {
                c = next;
            }
        }
        let initial = c;
        c.sync = f32::NEG_INFINITY;
        let step = if radius == 128 { 0.25 } else { 0.05 };
        for offset in -2..=2 {
            let mut next = initial;
            next.frequency += offset as f32 * step;
            next.sync = sync(data, next);
            if next.sync > c.sync {
                c = next;
            }
        }
        if radius == 128 && refine_drift {
            let initial = c;
            for drift in [-0.5, 0.5] {
                let mut next = initial;
                next.drift += drift;
                next.sync = sync(data, next);
                if next.sync > c.sync {
                    c = next;
                }
            }
        }
    }
    c
}
fn soft_symbols(data: &[C], c: Candidate, block: usize, normalized: bool) -> [u8; 162] {
    let (symbols, phases) = correlations(data, c);
    let mut metrics = [0f32; 162];
    for start in (0..162).step_by(block) {
        let count = 1 << block;
        let mut values = vec![0f32; count];
        for (sequence, value) in values.iter_mut().enumerate() {
            let mut z = C::default();
            let mut phase = C::new(1.0, 0.0);
            for j in 0..block {
                let bit = (sequence >> (block - 1 - j)) & 1;
                let tone = SYNC[start + j] as usize + 2 * bit;
                z += symbols[start + j][tone] * phase;
                phase *= phases[start + j][tone];
            }
            *value = z.norm();
        }
        for bit in 0..block {
            let mut one = 0f32;
            let mut zero = 0f32;
            for (sequence, &value) in values.iter().enumerate() {
                if sequence & (1 << (block - 1 - bit)) != 0 {
                    one = one.max(value);
                } else {
                    zero = zero.max(value);
                }
            }
            metrics[start + bit] = (one - zero)
                / if normalized {
                    one.max(zero).max(1e-30)
                } else {
                    1.0
                };
        }
    }
    let mean = metrics.iter().sum::<f32>() / 162.0;
    let sigma = (metrics.iter().map(|x| x * x).sum::<f32>() / 162.0 - mean * mean)
        .max(1e-30)
        .sqrt();
    metrics.map(|m| (128.0 + (50.0 * m / sigma).clamp(-128.0, 127.0)) as u8)
}
fn subtract(data: &mut [C], candidate: Candidate, payload: &[u8; 50]) {
    // subtract_signal2's phase-continuous reference and 360-point sine LPF.
    let tones = fano::tones(payload);
    let mut reference = vec![C::default(); FRAME];
    let mut phase = 0f64;
    for i in 0..162 {
        let frequency = candidate.frequency
            + candidate.drift * 0.5 * (i as f32 - 81.0) / 81.0
            + (tones[i] as f32 - 1.5) * DF;
        let step = std::f64::consts::TAU * frequency as f64 / RATE as f64;
        for j in 0..256 {
            reference[i * 256 + j] = C::from_polar(1.0, phase as f32);
            phase = (phase + step) % std::f64::consts::TAU;
        }
    }
    let mut mixed = vec![C::default(); FRAME + 360];
    for (i, &r) in reference.iter().enumerate() {
        let index = candidate.shift + i as isize;
        if index > 0 && index < data.len() as isize {
            mixed[180 + i] = data[index as usize] * r.conj();
        }
    }
    let mut window: [f32; 360] =
        std::array::from_fn(|i| (std::f32::consts::PI * i as f32 / 359.0).sin());
    let sum = window.iter().sum::<f32>();
    for w in &mut window {
        *w /= sum;
    }
    let mut partial = [0f32; 360];
    for i in 1..360 {
        partial[i] = partial[i - 1] + window[i];
    }
    for (i, &r) in reference.iter().enumerate() {
        let index = candidate.shift + i as isize;
        if index <= 0 || index >= data.len() as isize {
            continue;
        }
        let amplitude = mixed[i..i + 360]
            .iter()
            .zip(window)
            .map(|(&z, w)| z * w)
            .sum::<C>();
        let norm = if i < 180 {
            partial[180 + i]
        } else if i > FRAME - 181 {
            partial[180 + FRAME - 1 - i]
        } else {
            1.0
        };
        data[index as usize] -= amplitude * r / norm;
    }
}
/// Decode one bounded 12 kHz mono window. `center_hz` selects the 300 Hz WSPR subband.
pub fn decode(
    audio: &[f32],
    low: f32,
    high: f32,
    passes: usize,
    messages: &mut Messages,
) -> Vec<Decode> {
    let center = (low + high) * 0.5;
    let mut data = downsample(audio, center);
    let mut results: Vec<Decode> = Vec::new();
    for pass in 0..passes {
        let candidates = candidates(
            &data,
            low - center,
            high - center,
            if pass >= 2 { 0 } else { 4 },
        );
        eprintln!("WSPR pass {}: {} candidates", pass + 1, candidates.len());
        let mut added = 0;
        for candidate in candidates {
            let mut candidate = refine(&data, candidate, pass < 2);
            if candidate.sync < if pass >= 2 { 0.10 } else { 0.12 } {
                continue;
            }
            if results.iter().any(|r| {
                (r.frequency_hz - center - candidate.frequency).abs() < 0.1
                    && (r.start_seconds - candidate.shift as f32 / RATE).abs() < 0.2
            }) {
                continue;
            }
            let mut found = None;
            'attempts: for (block, normalized) in [(1, false), (2, false), (3, false), (1, true)] {
                for jitter in [
                    0, -8, 8, -16, 16, -24, 24, -32, 32, -40, 40, -48, 48, -56, 56, -64, 64,
                ] {
                    let mut trial = candidate;
                    trial.shift += jitter;
                    let symbols = soft_symbols(&data, trial, block, normalized);
                    let rms = (symbols
                        .iter()
                        .map(|&s| (s as f32 - 128.0).powi(2))
                        .sum::<f32>()
                        / 162.0)
                        .sqrt();
                    if rms < 40.625 {
                        continue;
                    }
                    let Some(payload) = fano::decode(
                        &fano::deinterleave(&symbols),
                        if pass >= 2 { 50_000 } else { 10_000 },
                    ) else {
                        continue;
                    };
                    let Some(decoded) = messages.unpack(&payload) else {
                        continue;
                    };
                    found = Some((payload, decoded));
                    candidate = trial;
                    break 'attempts;
                }
            }
            if let Some((payload, decoded)) = found {
                let start = candidate.shift as f32 / RATE;
                let dupe = results.iter().any(|r| {
                    r.decoded.callsign == decoded.callsign
                        && (r.frequency_hz - center - candidate.frequency).abs() < 4.0
                        && (r.start_seconds - start).abs() < 5.0
                });
                if !dupe {
                    if pass + 1 < passes {
                        subtract(&mut data, candidate, &payload);
                    }
                    results.push(Decode {
                        start_seconds: start,
                        frequency_hz: center + candidate.frequency,
                        snr_db: candidate.snr,
                        drift_hz: candidate.drift,
                        payload,
                        decoded,
                    });
                    added += 1;
                }
            }
        }
        if added == 0 {
            break;
        }
    }
    for r in &mut results {
        if let Some(message) = messages.unpack(&r.payload) {
            r.decoded = message;
        }
    }
    results.sort_by(|a, b| {
        a.start_seconds
            .total_cmp(&b.start_seconds)
            .then(a.frequency_hz.total_cmp(&b.frequency_hz))
    });
    results
}
