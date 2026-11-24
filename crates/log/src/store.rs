use crate::{CompletedContact, ContactRecord, adif};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogError(pub String);
impl std::fmt::Display for LogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for LogError {}
impl From<serde_json::Error> for LogError {
    fn from(e: serde_json::Error) -> Self {
        Self(e.to_string())
    }
}
pub const PAGE_SIZE: usize = 25;
pub const SCHEMA_VERSION: i64 = 1;
pub const SCHEMA:&str="CREATE TABLE IF NOT EXISTS contacts (id TEXT PRIMARY KEY, revision INTEGER NOT NULL,
target TEXT NOT NULL, start_ms REAL NOT NULL, outcome TEXT NOT NULL, body TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS evidence (contact_id TEXT NOT NULL, sequence INTEGER NOT NULL,
start_ms REAL NOT NULL, direction TEXT NOT NULL, body TEXT NOT NULL, PRIMARY KEY(contact_id,sequence));
CREATE INDEX IF NOT EXISTS contact_dates ON contacts(start_ms DESC);
PRAGMA user_version=1;";

/// Portable SQL values: revisions are restricted to JS-safe integers at validation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Integer(i64),
    Real(f64),
    Text(String),
    Null,
}
impl Value {
    fn integer(&self) -> Result<i64, LogError> {
        match self {
            Self::Integer(n) => Ok(*n),
            Self::Real(n) if n.is_finite() && n.fract() == 0.0 => Ok(*n as i64),
            _ => Err(LogError("Expected SQL integer".into())),
        }
    }
    fn text(&self) -> Result<&str, LogError> {
        match self {
            Self::Text(s) => Ok(s),
            _ => Err(LogError("Expected SQL text".into())),
        }
    }
    fn real(&self) -> Result<f64, LogError> {
        match self {
            Self::Integer(n) => Ok(*n as f64),
            Self::Real(n) => Ok(*n),
            _ => Err(LogError("Expected SQL number".into())),
        }
    }
}
/// Synchronous connection used inside an adapter's resource owner/transaction.
/// Native implementations must bind parameters, never interpolate user values.
pub trait Database {
    fn execute_batch(&mut self, sql: &str) -> Result<(), LogError>;
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<(), LogError>;
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, LogError>;
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Page {
    pub records: Vec<ContactRecord>,
    pub total: usize,
    pub contacted: Vec<CompletedContact>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Export {
    pub text: String,
    pub count: usize,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum Request {
    List {
        #[serde(default)]
        offset: usize,
    },
    Save {
        records: Vec<ContactRecord>,
        #[serde(default)]
        offset: usize,
    },
    Export {
        #[serde(default)]
        include_incomplete: bool,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Page(Page),
    Export(Export),
}
/// A durable owner acknowledges Save only after its outer persistence commits.
/// For a file-backed DB SQL COMMIT suffices; browser adapters additionally wait
/// for the IndexedDB image transaction. execute() alone is not an IDB ack.
pub trait ContactStore {
    fn request(&mut self, request: Request) -> Result<Response, LogError>;
}
pub struct SqlStore<D: Database> {
    db: D,
}
impl<D: Database> SqlStore<D> {
    pub fn new(mut db: D) -> Result<Self, LogError> {
        let version = db
            .query("PRAGMA user_version", &[])?
            .first()
            .and_then(|r| r.first())
            .ok_or_else(|| LogError("Missing database schema version".into()))?
            .integer()?;
        if version > SCHEMA_VERSION {
            return Err(LogError(
                "Contact database was created by a newer application. It has not been changed."
                    .into(),
            ));
        }
        if version == 0 {
            db.execute_batch("BEGIN IMMEDIATE")?;
            if let Err(error) = db
                .execute_batch(SCHEMA)
                .and_then(|_| db.execute_batch("COMMIT"))
            {
                let _ = db.execute_batch("ROLLBACK");
                return Err(error);
            }
        }
        Ok(Self { db })
    }
    pub fn into_inner(self) -> D {
        self.db
    }
    fn records(&mut self, offset: usize) -> Result<Vec<ContactRecord>, LogError> {
        let rows = self.db.query(
            "SELECT id,body FROM contacts ORDER BY start_ms DESC,id LIMIT ? OFFSET ?",
            &[
                Value::Integer(PAGE_SIZE as i64),
                Value::Integer(
                    offset
                        .try_into()
                        .map_err(|_| LogError("Invalid offset".into()))?,
                ),
            ],
        )?;
        let mut records = Vec::new();
        for row in rows {
            let mut record: ContactRecord = serde_json::from_str(row[1].text()?)?;
            record.evidence = self
                .db
                .query(
                    "SELECT body FROM evidence WHERE contact_id=? ORDER BY start_ms,sequence",
                    &[row[0].clone()],
                )?
                .into_iter()
                .map(|r| Ok(serde_json::from_str(r[0].text()?)?))
                .collect::<Result<_, LogError>>()?;
            record.validate()?;
            records.push(record);
        }
        Ok(records)
    }
    pub fn page(&mut self, offset: usize) -> Result<Page, LogError> {
        self.db.execute_batch("BEGIN")?;
        let result = self.page_snapshot(offset);
        match result {
            Ok(page) => match self.db.execute_batch("COMMIT") {
                Ok(()) => Ok(page),
                Err(e) => {
                    let _ = self.db.execute_batch("ROLLBACK");
                    Err(e)
                }
            },
            Err(e) => {
                let _ = self.db.execute_batch("ROLLBACK");
                Err(e)
            }
        }
    }
    fn page_snapshot(&mut self, offset: usize) -> Result<Page, LogError> {
        let records = self.records(offset)?;
        let total = self.db.query("SELECT count(*) FROM contacts", &[])?[0][0].integer()? as usize;
        let contacted=self.db.query("SELECT target, MAX(json_extract(body, '$.end_ms')) FROM contacts WHERE outcome='Completed' GROUP BY target",&[])?.into_iter().map(|r|Ok(CompletedContact{target:r[0].text()?.into(),end_ms:r[1].real()?})).collect::<Result<_,LogError>>()?;
        Ok(Page {
            records,
            total,
            contacted,
        })
    }
    fn save(&mut self, records: &[ContactRecord]) -> Result<(), LogError> {
        if records.len() > 512 {
            return Err(LogError("Contact save batch is too large".into()));
        }
        for r in records {
            r.validate()?;
        }
        self.db.execute_batch("BEGIN IMMEDIATE")?;
        let result = (|| {
            for record in records {
                let id = Value::Text(record.id.clone());
                let previous = self.db.query(
                    "SELECT revision FROM contacts WHERE id=?",
                    std::slice::from_ref(&id),
                )?;
                if previous
                    .first()
                    .is_some_and(|row| row[0].integer().is_ok_and(|r| r >= record.revision as i64))
                {
                    continue;
                }
                let mut body = record.clone();
                body.evidence.clear();
                // Keep the version-1 browser schema's JSON body (evidence lives
                // in the evidence table); extra empty evidence is harmless.
                self.db.execute(
                    "INSERT OR REPLACE INTO contacts VALUES(?,?,?,?,?,?)",
                    &[
                        id.clone(),
                        Value::Integer(record.revision as i64),
                        Value::Text(record.target.clone()),
                        Value::Real(record.start_ms),
                        Value::Text(record.outcome.clone()),
                        Value::Text(serde_json::to_string(&body)?),
                    ],
                )?;
                self.db.execute(
                    "DELETE FROM evidence WHERE contact_id=?",
                    std::slice::from_ref(&id),
                )?;
                for (i, event) in record.evidence.iter().enumerate() {
                    self.db.execute(
                        "INSERT INTO evidence VALUES(?,?,?,?,?)",
                        &[
                            id.clone(),
                            Value::Integer(i as i64),
                            Value::Real(event.start_ms),
                            Value::Text(event.direction.clone()),
                            Value::Text(serde_json::to_string(event)?),
                        ],
                    )?;
                }
            }
            self.db.execute_batch("COMMIT")
        })();
        if result.is_err() {
            let _ = self.db.execute_batch("ROLLBACK");
        }
        result
    }
    pub fn write_adif(
        &mut self,
        include_incomplete: bool,
        writer: &mut dyn std::io::Write,
    ) -> Result<usize, LogError> {
        let io = |e: std::io::Error| LogError(e.to_string());
        writer.write_all(adif::HEADER.as_bytes()).map_err(io)?;
        // One read snapshot keeps multi-page exports consistent with concurrent writers.
        self.db.execute_batch("BEGIN")?;
        let result = (|| {
            let mut offset = 0;
            let mut count = 0;
            loop {
                let records = self.records(offset)?;
                if records.is_empty() {
                    break;
                }
                for record in &records {
                    if include_incomplete || record.outcome == "Completed" {
                        writer
                            .write_all(adif::record(record)?.as_bytes())
                            .map_err(io)?;
                        count += 1;
                    }
                }
                offset += records.len();
            }
            self.db.execute_batch("COMMIT")?;
            Ok(count)
        })();
        if result.is_err() {
            let _ = self.db.execute_batch("ROLLBACK");
        }
        result
    }
}
impl<D: Database> ContactStore for SqlStore<D> {
    fn request(&mut self, request: Request) -> Result<Response, LogError> {
        match request {
            Request::List { offset } => Ok(Response::Page(self.page(offset)?)),
            Request::Save { records, offset } => {
                self.save(&records)?;
                Ok(Response::Page(self.page(offset)?))
            }
            Request::Export { include_incomplete } => {
                let mut bytes = Vec::new();
                let count = self.write_adif(include_incomplete, &mut bytes)?;
                Ok(Response::Export(Export {
                    text: String::from_utf8(bytes).map_err(|e| LogError(e.to_string()))?,
                    count,
                }))
            }
        }
    }
}
