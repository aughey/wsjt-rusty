//! Framework-independent FT8 receive sessions and display models.
//! No device, window, browser, or network dependencies. PCM ownership belongs to
//! the caller; protocol algorithms remain in the existing headless libraries.
pub mod annotations;
pub mod contact_log;
pub mod contact_queue;
pub mod conversations;
pub mod geography;
pub mod history;
pub mod message_kind;
pub mod open_frequency;
pub mod receiver;
pub mod spectrum;
pub mod transmit;
