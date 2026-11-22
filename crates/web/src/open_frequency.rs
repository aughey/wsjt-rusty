//! Bounded spectrum occupancy for optional transmit placement. Never alters RX PCM.
use std::collections::VecDeque;

const HISTORY_MS: f64 = 60_000.0;
const MAX_ROWS: usize = 600;

pub struct Occupancy {
    low: f32,
    high: f32,
    rows: VecDeque<(f64, Vec<f32>)>,
    totals: Vec<f32>,
}

impl Occupancy {
    pub fn new(low: f32, high: f32, width: usize) -> Self {
        Self {
            low,
            high,
            rows: VecDeque::new(),
            totals: vec![0.0; width],
        }
    }

    /// One actual waterfall snapshot; missing rows never count as quiet audio.
    /// Score excess above each row's median, independent of display leveling.
    pub fn push(&mut self, ms: f64, levels: &[f32]) {
        if !ms.is_finite()
            || levels.len() != self.totals.len()
            || levels.is_empty()
            || levels.iter().any(|v| !v.is_finite())
        {
            return;
        }
        if self.rows.back().is_some_and(|(last, _)| ms <= *last) {
            return;
        }
        while self
            .rows
            .front()
            .is_some_and(|(time, _)| ms - time >= HISTORY_MS)
            || self.rows.len() >= MAX_ROWS
        {
            let (_, row) = self.rows.pop_front().expect("retained row");
            for (total, value) in self.totals.iter_mut().zip(row) {
                *total -= value;
            }
        }
        let mut sorted = levels.to_vec();
        let mid = sorted.len() / 2;
        let (_, median, _) = sorted.select_nth_unstable_by(mid, f32::total_cmp);
        let row: Vec<f32> = levels
            .iter()
            .map(|db| (db - *median - 6.0).max(0.0))
            .collect();
        for (total, value) in self.totals.iter_mut().zip(&row) {
            *total += value;
        }
        self.rows.push_back((ms, row));
    }

    /// Lowest tone for a 50 Hz FT8 footprint plus 10 Hz guards on both sides.
    /// Minimize accumulated excess across the footprint; ties prefer 1500 Hz.
    pub fn best(&self) -> Option<f32> {
        if self.rows.is_empty()
            || !self.low.is_finite()
            || !self.high.is_finite()
            || self.high <= self.low
            || self.totals.is_empty()
        {
            return None;
        }
        let hz = (self.high - self.low) / self.totals.len() as f32;
        let width = (70.0 / hz).ceil() as usize;
        if width > self.totals.len() {
            return None;
        }
        let mut sum: f32 = self.totals[..width].iter().sum();
        let mut best = (f32::INFINITY, f32::INFINITY, 0.0);
        for start in 0..=self.totals.len() - width {
            if start > 0 {
                sum += self.totals[start + width - 1] - self.totals[start - 1];
            }
            let tone = self.low + start as f32 * hz + 10.0;
            let score = sum.max(0.0) / self.rows.len() as f32;
            let distance = (tone - 1500.0).abs();
            if score < best.0 - 0.001 || ((score - best.0).abs() <= 0.001 && distance < best.1) {
                best = (score, distance, tone);
            }
        }
        Some(best.2)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_history_expires_old_signals_and_keeps_full_footprint_in_band() {
        let mut history = Occupancy::new(100.0, 4000.0, 390);
        assert_eq!(history.best(), None);
        let mut row = vec![-90.0; 390];
        // Only a 100 Hz gap at 2000 Hz is open in this busy spectrum.
        for i in (0..390).step_by(5) {
            row[i] = -30.0;
        }
        row[190..200].fill(-90.0);
        row[189] = -30.0;
        history.push(0.0, &row);
        let tone = history.best().unwrap();
        assert!((2010.0..=2040.0).contains(&tone), "{tone}");
        history.push(59_900.0, &vec![-90.0; 390]);
        assert_eq!(
            history.best(),
            Some(tone),
            "a quiet recent row does not erase history"
        );
        history.push(60_000.0, &vec![-90.0; 390]);
        assert_eq!(history.best(), Some(1500.0));
        for n in 1..=1000 {
            history.push(60_000.0 + n as f64 * 100.0, &row);
        }
        assert_eq!(history.rows.len(), 600);
        assert!((110.0..=3940.0).contains(&history.best().unwrap()));
    }

    #[test]
    fn levels_and_gaps_do_not_invent_occupancy() {
        let mut a = Occupancy::new(100.0, 4000.0, 390);
        let mut b = Occupancy::new(100.0, 4000.0, 390);
        let mut row = vec![-90.0; 390];
        row[130..150].fill(-30.0);
        a.push(100.0, &row);
        b.push(100.0, &row.iter().map(|x| x + 25.0).collect::<Vec<_>>());
        assert_eq!(a.best(), b.best());
        a.push(200.0, &[f32::NAN; 390]);
        assert_eq!(a.rows.len(), 1);
        a.push(70_000.0, &vec![-90.0; 390]);
        assert_eq!(a.rows.len(), 1);
        assert_eq!(a.best(), Some(1500.0));
    }
}
