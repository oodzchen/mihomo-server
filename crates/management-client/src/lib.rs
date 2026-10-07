//! Client side of the mihomo-server management API, shared by the
//! `mihomo-server` command line and the desktop client: locating an instance,
//! running authenticated commands and reading their responses.
mod api;
mod endpoint;
#[cfg(feature = "events")]
pub mod events;
pub mod installation;
pub mod view;

pub use api::{Api, CommandError};
pub use endpoint::{
    Endpoint, ServiceState, UNIT, argument, default_token_file, discover_running, from_arguments, locate,
    management_port, parse_show, service_state,
};
pub use view::current_mode;
