//! Semantic categories for ordinary FT8 exchanges; classification alone is not a QSO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MessageKind {
    Cq,
    Reply,
    Report,
    RogerReport,
    Acknowledgment,
    Signoff,
    Other,
}

/// Limit ordinary exchange labels to standard/nonstandard callsign payloads
/// (i3 = 1, 2, 4). Free text, telemetry and specialized contest forms stay Other,
/// even when their rendered text contains a word such as CQ or 73.
pub fn classify(message: &str, payload: &str) -> MessageKind {
    use MessageKind::*;
    if payload.len() != 77 || !payload.bytes().all(|c| matches!(c, b'0' | b'1')) {
        return Other;
    }
    let kind = payload.as_bytes()[74..]
        .iter()
        .fold(0, |n, &bit| n * 2 + bit - b'0');
    if !matches!(kind, 1 | 2 | 4) {
        return Other;
    }
    let words: Vec<_> = message.split_whitespace().collect();
    if words.len() < 2 {
        return Other;
    }
    if matches!(words[0], "CQ" | "QRZ") {
        return Cq;
    }
    let tail = words[words.len() - 1];
    match tail {
        "73" | "RR73" => Signoff,
        "RRR" => Acknowledgment,
        _ if tail.strip_prefix('R').is_some_and(report) => RogerReport,
        _ if report(tail) => Report,
        _ if words.len() == 2 || locator(tail) => Reply,
        _ => Other,
    }
}

fn report(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 3 && matches!(bytes[0], b'+' | b'-') && bytes[1..].iter().all(u8::is_ascii_digit)
}

pub(crate) fn locator(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 4
        && bytes[..2].iter().all(|c| (b'A'..=b'R').contains(c))
        && bytes[2..].iter().all(u8::is_ascii_digit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload(kind: u8) -> String {
        format!("{}{:03b}", "0".repeat(74), kind)
    }

    #[test]
    fn ordinary_exchanges_and_portable_hashed_calls_have_distinct_categories() {
        for kind in [1, 2, 4] {
            for (message, expected) in [
                ("CQ K1ABC FN42", MessageKind::Cq),
                ("CQ DX K1ABC FN42", MessageKind::Cq),
                ("CQ 123 K1ABC FN42", MessageKind::Cq),
                ("QRZ K1ABC FN42", MessageKind::Cq),
                ("K1ABC W9XYZ EN37", MessageKind::Reply),
                ("K1ABC W9XYZ R EN37", MessageKind::Reply),
                ("<...> PJ4/K1ABC", MessageKind::Reply),
                ("K1ABC/P W9XYZ -12", MessageKind::Report),
                ("K1ABC W9XYZ +02", MessageKind::Report),
                ("K1ABC <W9XYZ> R-12", MessageKind::RogerReport),
                ("K1ABC W9XYZ R+02", MessageKind::RogerReport),
                ("K1ABC W9XYZ RRR", MessageKind::Acknowledgment),
                ("K1ABC W9XYZ RR73", MessageKind::Signoff),
                ("K1ABC W9XYZ 73", MessageKind::Signoff),
            ] {
                assert_eq!(classify(message, &payload(kind)), expected, "{message}");
            }
        }
    }

    #[test]
    fn free_text_contest_and_malformed_messages_are_not_mistaken_for_qso_steps() {
        for kind in [0, 3, 5] {
            assert_eq!(
                classify("CQ K1ABC FN42", &payload(kind)),
                MessageKind::Other
            );
            assert_eq!(
                classify("K1ABC W9XYZ 73", &payload(kind)),
                MessageKind::Other
            );
        }
        for message in [
            "",
            "CQ",
            "K1ABC W9XYZ R73",
            "K1ABC W9XYZ -1",
            "K1ABC W9XYZ ZZ99",
        ] {
            assert_eq!(classify(message, &payload(1)), MessageKind::Other);
        }
        assert_eq!(classify("K1ABC W9XYZ 73", ""), MessageKind::Other);
    }
}
