//! Synchronous sql.js connection inside the log worker's IndexedDB transaction.
use wasm_bindgen::prelude::*;
use wsjt_log::{ContactStore, Database, LogError, SqlStore, Value};
#[wasm_bindgen(module = "/src/contact-log-sql.js")]
extern "C" {
    #[wasm_bindgen(catch,js_name=contactSqlBatch)]
    fn batch(db: &JsValue, sql: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch,js_name=contactSqlExecute)]
    fn execute(db: &JsValue, sql: &str, params: &str) -> Result<(), JsValue>;
    #[wasm_bindgen(catch,js_name=contactSqlQuery)]
    fn query(db: &JsValue, sql: &str, params: &str) -> Result<String, JsValue>;
}
struct BrowserDatabase(JsValue);
fn error(e: JsValue) -> LogError {
    LogError(
        js_sys::Reflect::get(&e, &"message".into())
            .ok()
            .and_then(|v| v.as_string())
            .or_else(|| e.as_string())
            .unwrap_or_else(|| format!("{e:?}")),
    )
}
impl Database for BrowserDatabase {
    fn execute_batch(&mut self, sql: &str) -> Result<(), LogError> {
        batch(&self.0, sql).map_err(error)
    }
    fn execute(&mut self, sql: &str, params: &[Value]) -> Result<(), LogError> {
        execute(&self.0, sql, &serde_json::to_string(params)?).map_err(error)
    }
    fn query(&mut self, sql: &str, params: &[Value]) -> Result<Vec<Vec<Value>>, LogError> {
        Ok(serde_json::from_str(
            &query(&self.0, sql, &serde_json::to_string(params)?).map_err(error)?,
        )?)
    }
}
#[wasm_bindgen]
pub struct ContactLogEngine(SqlStore<BrowserDatabase>);
#[wasm_bindgen]
impl ContactLogEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(database: JsValue) -> Result<ContactLogEngine, JsValue> {
        SqlStore::new(BrowserDatabase(database))
            .map(Self)
            .map_err(|e| JsValue::from_str(&e.to_string()))
    }
    pub fn request(&mut self, json: &str) -> Result<String, JsValue> {
        let result = (|| {
            let request = serde_json::from_str(json)
                .map_err(|e| LogError(format!("Invalid contact request: {e}")))?;
            let response = self.0.request(request)?;
            Ok::<_, LogError>(serde_json::to_string(&response)?)
        })();
        result.map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
