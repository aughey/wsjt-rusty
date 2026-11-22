use std::{collections::BTreeSet, path::Path};
use wsjt_rusty::{
    audio,
    decoder::{AudioDecoder, Mode, Settings, WsprDecoder},
    wspr::{fano, message::Messages},
};
fn sample() -> Vec<f32> {
    audio::read(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/wspr-150426-0918.wav"
    )))
    .unwrap()
}
#[test]
fn native_wsjtx_code_vectors_and_all_message_types() {
    for line in include_str!("../../../tests/fixtures/wspr-code-vectors.txt")
        .lines()
        .filter_map(|l| l.strip_prefix("hash:"))
    {
        let (text, value) = line.rsplit_once(':').unwrap();
        assert_eq!(
            wsjt_rusty::wspr::message::hash(text),
            value.parse::<u16>().unwrap()
        );
    }
    let mut messages = Messages::default();
    for line in include_str!("../../../tests/fixtures/wspr-code-vectors.txt")
        .lines()
        .filter(|l| !l.starts_with("hash:"))
    {
        let (text, tones) = line.split_once('|').unwrap();
        let tones: [u8; 162] = tones
            .bytes()
            .map(|c| c - b'0')
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
        let soft = tones.map(|tone| if tone >= 2 { 255 } else { 0 });
        let payload = fano::decode(&fano::deinterleave(&soft), 10000).unwrap();
        assert_eq!(fano::tones(&payload), tones, "{text}");
        let decoded = messages.unpack(&payload).unwrap();
        assert_eq!(decoded.text, text);
        let mut noisy = fano::deinterleave(&soft);
        for i in [3, 18, 37, 64, 97, 120] {
            noisy[i] = 255 - noisy[i];
        }
        assert_eq!(fano::decode(&noisy, 10000), Some(payload));
    }
}
#[test]
fn public_recording_matches_native_wsprd_messages() {
    let mut decoder = WsprDecoder::new(Settings::defaults(Mode::Wspr)).unwrap();
    let results = decoder.decode(&sample());
    let messages: BTreeSet<_> = results.iter().map(|r| r.message.as_str()).collect();
    let expected: BTreeSet<_> = [
        "ND6P DM04 30",
        "W5BIT EL09 17",
        "G8VDQ IO91 37",
        "WD4LHT EL89 30",
        "NM7J DM26 30",
        "KI7CI DM09 37",
        "DJ6OL JO52 37",
        "W3HH EL89 30",
        "W3BI FN20 30",
    ]
    .into_iter()
    .collect();
    assert_eq!(messages, expected);
    assert_eq!(results.len(), expected.len());
    for r in results {
        assert_eq!(r.mode, Mode::Wspr);
        assert_eq!(r.payload.len(), 50);
        assert_eq!(r.frequency_reference, "center");
        assert!(r.frequency_hz >= 1390.0 && r.frequency_hz <= 1610.0);
    }
}
#[test]
fn silence_and_noise_do_not_decode() {
    let mut settings = Settings::defaults(Mode::Wspr);
    settings.passes = 1;
    let mut decoder = WsprDecoder::new(settings).unwrap();
    assert!(decoder.decode(&vec![0.0; 120 * 12000]).is_empty());
    let mut state = 17u64;
    let noise: Vec<f32> = (0..120 * 12000)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state as u32 as f32 / u32::MAX as f32 - 0.5) * 0.2
        })
        .collect();
    assert!(decoder.decode(&noise).is_empty());
}

#[test]
fn native_weak_signal_soft_symbols() {
    let symbols: &[u8; 162] = include_bytes!("../../../tests/fixtures/wspr-g8vdq-soft.bin");
    let payload = fano::decode(&fano::deinterleave(symbols), 10000)
        .expect("native G8VDQ soft symbols should decode");
    assert_eq!(
        Messages::default().unpack(&payload).unwrap().text,
        "G8VDQ IO91 37"
    );
}

#[test]
fn shifted_recording_finds_start_without_utc_alignment() {
    let mut audio = vec![0.0; 7 * 12000];
    audio.extend(sample());
    let mut settings = Settings::defaults(Mode::Wspr);
    settings.passes = 1;
    let mut decoder = WsprDecoder::new(settings).unwrap();
    let results = decoder.decode(&audio);
    let known = results
        .iter()
        .find(|r| r.message == "ND6P DM04 30")
        .unwrap();
    assert!(
        (known.start_seconds - 9.1333).abs() < 0.3,
        "{}",
        known.start_seconds
    );
    assert!((known.frequency_hz - 1446.28).abs() < 0.3);
}

#[test]
fn both_protocols_decode_from_the_same_chunked_audio() {
    use wsjt_rusty::decoder::{AudioStream, Ft8Decoder};
    let mut audio = sample();
    let payload: [u8; 77] =
        "00001001101111111001101110010100000010111110001010001111100100101101101011001"
            .bytes()
            .map(|b| b - b'0')
            .collect::<Vec<_>>()
            .try_into()
            .unwrap();
    let wave = wsjt_rusty::waveform::generate(&wsjt_rusty::waveform::tones(&payload), 2501.25);
    for (sample, z) in audio.iter_mut().skip((20.73 * 12000.0) as usize).zip(wave) {
        *sample += 0.1 * z.re;
    }
    let ft8 = Ft8Decoder::new(Settings {
        low_hz: 2400.0,
        high_hz: 2600.0,
        passes: 1,
    })
    .unwrap();
    let wspr = WsprDecoder::new(Settings::defaults(Mode::Wspr)).unwrap();
    let mut stream = AudioStream::new(vec![Box::new(ft8), Box::new(wspr)]).unwrap();
    let mut results = Vec::new();
    for chunk in audio.chunks(3197) {
        results.extend(stream.push(chunk).unwrap());
    }
    results.extend(stream.finish());
    let ft8: Vec<_> = results
        .iter()
        .filter(|r| r.mode == Mode::Ft8 && r.message == "K1JT HA0DU KN07")
        .collect();
    assert_eq!(ft8.len(), 1);
    assert!((ft8[0].start_seconds - 20.73).abs() < 0.03);
    assert_eq!(results.iter().filter(|r| r.mode == Mode::Wspr).count(), 9);
}

#[test]
fn live_wspr_window_emits_before_eof_and_deduplicates_final_overlap() {
    use wsjt_rusty::decoder::AudioStream;
    let mut audio = vec![0.0; 17 * 12000];
    audio.extend(sample());
    audio.resize(140 * 12000, 0.0);
    let decoder = WsprDecoder::new(Settings {
        low_hz: 1430.0,
        high_hz: 1470.0,
        passes: 1,
    })
    .unwrap();
    let mut stream = AudioStream::new(vec![Box::new(decoder)]).unwrap();
    let mut results = Vec::new();
    for chunk in audio.chunks(4096) {
        results.extend(stream.push(chunk).unwrap());
    }
    assert!(
        results.iter().any(|r| r.message == "ND6P DM04 30"),
        "expected decode before EOF"
    );
    results.extend(stream.finish());
    let known: Vec<_> = results
        .iter()
        .filter(|r| r.message == "ND6P DM04 30")
        .collect();
    assert_eq!(known.len(), 1);
    assert!((known[0].start_seconds - 19.1333).abs() < 0.3);
}
