//! Browser transport ownership. Receive processing runs in the decoder worker.
use js_sys::{Function, Promise};
use wasm_bindgen::{JsValue, prelude::*};
use wasm_bindgen_futures::JsFuture;
use web_sys::AnalyserNode;

#[wasm_bindgen(module = "/src/capture-runtime.js")]
extern "C" {
    #[wasm_bindgen(js_name = openCapture)]
    fn open_capture(callback: &Function, token: &JsValue) -> Promise;
    #[wasm_bindgen(js_name = createCancellation)]
    fn create_cancellation() -> JsValue;
    #[wasm_bindgen(js_name = cancelCapture)]
    fn cancel_capture(token: &JsValue);
    #[wasm_bindgen(js_name = captureAnalyser)]
    fn capture_analyser(capture: &JsValue) -> AnalyserNode;
    #[wasm_bindgen(js_name = captureRate)]
    fn capture_rate(capture: &JsValue) -> f32;
    #[wasm_bindgen(js_name = captureTime)]
    fn capture_time(capture: &JsValue) -> f64;
    #[wasm_bindgen(js_name = stopCapture)]
    fn stop_capture(capture: &JsValue) -> Promise;
    #[wasm_bindgen(js_name = abortCapture)]
    fn abort_capture(capture: &JsValue);
}

pub struct Cancellation(JsValue);

impl Cancellation {
    pub fn new() -> Self {
        Self(create_cancellation())
    }

    pub fn cancel(&self) {
        cancel_capture(&self.0);
    }
}

pub struct Capture {
    handle: JsValue,
    pub analyser: AnalyserNode,
    // Kept alive until the runtime has stopped invoking it.
    _callback: Closure<dyn FnMut(String)>,
}

impl Capture {
    pub async fn open(
        callback: Closure<dyn FnMut(String)>,
        token: &Cancellation,
    ) -> Result<Self, JsValue> {
        let handle =
            JsFuture::from(open_capture(callback.as_ref().unchecked_ref(), &token.0)).await?;
        Ok(Self {
            analyser: capture_analyser(&handle),
            handle,
            _callback: callback,
        })
    }

    pub fn sample_rate(&self) -> f32 {
        capture_rate(&self.handle)
    }

    pub fn time_ms(&self) -> f64 {
        capture_time(&self.handle)
    }

    pub async fn finish(&self) -> Result<(), JsValue> {
        JsFuture::from(stop_capture(&self.handle)).await.map(|_| ())
    }

    pub fn abort(&self) {
        abort_capture(&self.handle);
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.abort();
    }
}

pub fn error_message(error: JsValue) -> String {
    let detail = error.as_string().unwrap_or_else(|| format!("{error:?}"));
    format!(
        "Could not start audio: {detail}. Check microphone permission and availability; use localhost or HTTPS."
    )
}
