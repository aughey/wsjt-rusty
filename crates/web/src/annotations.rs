//! Decode rectangles in the waterfall's audio-clock coordinate system.
pub const HISTORY_MS: f64 = 60_000.0;
// Eight FT8 tones spaced 6.25 Hz, with half a tone of padding at either edge.
pub const TONE_SPACING_HZ: f64 = 6.25;

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
