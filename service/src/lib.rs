pub mod backup;
pub mod connection_settings;
pub mod core_manager;
pub mod core_release;
pub mod core_upgrade;
mod dat_validation;
#[cfg(unix)]
mod geo_live;
#[cfg(unix)]
pub mod geo_online;
#[cfg(unix)]
mod geo_resources;
pub mod geo_settings;
#[cfg(unix)]
pub mod geo_update;
pub mod geo_validation;
pub mod management;
mod native_tun;
mod proxy_access;
pub mod remote;
pub mod resource_inventory;
pub mod resources;
pub mod script;
mod selections;
mod settings_readback;
pub mod shutdown;
mod validation;

pub use clash_verge_i18n as i18n;
pub use clash_verge_signal as signal;
