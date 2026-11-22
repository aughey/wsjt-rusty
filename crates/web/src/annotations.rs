//! Decode rectangles in the waterfall's audio-clock coordinate system.
pub const HISTORY_MS: f64 = 60_000.0;
// Eight FT8 tones spaced 6.25 Hz, with half a tone of padding at either edge.
pub const TONE_SPACING_HZ: f64 = 6.25;

/// Nominal FT8 audio boundaries, in the same FFT-center coordinates as boxes.
/// These are UTC references, not measurements of propagation/device latency.
pub fn timing_guides(clock: Option<(f64, f64)>) -> Vec<(f64, bool)> {
    let Some((first, latest)) = clock else {
        return Vec::new();
    };
    if !first.is_finite() || !latest.is_finite() || first > latest {
        return Vec::new();
    }
    let oldest = first.max(latest - HISTORY_MS);
    let mut guides = Vec::new();
    let first_slot = ((oldest - 13_140.0) / 15_000.0).ceil() as i64;
    let last_slot = ((latest - 500.0) / 15_000.0).floor() as i64;
    for slot in first_slot..=last_slot {
        for (offset, start) in [(500.0, true), (13_140.0, false)] {
            let time = slot as f64 * 15_000.0 + offset;
            if time >= oldest && time <= latest {
                guides.push(((latest - time) / HISTORY_MS * 100.0, start));
            }
        }
    }
    guides
}

/// Observed start minus the nearest nominal slot +0.5 s (includes clock and I/O).
pub fn start_offset_seconds(start_ms: f64) -> f64 {
    let slot = ((start_ms - 500.0) / 15_000.0).round();
    (start_ms - (slot * 15_000.0 + 500.0)) / 1000.0
}

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
}

/// Percentages clipped to both the visible passband and captured history.
/// `latest_ms` and `first_ms` describe painted FFT row centers, not decode arrival.
pub fn bounds(
    clock: Option<(f64, f64)>,
    low: f32,
    high: f32,
    start_ms: f64,
    end_ms: f64,
    lowest_tone_hz: f32,
) -> Option<Bounds> {
    let (first_ms, latest_ms) = clock?;
    if ![
        first_ms,
        latest_ms,
        start_ms,
        end_ms,
        low.into(),
        high.into(),
        lowest_tone_hz.into(),
    ]
    .iter()
    .all(|v| v.is_finite())
        || low >= high
        || start_ms >= end_ms
        || first_ms > latest_ms
    {
        return None;
    }
    let left_hz = (f64::from(lowest_tone_hz) - TONE_SPACING_HZ / 2.0).max(low.into());
    let right_hz = (f64::from(lowest_tone_hz) + 7.5 * TONE_SPACING_HZ).min(high.into());
    let start = start_ms.max(first_ms).max(latest_ms - HISTORY_MS);
    let end = end_ms.min(latest_ms);
    if right_hz <= left_hz || end <= start {
        return None;
    }
    let span = f64::from(high - low);
    Some(Bounds {
        left: (left_hz - f64::from(low)) / span * 100.0,
        top: (latest_ms - end) / HISTORY_MS * 100.0,
        width: (right_hz - left_hz) / span * 100.0,
        height: (end - start) / HISTORY_MS * 100.0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timing_references_clip_scroll_and_keep_the_nominal_half_second() {
        assert!(timing_guides(None).is_empty());
        assert!(timing_guides(Some((f64::NAN, 1000.0))).is_empty());
        assert!(timing_guides(Some((0.0, 499.0))).is_empty());
        assert_eq!(timing_guides(Some((0.0, 500.0))), vec![(0.0, true)]);
        let guides = timing_guides(Some((0.0, 15_500.0)));
        assert_eq!(guides.len(), 3);
        assert_eq!(guides[0], (25.0, true));
        assert_eq!(guides[1], ((15_500.0 - 13_140.0) / 600.0, false));
        assert_eq!(guides[2], (0.0, true));
        let scrolled = timing_guides(Some((0.0, 75_500.0)));
        assert_eq!(scrolled.len(), 9);
        assert!(scrolled.iter().all(|(top, _)| (0.0..=100.0).contains(top)));
        assert_eq!(start_offset_seconds(15_500.0), 0.0);
        assert_eq!(start_offset_seconds(15_800.0), 0.3);
        assert_eq!(start_offset_seconds(15_200.0), -0.3);
    }

    #[test]
    fn late_decodes_align_to_signal_time_and_move_with_audio_history() {
        let b = bounds(
            Some((0.0, 40000.0)),
            100.0,
            4000.0,
            10000.0,
            22640.0,
            1500.0,
        )
        .unwrap();
        assert!((b.top - 28.933333333).abs() < 1e-6);
        assert!((b.height - 21.066666667).abs() < 1e-6);
        assert!((b.width - 50.0 / 3900.0 * 100.0).abs() < 1e-6);
        let moved = bounds(
            Some((0.0, 55000.0)),
            100.0,
            4000.0,
            10000.0,
            22640.0,
            1500.0,
        )
        .unwrap();
        assert!((moved.top - b.top - 25.0).abs() < 1e-6);
        assert_eq!(moved.left, b.left);
    }

    #[test]
    fn clip_partial_frames_and_passband_edges_and_expire_old_frames() {
        let b = bounds(Some((0.0, 5000.0)), 100.0, 4000.0, -1000.0, 11640.0, 100.0).unwrap();
        assert_eq!(b.left, 0.0);
        assert_eq!(b.top, 0.0);
        assert!((b.height - 5000.0 / HISTORY_MS * 100.0).abs() < 1e-6);
        let b = bounds(Some((0.0, 70000.0)), 100.0, 4000.0, 5000.0, 17640.0, 3990.0).unwrap();
        assert!((b.left + b.width - 100.0).abs() < 1e-6);
        assert!((b.top + b.height - 100.0).abs() < 1e-6);
        assert!(bounds(Some((0.0, 90000.0)), 100.0, 4000.0, 5000.0, 17640.0, 1500.0).is_none());
        assert!(
            bounds(
                Some((0.0, 40000.0)),
                100.0,
                4000.0,
                10000.0,
                22640.0,
                4100.0
            )
            .is_none()
        );
        assert!(bounds(None, 100.0, 4000.0, 0.0, 12640.0, 1500.0).is_none());
    }
}
