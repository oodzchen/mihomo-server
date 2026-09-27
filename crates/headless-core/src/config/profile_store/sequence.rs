//! Strict service inputs around the unchanged upstream SeqMap/use_seq operations.
use super::*;
use crate::enhance::seq::SeqMap;
use serde::{Deserialize, Serialize};
use serde_yaml_ng::{Sequence, Value};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequenceKind {
    Rules,
    Proxies,
    Groups,
}

impl SequenceKind {
    /// Validate stored source without changing files or applying enhancement.
    pub fn validate_source(self, yaml: &str) -> Result<()> {
        parse_sequence(yaml, self).map(|_| ())
    }

    pub fn field(self) -> &'static str {
        match self {
            Self::Rules => "rules",
            Self::Proxies => "proxies",
            Self::Groups => "proxy-groups",
        }
    }
}

pub(super) fn parse_sequence(yaml: &str, kind: SequenceKind) -> Result<SeqMap> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Input {
        prepend: Sequence,
        append: Sequence,
        delete: Vec<String>,
    }
    ensure!(yaml.len() <= MAX_CONFIG_BYTES, "sequence enhancement exceeds 8 MiB");
    let Input {
        prepend,
        append,
        delete,
    } = serde_yaml_ng::from_str(yaml).context("sequence enhancement requires prepend, append and delete lists")?;
    ensure!(
        delete.iter().all(|name| !name.trim().is_empty()),
        "deleted names must be nonempty"
    );
    for entry in prepend.iter().chain(&append) {
        match kind {
            SequenceKind::Rules => ensure!(
                entry.as_str().is_some_and(|rule| !rule.trim().is_empty()),
                "rule entries must be nonempty strings"
            ),
            SequenceKind::Proxies | SequenceKind::Groups => {
                let mapping = entry.as_mapping().context("proxy/group entries must be mappings")?;
                for field in ["name", "type"] {
                    ensure!(
                        mapping
                            .get(field)
                            .and_then(Value::as_str)
                            .is_some_and(|value| !value.trim().is_empty()),
                        "proxy/group entries require nonempty name and type"
                    );
                }
            }
        }
    }
    Ok(SeqMap {
        prepend,
        append,
        delete,
    })
}
