use wsjt_audio as audio;

#[test]
fn wav_resampling_suppresses_aliases_and_averages_channels() {
    let path = std::env::temp_dir().join(format!("wsjt-rusty-resample-{}.wav", std::process::id()));
    let spec = hound::WavSpec {
        channels: 2,
        sample_rate: 48000,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(&path, spec).unwrap();
    for i in 0..48000 {
        let t = i as f64 / 48000.0;
        let sample = 0.3 * (std::f64::consts::TAU * 1000.0 * t).sin()
            + 0.3 * (std::f64::consts::TAU * 9000.0 * t).sin();
        writer.write_sample((sample * 32767.0) as i16).unwrap();
        writer.write_sample(0i16).unwrap();
    }
    writer.finalize().unwrap();
    let resampled = audio::read(&path).unwrap();
    std::fs::remove_file(path).unwrap();
    assert_eq!(resampled.len(), 12000);
    let amplitude = |freq: f64| -> f64 {
        let (sin, cos) =
            resampled[1000..11000]
                .iter()
                .enumerate()
                .fold((0.0, 0.0), |(s, c), (i, &v)| {
                    let angle = std::f64::consts::TAU * freq * (i + 1000) as f64 / 12000.0;
                    (s + v as f64 * angle.sin(), c + v as f64 * angle.cos())
                });
        2.0 * (sin * sin + cos * cos).sqrt() / 10000.0
    };
    assert!((amplitude(1000.0) - 0.15).abs() < 0.002);
    assert!(amplitude(3000.0) < 0.001); // 9kHz would alias here without filtering
}
