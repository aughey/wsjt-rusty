//! Shared decoder adapters and bounded audio streaming for FT8 and WSPR.
pub mod decoder;

// Preserve the original library paths while allowing direct use of each crate.
pub use wsjt_audio as audio;
pub use wsjt_ft8 as ft8;
pub use wsjt_ft8::{ldpc, message, waveform};
pub use wsjt_wspr as wspr;
