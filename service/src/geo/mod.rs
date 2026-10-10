//! Geo databases: pinned seeds, validation, online updates, live replacement and settings.
#[cfg(unix)]
pub(crate) mod cn;
pub(crate) mod dat;
#[cfg(unix)]
pub(crate) mod live;
#[cfg(unix)]
pub mod online;
#[cfg(unix)]
pub(crate) mod resources;
pub mod settings;
#[cfg(unix)]
pub mod update;
pub mod validation;
