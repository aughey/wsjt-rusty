//! Browser-only Leptos shell. Capture and presentation belong here, outside DSP crates.

pub mod annotations;
pub mod conversations;
pub mod message_kind;
pub mod receiver;
pub mod spectrum;

#[cfg(target_arch = "wasm32")]
mod app;

#[cfg(target_arch = "wasm32")]
mod worker;

#[cfg(target_arch = "wasm32")]
mod waterfall;

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(start)]
pub fn start() {
    // The decoder worker instantiates the same module without a DOM or UI.
    if web_sys::window().is_some() {
        leptos::mount::mount_to_body(app::App);
    }
}
