//! Core configuration and subscription logic for the headless service.
//!
//! Extracted business logic remains independent of Tauri and HTTP frameworks.

pub mod backup;
pub mod config;
pub mod enhance;

pub use clash_verge_draft::{Draft, DraftBusy, DraftLayer, DraftTransaction, SharedDraft};
pub use clash_verge_limiter::{Clock, Limiter, SystemClock, SystemLimiter};
