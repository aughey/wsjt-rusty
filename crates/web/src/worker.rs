use crate::receiver::Receiver;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = self, js_name = postMessage)]
    fn post_message(message: &JsValue);
}

fn report_decode_activity(active: bool) {
    let event = js_sys::Object::new();
    let kind = if active {
        "decode-started"
    } else {
        "decode-completed"
    };
    js_sys::Reflect::set(&event, &"kind".into(), &kind.into()).expect("plain event object");
    post_message(&event);
}

#[wasm_bindgen]
pub struct DecoderEngine(Receiver);

#[wasm_bindgen]
impl DecoderEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(rate: u32, start_utc_ms: f64) -> Result<DecoderEngine, JsValue> {
        Receiver::with_decode_activity(rate, start_utc_ms, report_decode_activity)
            .map(Self)
            .map_err(|e| JsValue::from_str(&e))
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
