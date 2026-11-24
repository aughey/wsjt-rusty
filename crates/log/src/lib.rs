//! Headless contact records, ADIF formatting and transactional storage contracts.
//! No decoder, UI framework, browser, device, filesystem or SQLite dependency.
pub mod adif;
mod record;
pub mod store;
pub use record::*;
pub use store::{
    ContactStore, Database, Export, LogError, Page, Request, Response, SqlStore, Value,
};

pub fn utc(ms: f64) -> Result<String, LogError> {
    let t = timestamp(ms)?;
    Ok(format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
        t.year(),
        t.month() as u8,
        t.day(),
        t.hour(),
        t.minute(),
        t.second(),
        t.millisecond()
    ))
}
pub(crate) fn timestamp(ms: f64) -> Result<time::OffsetDateTime, LogError> {
    if !ms.is_finite() || ms.abs() > 253_402_300_799_999.0 {
        return Err(LogError("Invalid UTC timestamp".into()));
    }
    let t = time::OffsetDateTime::from_unix_timestamp_nanos(ms.trunc() as i128 * 1_000_000)
        .map_err(|e| LogError(e.to_string()))?;
    if !(1..=9999).contains(&t.year()) {
        return Err(LogError("UTC year must be 0001–9999".into()));
    }
    Ok(t)
}
