pub mod backup;
pub mod cli;
pub mod connection_settings;
pub mod core_manager;
pub mod core_release;
pub mod core_upgrade;
pub mod geo;
mod http3;
pub mod management;
#[cfg(target_os = "linux")]
pub mod multi_user;
mod native_tun;
mod proxy_access;
pub mod proxy_probe;
pub mod remote;
pub mod resource_inventory;
pub mod resources;
pub mod script;
mod secure_fs;
mod selections;
pub mod service_control;
mod settings_readback;
pub mod shutdown;
#[cfg(target_os = "linux")]
pub mod tun_exec;
#[cfg(target_os = "linux")]
pub mod tun_lock;
pub mod unlock;
mod validation;

/// Release builds take the version from the tag.
pub const VERSION: &str = match option_env!("MIHOMO_SERVER_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub use clash_verge_i18n as i18n;
pub use clash_verge_signal as signal;
