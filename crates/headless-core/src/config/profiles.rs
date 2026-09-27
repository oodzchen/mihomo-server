use serde::{Deserialize, Serialize};
use smartstring::alias::String;

use super::PrfItem;

/// Define the `profiles.yaml` schema
#[derive(Default, Debug, Clone, Deserialize, Serialize)]
pub struct IProfiles {
    pub current: Option<String>,

    pub items: Option<Vec<PrfItem>>,
}
