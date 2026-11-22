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

#[test]
fn transmit_packing_matches_recorded_payload_and_pcm_decodes() {
    // Payload captured from the retained digirig recording, not the new packer.
    let text = "VO2RC W3DVX R-01";
    let expected = "11100010110001000110000100010000011000000100011010001111101111111010110010001";
    let bits = ft8::transmit::pack(text).unwrap();
    assert_eq!(
        bits.map(|b| char::from(b'0' + b))
            .iter()
            .collect::<String>(),
        expected
    );
    let pcm = ft8::transmit::audio(text, 1200.0).unwrap();
    assert_eq!(pcm.len(), 151_680);
    assert!(pcm.iter().all(|s| s.is_finite() && s.abs() <= 0.5));
    let mut slot = vec![0.0; 180_000];
    slot[6000..6000 + pcm.len()].copy_from_slice(&pcm);
    let decodes = ft8::decode(&slot, 1100.0, 1300.0);
    assert!(decodes.iter().any(|d| d.message == text
        && d.payload == expected
        && (d.start_seconds - 0.5).abs() < 0.03
        && (d.frequency_hz - 1200.0).abs() < 1.0));
}

#[test]
fn prioritized_incremental_decode_preserves_the_full_recorded_payload_set() {
    use std::{collections::BTreeSet, time::Instant};
    let samples = audio::read(std::path::Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/ft8-210703-133430.wav"
    )))
    .unwrap();
    let baseline = ft8::decode_with_passes(&samples, 100.0, 4000.0, 2);
    let started = Instant::now();
    let mut emitted = Vec::new();
    let mut first = None;
    let result = ft8::decode_with_context_and_observer(
        &samples,
        100.0,
        4000.0,
        2,
        &mut Default::default(),
        &[1196.875],
        &mut |r| {
            if first.is_none() {
                first = Some(started.elapsed());
            }
            emitted.push(r.clone());
        },
    );
    assert_eq!(
        emitted[0].message, "CQ F5RXL IN94",
        "queued frequency should be attempted before the rest of the band"
    );
    let payloads = |r: &[ft8::Decode]| r.iter().map(|r| r.payload.clone()).collect::<BTreeSet<_>>();
    assert_eq!(payloads(&result), payloads(&baseline));
    assert_eq!(payloads(&emitted), payloads(&result));
    assert_eq!(result.len(), baseline.len());
    eprintln!(
        "First prioritized CQ: {:?}; all {} recorded messages: {:?}",
        first.unwrap(),
        result.len(),
        started.elapsed()
    );
}

#[test]
fn stream_observers_adjust_origins_and_share_overlap_deduplication() {
    use wsjt_rusty::decoder::{AudioDecoder, AudioStream, Ft8Decoder, Mode, Settings};
    let frame = ft8::transmit::audio("CQ K1ABC FN42", 1234.375).unwrap();
    let mut pcm = vec![0.0; 15 * 12000];
    pcm[6000..6000 + frame.len()].copy_from_slice(&frame);
    let decoder = Ft8Decoder::new(Settings::defaults(Mode::Ft8)).unwrap();
    let mut stream = AudioStream::new(vec![Box::new(decoder) as Box<dyn AudioDecoder>]).unwrap();
    stream.set_priority_frequencies(&[1234.375]);
    let mut callbacks = Vec::new();
    let mut observer = |r: &wsjt_rusty::decoder::Decode| callbacks.push(r.clone());
    assert!(
        stream
            .push_with_observer(&pcm, &mut observer)
            .unwrap()
            .is_empty()
    );
    let early = stream.decode_pending_with_observer(16 * 12000, &mut observer);
    assert_eq!(early.len(), 1);
    assert!(
        stream
            .decode_pending_with_observer(16 * 12000, &mut observer)
            .is_empty()
    );
    assert!(stream.finish_with_observer(&mut observer).is_empty());
    assert_eq!(callbacks.len(), 1);
    assert_eq!(callbacks[0].payload, early[0].payload);
    assert!((callbacks[0].start_seconds - 0.5).abs() < 0.03);
    assert!(stream.push(&pcm).is_err());
    // A full first window moves the retained origin by a hop; callbacks add it once.
    let decoder = Ft8Decoder::new(Settings::defaults(Mode::Ft8)).unwrap();
    let mut stream = AudioStream::new(vec![Box::new(decoder)]).unwrap();
    stream.push(&vec![0.0; 30 * 12000]).unwrap();
    let mut callbacks = Vec::new();
    let batch = stream
        .push_with_observer(&pcm, &mut |r| callbacks.push(r.clone()))
        .unwrap();
    assert_eq!(batch.len(), 1);
    assert_eq!(callbacks.len(), 1);
    assert!((callbacks[0].start_seconds - 30.5).abs() < 0.03);
}
