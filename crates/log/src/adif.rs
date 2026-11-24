//! ADIF 3.1.7 ADI output, shared verbatim by browser and native adapters.
//! ASCII field lengths remain correct even with Unicode or '<EOR>' in evidence.
use crate::{ContactRecord, LogError, timestamp, utc};

pub const HEADER: &str = "<ADIF_VER:5>3.1.7<PROGRAMID:10>WSJT_RUSTY<EOH>\n";
fn field(output: &mut String, name: &str, value: &str) {
    let mut ascii = String::new();
    for c in value.chars() {
        if c.is_ascii() {
            ascii.push(c);
        } else {
            let mut utf16 = [0; 2];
            for unit in c.encode_utf16(&mut utf16) {
                ascii.push_str(&format!("\\u{unit:04x}"));
            }
        }
    }
    output.push_str(&format!("<{name}:{}>{ascii}", ascii.len()));
}
fn parts(ms: f64) -> Result<(String, String), LogError> {
    let t = timestamp(ms)?;
    Ok((
        format!("{:04}{:02}{:02}", t.year(), t.month() as u8, t.day()),
        format!("{:02}{:02}{:02}", t.hour(), t.minute(), t.second()),
    ))
}
pub fn record(record: &ContactRecord) -> Result<String, LogError> {
    record.validate()?;
    let mut text = String::new();
    let (date, time) = parts(record.start_ms)?;
    let (end_date, end_time) = parts(record.end_ms)?;
    for (name, value) in [
        ("CALL", record.target.as_str()),
        ("STATION_CALLSIGN", record.station_call.as_str()),
        ("MODE", "FT8"),
        ("QSO_DATE", &date),
        ("TIME_ON", &time),
        ("QSO_DATE_OFF", &end_date),
        ("TIME_OFF", &end_time),
    ] {
        field(&mut text, name, value);
    }
    if let Some(grid) = &record.grid {
        field(&mut text, "GRIDSQUARE", grid);
    }
    if !record.station_grid.is_empty() {
        field(&mut text, "MY_GRIDSQUARE", &record.station_grid);
    }
    if let Some(hz) = record.dial_hz.filter(|hz| *hz > 0) {
        field(
            &mut text,
            "FREQ",
            &format!("{:.6}", hz as f64 / 1_000_000.0),
        );
        if let Some(ms) = record.frequency_read_ms {
            field(&mut text, "APP_WSJT_RUSTY_FREQ_READ_UTC", &utc(ms)?);
        }
    }
    for (direction, name) in [("TX audio", "RST_SENT"), ("RX", "RST_RCVD")] {
        if let Some(report) = record
            .evidence
            .iter()
            .rev()
            .filter(|e| e.direction == direction)
            .find_map(|e| {
                let w: Vec<_> = e.message.split_whitespace().collect();
                let (to, from) = if direction == "RX" {
                    (&record.station_call, &record.target)
                } else {
                    (&record.target, &record.station_call)
                };
                if w.first() != Some(&to.as_str()) || w.get(1) != Some(&from.as_str()) {
                    return None;
                }
                let report = w.get(2)?.strip_prefix('R').unwrap_or(w[2]);
                let b = report.as_bytes();
                (b.len() == 3
                    && matches!(b[0], b'+' | b'-')
                    && b[1..].iter().all(u8::is_ascii_digit))
                .then_some(report)
            })
        {
            field(&mut text, name, report);
        }
    }
    field(
        &mut text,
        "QSO_COMPLETE",
        if record.outcome == "Completed" {
            "?"
        } else {
            "N"
        },
    );
    field(
        &mut text,
        "COMMENT",
        &format!(
            "{}; local software exchange state; RF delivery and mutual QSO completion not confirmed",
            record.outcome
        ),
    );
    field(&mut text, "APP_WSJT_RUSTY_ID", &record.id);
    field(&mut text, "APP_WSJT_RUSTY_STATUS", &record.outcome);
    field(
        &mut text,
        "APP_WSJT_RUSTY_EVIDENCE",
        &serde_json::to_string(&record.evidence)?,
    );
    text.push_str("<EOR>\n");
    Ok(text)
}
