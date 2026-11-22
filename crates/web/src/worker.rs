use crate::receiver::Receiver;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct DecoderEngine(Receiver);

#[wasm_bindgen]
impl DecoderEngine {
    #[wasm_bindgen(constructor)]
    pub fn new(rate: u32, start_utc_ms: f64) -> Result<DecoderEngine, JsValue> {
        Receiver::new(rate, start_utc_ms)
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
