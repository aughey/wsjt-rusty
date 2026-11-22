use crate::receiver::{ReceiveRole, Receiver, TimedDecode};
use std::cell::Cell;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = self, js_name = postMessage)]
    fn post_message(message: &JsValue);
}

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn performance_now() -> f64;
}
thread_local! {
    static ROLE: Cell<&'static str> = const { Cell::new("combined") };
    static ATTEMPT_STARTED: Cell<f64> = const { Cell::new(0.0) };
}

fn report_decode_activity(active: bool) {
    let now = performance_now();
    if active {
        ATTEMPT_STARTED.set(now);
    }
    let json = serde_json::to_string(&serde_json::json!({
        "kind": if active { "decode-started" } else { "decode-completed" },
        "attempt": ROLE.get(),
        "elapsed_ms": if active { 0.0 } else { now - ATTEMPT_STARTED.get() },
    }))
    .expect("finite attempt timing");
    post_message(&js_sys::JSON::parse(&json).expect("valid event JSON"));
}

fn report_decode(decode: &TimedDecode) {
    let json = serde_json::to_string(&serde_json::json!({"kind":"decode-found", "decode": decode}))
        .expect("finite decode");
    post_message(&js_sys::JSON::parse(&json).expect("valid decode JSON"));
}

#[wasm_bindgen]
pub struct DecoderEngine(Receiver);

#[wasm_bindgen]
impl DecoderEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(rate: u32, start_utc_ms: f64) -> Result<DecoderEngine, JsValue> {
        Receiver::with_observers(
            rate,
            start_utc_ms,
            report_decode_activity,
            Some(report_decode),
        )
        .map(Self)
        .map_err(|e| JsValue::from_str(&e))
    }

    pub fn set_role(&mut self, role: &str) -> Result<(), JsValue> {
        let (role, label) = match role {
            "slot" => (ReceiveRole::Slot, "slot"),
            "recovery" => (ReceiveRole::Recovery, "recovery"),
            "combined" => (ReceiveRole::Combined, "combined"),
            _ => return Err(JsValue::from_str("invalid decode lane")),
        };
        self.0.set_role(role).map_err(|e| JsValue::from_str(&e))?;
        ROLE.set(label);
        Ok(())
    }

    pub fn set_priority_frequencies(&mut self, frequencies: &[f32]) {
        self.0.set_priority_frequencies(frequencies);
    }

    pub fn push(&mut self, offset: f64, samples: &[f32]) -> Result<String, JsValue> {
        if !offset.is_finite() || offset < 0.0 || offset.fract() != 0.0 {
            return Err(JsValue::from_str("invalid audio offset"));
        }
        let update = self
            .0
            .push(offset as u64, samples)
            .map_err(|e| JsValue::from_str(&e))?;
        serde_json::to_string(&update).map_err(|e| JsValue::from_str(&e.to_string()))
    }

    pub fn finish(&mut self) -> Result<String, JsValue> {
        let update = self.0.finish().map_err(|e| JsValue::from_str(&e))?;
        serde_json::to_string(&update).map_err(|e| JsValue::from_str(&e.to_string()))
    }
}
