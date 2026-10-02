pub mod backup;
pub mod connection_settings;
pub mod core_manager;
pub mod core_release;
pub mod core_upgrade;
pub mod geo;
pub mod management;
#[cfg(target_os = "linux")]
pub mod multi_user;
mod native_tun;
mod proxy_access;
pub mod remote;
pub mod resource_inventory;
pub mod resources;
pub mod script;
mod secure_fs;
mod selections;
mod settings_readback;
pub mod shutdown;
mod validation;

pub use clash_verge_i18n as i18n;
pub use clash_verge_signal as signal;
