// SPDX-License-Identifier: GPL-3.0-or-later
// Derived from WSJT-X, Copyright (C) 2001-2023 Joe Taylor, K1JT, and contributors.
//! Restricted standard-message transmit port of pack28 / pack77_1.
//! Unsupported calls and message forms are rejected, never hashed or sent as free text.
use crate::waveform;

/// Pack a standard base callsign (no portable suffixes or hashes).
pub fn pack_call(call: &str) -> Result<u32, &'static str> {
    let area = call
        .bytes()
        .rposition(|b| b.is_ascii_digit())
        .ok_or("Use a standard base callsign with an area digit")?;
    if !call.is_ascii()
        || !(area == 1 || area == 2)
        || call.len() <= area + 1
        || call.len() > area + 4
        || call.starts_with('Q')
        || !call[..area].bytes().any(|b| b.is_ascii_uppercase())
        || !call[area + 1..].bytes().all(|b| b.is_ascii_uppercase())
    {
        return Err("Only standard base callsigns are supported for transmit");
    }
    let mut padded = [b' '; 6];
    let offset = usize::from(area == 1);
    padded[offset..offset + call.len()].copy_from_slice(call.as_bytes());
    let alphabets: [&[u8]; 6] = [
        b" 0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b"0123456789",
        b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
        b" ABCDEFGHIJKLMNOPQRSTUVWXYZ",
    ];
    let mut n = 0;
    for (c, alphabet) in padded.into_iter().zip(alphabets) {
        let digit = alphabet
            .iter()
            .position(|&b| b == c)
            .ok_or("Invalid callsign")?;
        n = n * alphabet.len() as u32 + digit as u32;
    }
    Ok(n + 6_257_896)
}

pub fn grid_index(grid: &str) -> Result<u32, &'static str> {
    let b = grid.as_bytes();
    if b.len() != 4
        || !b[..2].iter().all(|c| (b'A'..=b'R').contains(c))
        || !b[2..].iter().all(u8::is_ascii_digit)
    {
        return Err("Enter a four-character Maidenhead grid, e.g. EN37");
    }
    Ok(u32::from(b[0] - b'A') * 1800
        + u32::from(b[1] - b'A') * 100
        + u32::from(b[2] - b'0') * 10
        + u32::from(b[3] - b'0'))
}

/// Type 1 only: `TO FROM GRID|+dd|-dd|R+dd|R-dd|RRR|RR73|73` or `CQ FROM GRID`.
pub fn pack(message: &str) -> Result<[u8; 77], &'static str> {
    let words: Vec<_> = message.split_whitespace().collect();
    if words.len() != 3 {
        return Err("Expected TO FROM and a locator, report or acknowledgment");
    }
    let a = if words[0] == "CQ" {
        2
    } else {
        pack_call(words[0])?
    };
    let b = pack_call(words[1])?;
    if a == b {
        return Err("Contact and station callsigns must differ");
    }
    let tail = words[2];
    let mut reply = 0;
    let grid = match tail {
        "RRR" => 32402,
        "RR73" => 32403,
        "73" => 32404,
        _ if tail.starts_with(['+', '-']) || tail.starts_with("R+") || tail.starts_with("R-") => {
            let report = if let Some(report) = tail.strip_prefix('R') {
                reply = 1;
                report
            } else {
                tail
            };
            if report.len() != 3 || !report.as_bytes()[1..].iter().all(u8::is_ascii_digit) {
                return Err("Report must be signed two digits");
            }
            let mut snr: i32 = report.parse().map_err(|_| "Invalid signal report")?;
            if !(-50..=49).contains(&snr) {
                return Err("Report must be between -50 and +49 dB");
            }
            if snr < -30 {
                snr += 101;
            }
            (32400 + snr + 35) as u32
        }
        _ => grid_index(tail)?,
    };
    if a == 2 && (reply != 0 || grid >= 32400) {
        return Err("CQ requires a locator");
    }
    let mut bits = [0; 77];
    let mut pos = 0;
    for (value, width) in [
        (a, 28),
        (0, 1),
        (b, 28),
        (0, 1),
        (reply, 1),
        (grid, 15),
        (1, 3),
    ] {
        for shift in (0..width).rev() {
            bits[pos] = ((value >> shift) & 1) as u8;
            pos += 1;
        }
    }
    Ok(bits)
}

/// Mono 12 kHz PCM, 79 GFSK symbols (12.64 s), peak amplitude 0.5.
/// Frequency denotes the lowest tone; timing and device playback belong to the caller.
pub fn audio(message: &str, frequency_hz: f32) -> Result<Vec<f32>, &'static str> {
    if !frequency_hz.is_finite() || !(100.0..=4000.0).contains(&frequency_hz) {
        return Err("Lowest tone must be between 100 and 4000 audio Hz");
    }
    let payload = pack(message)?;
    Ok(waveform::generate(&waveform::tones(&payload), frequency_hz)
        .into_iter()
        .map(|v| 0.5 * v.im)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Messages;
    #[test]
    fn standard_messages_and_report_edges_roundtrip() {
        for tail in [
            "FN42", "-50", "-31", "-30", "+00", "+49", "R-50", "R+49", "RRR", "RR73", "73",
        ] {
            let text = format!("K1ABC W9XYZ {tail}");
            assert_eq!(
                Messages::default().unpack(&pack(&text).unwrap()).as_deref(),
                Some(text.as_str())
            );
        }
        assert_eq!(
            Messages::default()
                .unpack(&pack("CQ K1ABC FN42").unwrap())
                .as_deref(),
            Some("CQ K1ABC FN42")
        );
        for text in [
            "K1ABC/P W9XYZ 73",
            "<K1ABC> W9XYZ 73",
            "CQ W9XYZ RR73",
            "K1ABC W9XYZ +50",
            "K1ABC W9XYZ -51",
            "K1ABC W9XYZ XX99",
            "K1ABC W9XYZ R-1",
            "HELLO WORLD",
        ] {
            assert!(pack(text).is_err(), "{text}");
        }
        assert!(pack_call("é1ABC").is_err());
        assert!(audio("K1ABC W9XYZ 73", f32::NAN).is_err());
    }
}
