//! Native system-SQLite adapter. No bundled SQLite compilation or decoder FFI.
use rusqlite as sqlite;
use std::{path::Path, time::Duration};
use wsjt_log::{Database, LogError, SqlStore, Value};

pub struct Connection(sqlite::Connection);
impl Connection {
    pub fn open(path: &Path) -> Result<Self, LogError> {
        let db = sqlite::Connection::open(path).map_err(error)?;
        db.busy_timeout(Duration::from_secs(2)).map_err(error)?;
        Ok(Self(db))
    }
    pub fn memory() -> Result<Self, LogError> {
        Ok(Self(sqlite::Connection::open_in_memory().map_err(error)?))
    }
}
fn error(e: sqlite::Error) -> LogError {
    LogError(e.to_string())
}
fn values(params: &[Value]) -> Vec<sqlite::types::Value> {
    params
        .iter()
        .map(|p| match p {
            Value::Null => sqlite::types::Value::Null,
            Value::Integer(n) => sqlite::types::Value::Integer(*n),
            Value::Real(n) => sqlite::types::Value::Real(*n),
            Value::Text(s) => sqlite::types::Value::Text(s.clone()),
        })
        .collect()
}
impl Database for Connection {
    fn execute_batch(&mut self, sql: &str) -> Result<(), LogError> {
        self.0.execute_batch(sql).map_err(error)
    }
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<(), LogError> {
        self.0
            .execute(sql, sqlite::params_from_iter(values(params)))
            .map(|_| ())
            .map_err(error)
    }
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, LogError> {
        let mut statement = self.0.prepare(sql).map_err(error)?;
        let columns = statement.column_count();
        let mut rows = statement
            .query(sqlite::params_from_iter(values(params)))
            .map_err(error)?;
        let mut result = Vec::new();
        while let Some(row) = rows.next().map_err(error)? {
            let mut values = Vec::with_capacity(columns);
            for column in 0..columns {
                values.push(match row.get_ref(column).map_err(error)? {
                    sqlite::types::ValueRef::Null => Value::Null,
                    sqlite::types::ValueRef::Integer(n) => Value::Integer(n),
                    sqlite::types::ValueRef::Real(n) => Value::Real(n),
                    sqlite::types::ValueRef::Text(text) => Value::Text(
                        std::str::from_utf8(text)
                            .map_err(|e| LogError(e.to_string()))?
                            .into(),
                    ),
                    sqlite::types::ValueRef::Blob(_) => {
                        return Err(LogError("Unexpected binary contact value".into()));
                    }
                });
            }
            result.push(values);
        }
        Ok(result)
    }
}
pub fn open(path: &Path) -> Result<SqlStore<Connection>, LogError> {
    SqlStore::new(Connection::open(path)?)
}

#[cfg(test)]
mod tests;
