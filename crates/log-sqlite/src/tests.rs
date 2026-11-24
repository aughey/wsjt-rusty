use super::*;
use wsjt_log::{ContactRecord, ContactStore, Evidence, Request, Response};

fn record(id: &str) -> ContactRecord {
    // Independently specified UTC epoch: 2026-11-22 23:59:55.
    let start = 1_795_391_995_000.0;
    ContactRecord {
        id: id.into(),
        revision: 1,
        station_call: "W9XYZ".into(),
        station_grid: "EN37".into(),
        target: "K1ABC".into(),
        grid: Some("FN42".into()),
        start_ms: start,
        end_ms: start + 20000.0,
        outcome: "Completed".into(),
        dial_hz: None,
        frequency_read_ms: None,
        evidence: vec![
            Evidence {
                start_ms: start,
                end_ms: start + 12640.0,
                direction: "RX".into(),
                message: "W9XYZ K1ABC -09".into(),
                payload: Some("001".into()),
                snr_db: Some(-9.0),
                frequency_hz: 1200.0,
                received_at_ms: Some(start + 13000.0),
            },
            Evidence {
                start_ms: start + 15000.0,
                end_ms: start + 20000.0,
                direction: "TX audio".into(),
                message: "K1ABC W9XYZ R-11".into(),
                payload: None,
                snr_db: None,
                frequency_hz: 1200.0,
                received_at_ms: None,
            },
        ],
    }
}
fn save(store: &mut impl ContactStore, records: Vec<ContactRecord>) -> wsjt_log::Page {
    match store.request(Request::Save { records, offset: 0 }).unwrap() {
        Response::Page(page) => page,
        _ => panic!("wrong response"),
    }
}
fn export(store: &mut impl ContactStore, include_incomplete: bool) -> wsjt_log::Export {
    match store
        .request(Request::Export { include_incomplete })
        .unwrap()
    {
        Response::Export(e) => e,
        _ => panic!("wrong response"),
    }
}
fn fields(text: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('<') {
        rest = &rest[start + 1..];
        let end = rest.find('>').unwrap();
        let tag = &rest[..end];
        rest = &rest[end + 1..];
        if let Some((name, length)) = tag.split_once(':') {
            let n: usize = length.parse().unwrap();
            let value = &rest[..n];
            out.push((name.into(), value.into()));
            rest = &rest[n..];
        }
    }
    out
}
#[test]
fn independent_adif_fields_midnight_reports_ascii_and_real_rf_only() {
    let mut store = SqlStore::new(Connection::memory().unwrap()).unwrap();
    let mut r = record("unicode-å-😀");
    r.evidence.push(Evidence {
        message: "Evidence <EOR> å 😀".into(),
        ..r.evidence[0].clone()
    });
    let incomplete = ContactRecord {
        id: "incomplete".into(),
        outcome: "Interrupted".into(),
        ..r.clone()
    };
    save(&mut store, vec![r.clone(), incomplete]);
    let exported = export(&mut store, false);
    assert_eq!(exported.count, 1);
    let parsed = fields(&exported.text);
    let get = |name: &str| {
        parsed
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    };
    assert_eq!(get("QSO_DATE"), Some("20261122"));
    assert_eq!(get("TIME_ON"), Some("235955"));
    assert_eq!(get("QSO_DATE_OFF"), Some("20261123"));
    assert_eq!(get("TIME_OFF"), Some("000015"));
    assert_eq!(get("RST_SENT"), Some("-11"));
    assert_eq!(get("RST_RCVD"), Some("-09"));
    assert_eq!(get("QSO_COMPLETE"), Some("?"));
    assert!(get("FREQ").is_none());
    assert!(get("BAND").is_none());
    assert!(exported.text.is_ascii());
    let evidence: Vec<Evidence> = serde_json_for_test(get("APP_WSJT_RUSTY_EVIDENCE").unwrap());
    assert_eq!(evidence.len(), 3);
    assert!(evidence.iter().any(|e| e.message == "Evidence <EOR> å 😀"));
    assert_eq!(export(&mut store, true).count, 2);
    r.revision += 1;
    r.dial_hz = Some(14_074_000);
    r.frequency_read_ms = Some(r.start_ms - 1000.0);
    save(&mut store, vec![r]);
    let text = export(&mut store, false).text;
    assert!(text.contains("<FREQ:9>14.074000"));
    assert!(text.contains("2026-11-22T23:59:54.000Z"));
}
fn serde_json_for_test<T: serde::de::DeserializeOwned>(s: &str) -> T {
    serde_json::from_str(s).unwrap()
}
#[test]
fn revisions_order_pages_completion_summary_and_atomic_rollback() {
    let mut store = SqlStore::new(Connection::memory().unwrap()).unwrap();
    let many = (0..32).map(|i| record(&format!("record-{i:02}"))).collect();
    save(&mut store, many);
    let page = store.page(25).unwrap();
    assert_eq!(page.records.len(), 7);
    assert_eq!(page.total, 32);
    assert_eq!(page.contacted.len(), 1);
    let mut newer = record("record-00");
    newer.revision = 3;
    newer.outcome = "Receive stopped: input gap".into();
    save(&mut store, vec![newer]);
    save(&mut store, vec![record("record-00")]);
    assert_eq!(
        store.page(0).unwrap().records[0].outcome,
        "Receive stopped: input gap"
    );
    let mut db = store.into_inner();
    db.execute_batch("CREATE TRIGGER fail_save BEFORE INSERT ON contacts WHEN NEW.id='reject' BEGIN SELECT RAISE(ABORT, 'forced failure'); END;").unwrap();
    let mut store = SqlStore::new(db).unwrap();
    assert!(
        store
            .request(Request::Save {
                records: vec![record("rollback"), record("reject")],
                offset: 0
            })
            .is_err()
    );
    assert_eq!(store.page(0).unwrap().total, 32);
    let mut invalid = record("invalid");
    invalid.evidence[0].snr_db = Some(f32::NAN);
    assert!(
        store
            .request(Request::Save {
                records: vec![invalid],
                offset: 0
            })
            .is_err()
    );
    assert_eq!(store.page(0).unwrap().total, 32);
}
#[test]
fn file_reopen_competing_connections_sql_parameters_and_future_schema() {
    let path = std::env::temp_dir().join(format!(
        "wsjt-log-test-{}-{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    {
        let mut a = open(&path).unwrap();
        save(&mut a, vec![record("a'); DROP TABLE evidence; --")]);
        let mut b = open(&path).unwrap();
        let mut newer = record("a'); DROP TABLE evidence; --");
        newer.revision = 5;
        newer.outcome = "Interrupted".into();
        save(&mut b, vec![newer]);
        save(&mut a, vec![record("a'); DROP TABLE evidence; --")]);
        assert_eq!(a.page(0).unwrap().records[0].revision, 5);
    }
    let mut reopened = open(&path).unwrap();
    assert_eq!(reopened.page(0).unwrap().records[0].evidence.len(), 2);
    let mut db = reopened.into_inner();
    db.execute_batch("PRAGMA user_version=99").unwrap();
    drop(db);
    assert!(
        open(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("newer application")
    );
    let mut db = Connection::open(&path).unwrap();
    assert!(matches!(
        db.query("PRAGMA user_version", &[]).unwrap()[0][0],
        Value::Integer(99)
    ));
    drop(db);
    std::fs::remove_file(path).unwrap();
}
#[test]
fn reads_existing_browser_version_one_json_without_evidence_in_body() {
    let mut db = Connection::memory().unwrap();
    db.execute_batch(wsjt_log::store::SCHEMA).unwrap();
    let mut body = serde_json::to_value(record("legacy-browser")).unwrap();
    body.as_object_mut().unwrap().remove("evidence");
    db.execute(
        "INSERT INTO contacts VALUES(?,?,?,?,?,?)",
        &[
            Value::Text("legacy-browser".into()),
            Value::Integer(1),
            Value::Text("K1ABC".into()),
            Value::Real(record("").start_ms),
            Value::Text("Completed".into()),
            Value::Text(body.to_string()),
        ],
    )
    .unwrap();
    let mut store = SqlStore::new(db).unwrap();
    let page = store.page(0).unwrap();
    assert_eq!(page.records[0].target, "K1ABC");
    assert!(page.records[0].evidence.is_empty());
}
