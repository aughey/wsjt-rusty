// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from WSJT-X, Copyright (C) 2001-2023 Joe Taylor, K1JT, and contributors.
//! Port of gen_ft8wave, gfsk_pulse, and subtractft8 (without optional DT refinement).
use crate::ldpc;
use rustfft::{FftPlanner, num_complex::Complex32 as C};
const NSPS: usize = 1920;
const FRAME: usize = 79 * NSPS;
const NFFT: usize = 180000;
const NFILT: usize = 4000;

pub fn tones(payload: &[u8; 77]) -> [usize; 79] {
    let cw = ldpc::encode(payload);
    let mut tones = [0; 79];
    let mut bit = 0;
    for (k, tone) in tones.iter_mut().enumerate() {
        *tone = if k < 7 {
            [3, 1, 4, 0, 6, 5, 2][k]
        } else if (36..43).contains(&k) {
            [3, 1, 4, 0, 6, 5, 2][k - 36]
        } else if k >= 72 {
            [3, 1, 4, 0, 6, 5, 2][k - 72]
        } else {
            let v = 4 * cw[bit] + 2 * cw[bit + 1] + cw[bit + 2];
            bit += 3;
            [0, 1, 3, 2, 5, 6, 4, 7][v as usize]
        };
    }
    tones
}
pub fn generate(tones: &[usize; 79], frequency: f32) -> Vec<C> {
    let tau = std::f64::consts::TAU;
    let c = std::f64::consts::PI * (2.0 / std::f64::consts::LN_2).sqrt();
    let pulse: Vec<f64> = (1..=3 * NSPS)
        .map(|i| {
            let t = (i as f64 - 1.5 * NSPS as f64) / NSPS as f64;
            0.5 * (libm::erf(c * 2.0 * (t + 0.5)) - libm::erf(c * 2.0 * (t - 0.5)))
        })
        .collect();
    let mut dphi = vec![tau * frequency as f64 / 12000.0; 81 * NSPS];
    let peak = tau / NSPS as f64;
    for (j, &tone) in tones.iter().enumerate() {
        for (i, &p) in pulse.iter().enumerate() {
            dphi[j * NSPS + i] += peak * tone as f64 * p;
        }
    }
    for i in 0..2 * NSPS {
        dphi[i] += peak * tones[0] as f64 * pulse[NSPS + i];
        dphi[79 * NSPS + i] += peak * tones[78] as f64 * pulse[i];
    }
    let ramp = NSPS / 8;
    let mut phi = 0.0;
    let mut out = Vec::with_capacity(FRAME);
    for k in 0..FRAME {
        let envelope = if k < ramp {
            0.5 * (1.0 - (std::f64::consts::PI * k as f64 / ramp as f64).cos())
        } else if k >= FRAME - ramp {
            0.5 * (1.0 + (std::f64::consts::PI * (k - (FRAME - ramp)) as f64 / ramp as f64).cos())
        } else {
            1.0
        };
        out.push(C::from_polar(envelope as f32, phi as f32));
        phi = (phi + dphi[NSPS + k]) % tau;
    }
    out
}
pub struct Subtractor {
    planner: FftPlanner<f32>,
    filter: Vec<C>,
    correction: Vec<f32>,
}
impl Default for Subtractor {
    fn default() -> Self {
        let mut planner = FftPlanner::new();
        let window: Vec<f32> = (-(NFILT as isize) / 2..=NFILT as isize / 2)
            .map(|j| {
                (std::f32::consts::PI * j as f32 / NFILT as f32)
                    .cos()
                    .powi(2)
            })
            .collect();
        let sum = window.iter().sum::<f32>();
        let mut filter = vec![C::default(); NFFT];
        // Fortran cshift by NFILT/2+1; keep its one-sample filter phase.
        for (i, &w) in window.iter().enumerate() {
            filter[(i + NFFT - (NFILT / 2 + 1)) % NFFT].re = w / sum / NFFT as f32;
        }
        planner.plan_fft_forward(NFFT).process(&mut filter);
        let mut correction = vec![0.0; NFILT / 2 + 1];
        for (i, c) in correction.iter_mut().enumerate() {
            *c = 1.0 / (1.0 - window[NFILT / 2 + i..].iter().sum::<f32>() / sum);
        }
        Self {
            planner,
            filter,
            correction,
        }
    }
}
impl Subtractor {
    pub fn subtract(&mut self, audio: &mut [f32], payload: &[u8; 77], frequency: f32, time: f32) {
        let reference = generate(&tones(payload), frequency);
        let start = (time * 12000.0).round() as isize;
        let mut amplitude = vec![C::default(); NFFT];
        for i in 0..FRAME {
            let j = start + i as isize;
            if j >= 0 && j < audio.len() as isize {
                amplitude[i] = audio[j as usize] * reference[i].conj();
            }
        }
        self.planner.plan_fft_forward(NFFT).process(&mut amplitude);
        for (v, &filter) in amplitude.iter_mut().zip(&self.filter) {
            *v *= filter;
        }
        self.planner.plan_fft_inverse(NFFT).process(&mut amplitude);
        for (i, &c) in self.correction.iter().enumerate() {
            amplitude[i] *= c;
            amplitude[FRAME - 1 - i] *= c;
        }
        for i in 0..FRAME {
            let j = start + i as isize;
            if j >= 0 && j < audio.len() as isize {
                audio[j as usize] -= 2.0 * (reference[i] * amplitude[i]).re;
            }
        }
    }
}
