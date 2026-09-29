//! Subscription schemas and service-owned runtime configuration storage.

pub mod dns;
pub mod operations;
mod prfitem;
pub mod profile_store;
mod profiles;
pub mod remote;
pub mod resource_paths;
pub mod resources;
pub mod runtime;
pub mod settings;

pub use operations::{DelayTestQuery, ProviderAction, ProviderOperationReceipt};
pub use prfitem::{PrfExtra, PrfItem, PrfOption, PrfSelected};
pub use profiles::IProfiles;
pub use resources::*;
