//! Browser-only Leptos shell. Capture and presentation belong here, outside DSP crates.

pub mod annotations;
pub mod contact_log;
pub mod contact_queue;
pub mod conversations;
pub mod geography;
pub mod message_kind;
pub mod open_frequency;
pub mod receiver;
pub mod spectrum;
pub mod transmit;

#[cfg(target_arch = "wasm32")]
mod log_store;

#[cfg(target_arch = "wasm32")]
mod exchange_ui;

#[cfg(target_arch = "wasm32")]
mod transmit_ui;

#[cfg(target_arch = "wasm32")]
mod app;

#[cfg(target_arch = "wasm32")]
mod radio;

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

#[cfg(target_arch = "wasm32")]
mod contact_map;

#[cfg(target_arch = "wasm32")]
mod contact_log_ui;
