//! Presentation categories for ordinary FT8 exchanges, not a tracked QSO state.
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

impl MessageKind {
    pub const ALL: [Self; 7] = [
        Self::Cq,
        Self::Reply,
        Self::Report,
        Self::RogerReport,
        Self::Acknowledgment,
        Self::Signoff,
        Self::Other,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cq => "CQ / calling",
            Self::Reply => "Reply / locator",
            Self::Report => "Signal report",
            Self::RogerReport => "R-report",
            Self::Acknowledgment => "RRR",
            Self::Signoff => "73 / RR73",
            Self::Other => "Other",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Cq => "◉",
            Self::Reply => "◇",
            Self::Report => "+",
            Self::RogerReport => "R",
            Self::Acknowledgment => "✓",
            Self::Signoff => "✦",
            Self::Other => "·",
        }
    }

    pub fn color(self) -> &'static str {
        match self {
            Self::Cq => "#75ed91",
            Self::Reply => "#60dcea",
            Self::Report => "#ffd16b",
            Self::RogerReport => "#ce9bff",
            Self::Acknowledgment => "#88b8ff",
            Self::Signoff => "#ff9dcd",
            Self::Other => "#c7d1dc",
        }
    }
}

/// Limit ordinary exchange labels to standard/nonstandard callsign payloads
/// (i3 = 1, 2, 4). Free text, telemetry and specialized contest forms stay Other,
/// even when their rendered text contains a word such as CQ or 73.
pub fn classify(message: &str, payload: &str) -> MessageKind {
    ft8_conversations::classify(message, payload).into()
}

impl From<ft8_conversations::MessageKind> for MessageKind {
    fn from(kind: ft8_conversations::MessageKind) -> Self {
        match kind {
            ft8_conversations::MessageKind::Cq => Self::Cq,
            ft8_conversations::MessageKind::Reply => Self::Reply,
            ft8_conversations::MessageKind::Report => Self::Report,
            ft8_conversations::MessageKind::RogerReport => Self::RogerReport,
            ft8_conversations::MessageKind::Acknowledgment => Self::Acknowledgment,
            ft8_conversations::MessageKind::Signoff => Self::Signoff,
            ft8_conversations::MessageKind::Other => Self::Other,
        }
    }
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
