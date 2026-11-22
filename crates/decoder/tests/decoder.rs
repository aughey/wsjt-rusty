use wsjt_rusty::{audio, ft8, ldpc, waveform};
const PAYLOAD: &str =
    "00001001101111111001101110010100000010111110001010001111100100101101101011001";
fn payload() -> [u8; 77] {
    PAYLOAD
        .bytes()
        .map(|b| b - b'0')
        .collect::<Vec<_>>()
        .try_into()
        .unwrap()
}

#[test]
fn ldpc_recovers_bit_errors_and_rejects_zero_word() {
    let message = payload();
    let cw = ldpc::encode(&message);
    let mut llr = cw.map(|b| if b == 1 { 5.0 } else { -5.0 });
    for i in [0, 7, 31, 55, 88, 120] {
        llr[i] *= -0.3;
    }
    assert_eq!(ldpc::decode(&llr), Some(message));
    assert_eq!(ldpc::decode(&[-5.0; 174]), None);
    assert_eq!(ldpc::decode(&[0.0; 174]), None);
}

#[test]
fn bundled_wsjt_sample_and_unknown_slot_offset() {
    let audio = audio::read(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/ft8-210703-133430.wav"
    )))
    .unwrap();
    let original = ft8::decode_with_passes(&audio, 100.0, 4000.0, 1);
    assert!(original.len() >= 14, "{} decodes", original.len());
    let known = original
        .iter()
        .find(|r| r.message == "K1JT HA0DU KN07")
        .unwrap();
    assert!((known.frequency_hz - 589.6).abs() < 1.0);
    assert!((known.start_seconds - 0.795).abs() < 0.03);
    let mut shifted = vec![0.0; (3.27 * 12000.0) as usize];
    shifted.extend_from_slice(&audio);
    let decoded = ft8::decode_with_passes(&shifted, 100.0, 4000.0, 1);
    let moved = decoded.iter().find(|r| r.message == known.message).unwrap();
    assert!((moved.start_seconds - known.start_seconds - 3.27).abs() < 0.03);
}

#[test]
fn overlapping_stations_and_partial_final_frame() {
    let message = payload();
    let reference = waveform::generate(&waveform::tones(&message), 1001.25);
    let mut audio = vec![0.0; 15 * 12000];
    let start = (3.417 * 12000.0) as usize;
    for (dst, z) in audio.iter_mut().skip(start).zip(&reference) {
        *dst = 0.3 * z.re;
    }
    let mut other = message;
    other[58] = 1; // same calls and locator with the R reply bit
    let reference2 = waveform::generate(&waveform::tones(&other), 1377.0);
    for (dst, z) in audio.iter_mut().skip(start + 240).zip(&reference2) {
        *dst += 0.15 * z.im;
    }
    let results = ft8::decode_with_passes(&audio, 900.0, 1500.0, 1);
    assert!(
        results
            .iter()
            .any(|r| r.payload == PAYLOAD && (r.start_seconds - 3.417).abs() < 0.03)
    );
    assert!(results.iter().any(|r| r.message == "K1JT HA0DU R KN07"));
}

#[test]
fn subtraction_removes_reconstructed_signal() {
    let message = payload();
    let reference = waveform::generate(&waveform::tones(&message), 1001.25);
    let start = 6000;
    let mut audio = vec![0.0; 180000];
    for (dst, z) in audio.iter_mut().skip(start).zip(&reference) {
        *dst = 0.3 * z.re;
    }
    let before = audio.iter().map(|v| v * v).sum::<f32>();
    waveform::Subtractor::default().subtract(&mut audio, &message, 1001.25, 0.5);
    let after = audio.iter().map(|v| v * v).sum::<f32>();
    assert!(
        after / before < 0.01,
        "remaining power fraction {}",
        after / before
    );
}

#[test]
fn silence_produces_no_decodes() {
    assert!(ft8::decode_with_passes(&vec![0.0; 180000], 100.0, 4000.0, 1).is_empty());
}

#[test]
fn leading_frame_with_missing_first_symbols() {
    let reference = waveform::generate(&waveform::tones(&payload()), 1001.25);
    let mut audio = vec![0.0; 15 * 12000];
    let missing = 8400; // recording begins 0.7s into the transmission
    for (dst, z) in audio.iter_mut().zip(&reference[missing..]) {
        *dst = 0.3 * z.re;
    }
    let results = ft8::decode_with_passes(&audio, 900.0, 1150.0, 1);
    let decoded = results.iter().find(|r| r.payload == PAYLOAD).unwrap();
    assert!((decoded.start_seconds + 0.7).abs() < 0.03);
}

#[test]
fn deterministic_noise_produces_no_decodes() {
    let mut state = 17u64;
    let audio: Vec<f32> = (0..180000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state as u32 as f32 / u32::MAX as f32 - 0.5) * 0.2
        })
        .collect();
    assert!(ft8::decode_with_passes(&audio, 100.0, 4000.0, 1).is_empty());
}
