//! Selection reconciliation extracted from the pinned upstream profiles.rs.

use headless_core::config::PrfSelected;
use mihomo_client::models::{Proxies, ProxyType};
use smartstring::alias::String;
use std::collections::{HashMap, HashSet};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct SelectedNodesPlan {
    pub selected: Vec<PrfSelected>,
    pub activations: Vec<(String, String)>,
    pub repaired_count: usize,
}

fn node_is_available(available_nodes: &[std::string::String], node: &str) -> bool {
    available_nodes.iter().any(|available| available == node)
}

pub(crate) fn selected_nodes_need_confirmation(selected: &[PrfSelected], proxies: &Proxies) -> bool {
    selected.iter().any(|selected_item| {
        let (Some(group_name), Some(node)) = (&selected_item.name, &selected_item.now) else {
            return false;
        };
        let Some(group) = proxies.proxies.get(group_name.as_str()) else {
            return true;
        };
        let Some(available_nodes) = group.all.as_deref().filter(|nodes| !nodes.is_empty()) else {
            return true;
        };
        !node_is_available(available_nodes, node)
    })
}

pub(crate) fn reconcile_selected_nodes(
    selected: &[PrfSelected],
    previous: Option<&Proxies>,
    proxies: &Proxies,
) -> SelectedNodesPlan {
    let mut plan = SelectedNodesPlan {
        selected: Vec::with_capacity(selected.len()),
        activations: Vec::new(),
        repaired_count: 0,
    };
    let mut seen_groups = HashSet::new();
    let mut unique_selected = selected
        .iter()
        .rev()
        .filter(|item| item.name.as_ref().is_some_and(|name| seen_groups.insert(name.clone())))
        .collect::<Vec<_>>();
    unique_selected.reverse();
    plan.repaired_count += selected.len() - unique_selected.len();

    for selected_item in unique_selected {
        let (Some(group_name), Some(node)) = (&selected_item.name, &selected_item.now) else {
            plan.repaired_count += 1;
            continue;
        };
        let Some(group) = proxies.proxies.get(group_name.as_str()) else {
            if previous.is_some_and(|snapshot| !snapshot.proxies.contains_key(group_name.as_str())) {
                plan.repaired_count += 1;
            } else {
                plan.selected.push(selected_item.clone());
            }
            continue;
        };
        let Some(available_nodes) = group.all.as_deref().filter(|nodes| !nodes.is_empty()) else {
            // Provider-backed groups can be temporarily incomplete immediately after a reload.
            plan.selected.push(selected_item.clone());
            continue;
        };
        let is_selectable_group = matches!(
            &group.proxy_type,
            ProxyType::Selector | ProxyType::URLTest | ProxyType::Fallback | ProxyType::LoadBalance
        );
        if !is_selectable_group {
            let preferred_node = group
                .now
                .as_deref()
                .filter(|current| node_is_available(available_nodes, current))
                .or_else(|| node_is_available(available_nodes, node).then_some(node.as_str()));
            if let Some(preferred_node) = preferred_node {
                if preferred_node != node.as_str() {
                    plan.repaired_count += 1;
                }
                plan.selected.push(PrfSelected {
                    name: Some(group_name.clone()),
                    now: Some(preferred_node.into()),
                });
            } else {
                plan.repaired_count += 1;
            }
            continue;
        }

        if node_is_available(available_nodes, node) {
            plan.selected.push(selected_item.clone());
            if group.now.as_deref() != Some(node.as_str()) {
                plan.activations.push((group_name.clone(), node.clone()));
            }
            continue;
        }

        let missing_was_confirmed = previous
            .and_then(|snapshot| snapshot.proxies.get(group_name.as_str()))
            .and_then(|group| group.all.as_deref())
            .filter(|nodes| !nodes.is_empty())
            .is_some_and(|nodes| !node_is_available(nodes, node));
        if !missing_was_confirmed {
            plan.selected.push(selected_item.clone());
            continue;
        }

        plan.repaired_count += 1;
        if let Some(current_node) = group
            .now
            .as_deref()
            .filter(|current| node_is_available(available_nodes, current))
        {
            plan.selected.push(PrfSelected {
                name: Some(group_name.clone()),
                now: Some(current_node.into()),
            });
        }
    }

    plan
}

pub(crate) fn remaining_activations(
    activations: &[(String, String)],
    completed: &HashMap<String, String>,
) -> Vec<(String, String)> {
    activations
        .iter()
        .filter(|(group_name, node)| completed.get(group_name) != Some(node))
        .cloned()
        .collect()
}

pub(crate) fn unsettled_selections(selected: &[PrfSelected], proxies: &Proxies) -> Vec<String> {
    selected
        .iter()
        .filter_map(|item| {
            let (Some(group_name), Some(node)) = (&item.name, &item.now) else {
                return None;
            };
            match proxies.proxies.get(group_name.as_str()) {
                Some(group) if group.now.as_deref() == Some(node.as_str()) => None,
                _ => Some(group_name.clone()),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mihomo_client::models::Proxy;
    fn selected(group: &str, node: &str) -> PrfSelected {
        PrfSelected {
            name: Some(group.into()),
            now: Some(node.into()),
        }
    }

    fn proxies(groups: Vec<(&str, &[&str], Option<&str>)>) -> Proxies {
        Proxies {
            proxies: groups
                .into_iter()
                .map(|(name, all, now)| {
                    (
                        name.to_owned(),
                        Proxy {
                            name: name.to_owned(),
                            all: Some(all.iter().map(|node| (*node).to_owned()).collect()),
                            now: now.map(str::to_owned),
                            proxy_type: ProxyType::Selector,
                            ..Proxy::default()
                        },
                    )
                })
                .collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn a_group_the_core_has_not_loaded_is_still_unsettled() {
        // The regression this pins. A provider-backed group is present but empty until its
        // provider loads, which on a cold start is exactly when restoring runs. Reconciling
        // produces no activation for it, so a restore that stopped after one re-check reported
        // success having left the group on the first entry of its `proxies:` list.
        let saved = vec![selected("provider-group", "saved")];
        let unloaded = proxies(vec![("provider-group", &[], None)]);

        assert!(
            reconcile_selected_nodes(&saved, Some(&unloaded), &unloaded)
                .activations
                .is_empty(),
            "nothing can be activated while the group is empty"
        );
        assert_eq!(
            unsettled_selections(&saved, &unloaded),
            vec![String::from("provider-group")],
            "so the restore must keep looking rather than call it done"
        );
    }

    #[test]
    fn a_group_the_core_is_already_on_is_settled() {
        let saved = vec![selected("Proxy", "Node A")];
        let loaded = proxies(vec![("Proxy", &["Node A", "Node B"], Some("Node A"))]);

        assert!(
            unsettled_selections(&saved, &loaded).is_empty(),
            "a group already on its node needs nothing, even though it produces no activation"
        );
    }

    #[test]
    fn a_group_on_the_wrong_node_is_unsettled() {
        // Covers a `select` that failed transiently: the group is loaded, the node exists, and
        // the core is simply not on it. Retrying is the only thing that fixes that.
        let saved = vec![selected("Proxy", "Node A")];
        let wrong = proxies(vec![("Proxy", &["Node A", "Node B"], Some("Node B"))]);

        assert_eq!(unsettled_selections(&saved, &wrong), vec![String::from("Proxy")]);
    }

    #[test]
    fn a_record_without_a_group_or_node_is_not_waited_on() {
        // A malformed record cannot be satisfied by waiting, and holding the settle loop open
        // for it would delay giving up on everything else.
        let malformed = vec![
            PrfSelected {
                name: Some("Proxy".into()),
                now: None,
            },
            PrfSelected {
                name: None,
                now: Some("Node A".into()),
            },
        ];

        assert!(unsettled_selections(&malformed, &proxies(vec![])).is_empty());
    }

    #[test]
    fn a_group_missing_from_both_snapshots_is_dropped_from_the_records() {
        // This is the pruning a profile switch wants and a core start must not do: the record is
        // gone from the plan, and `persist_reconciled_selected` writes the plan back. Pinned here
        // because it is why `SelectionRepair` exists — the predicate is right, the question is
        // only who is entitled to act on it.
        let saved = vec![selected("provider-group", "saved")];
        let empty = proxies(vec![]);

        let plan = reconcile_selected_nodes(&saved, Some(&empty), &empty);

        assert!(
            plan.selected.is_empty(),
            "a group absent from both looks invalid, so the record is dropped"
        );
        assert_eq!(plan.repaired_count, 1, "and dropping it counts as a repair");
    }

    #[test]
    fn a_group_missing_from_only_the_second_snapshot_is_kept() {
        // Absent once is not evidence: only a group that was already absent when the first
        // snapshot was taken is treated as gone.
        let saved = vec![selected("provider-group", "saved")];
        let had_it = proxies(vec![("provider-group", &["saved"], Some("saved"))]);
        let lost_it = proxies(vec![]);

        let plan = reconcile_selected_nodes(&saved, Some(&had_it), &lost_it);

        assert_eq!(plan.selected, saved);
        assert_eq!(plan.repaired_count, 0);
    }

    #[test]
    fn keeps_valid_selection_and_activates_when_needed() {
        let saved = vec![selected("group", "saved")];
        let plan = reconcile_selected_nodes(
            &saved,
            None,
            &proxies(vec![("group", &["current", "saved"], Some("current"))]),
        );

        assert_eq!(plan.selected, saved);
        assert_eq!(plan.activations, vec![("group".into(), "saved".into())]);
        assert_eq!(plan.repaired_count, 0);
    }

    #[test]
    fn replaces_missing_node_with_valid_current_node() {
        let snapshot = proxies(vec![("group", &["current"], Some("current"))]);
        let plan = reconcile_selected_nodes(&[selected("group", "renamed-node")], Some(&snapshot), &snapshot);

        assert_eq!(plan.selected, vec![selected("group", "current")]);
        assert!(plan.activations.is_empty());
        assert_eq!(plan.repaired_count, 1);
    }

    #[test]
    fn validates_membership_in_group_not_global_existence() {
        let snapshot = proxies(vec![
            ("group", &["current"], Some("current")),
            ("other-node", &[], None),
        ]);
        let plan = reconcile_selected_nodes(&[selected("group", "other-node")], Some(&snapshot), &snapshot);

        assert_eq!(plan.selected, vec![selected("group", "current")]);
        assert!(plan.activations.is_empty());
        assert_eq!(plan.repaired_count, 1);
    }

    #[test]
    fn does_not_activate_non_selectable_groups() {
        let snapshot = Proxies {
            proxies: HashMap::from([(
                "group".to_owned(),
                Proxy {
                    name: "group".to_owned(),
                    all: Some(vec!["current".to_owned(), "saved".to_owned()]),
                    now: Some("current".to_owned()),
                    proxy_type: ProxyType::Direct,
                    ..Proxy::default()
                },
            )]),
        };

        let plan = reconcile_selected_nodes(&[selected("group", "saved")], None, &snapshot);

        assert_eq!(plan.selected, vec![selected("group", "current")]);
        assert!(plan.activations.is_empty());
        assert_eq!(plan.repaired_count, 1);
    }

    #[test]
    fn removes_selection_when_group_or_fallback_is_invalid() {
        let snapshot = proxies(vec![("group", &["valid"], Some("invalid-current"))]);
        let plan = reconcile_selected_nodes(
            &[
                selected("missing-group", "node"),
                selected("group", "missing-node"),
                PrfSelected::default(),
            ],
            Some(&snapshot),
            &snapshot,
        );

        assert!(plan.selected.is_empty());
        assert!(plan.activations.is_empty());
        assert_eq!(plan.repaired_count, 3);
    }

    #[test]
    fn preserves_selection_until_missing_node_is_confirmed() {
        let saved = vec![selected("group", "saved")];
        let incomplete = proxies(vec![("group", &[], None)]);
        let complete = proxies(vec![("group", &["current"], Some("current"))]);

        let incomplete_plan = reconcile_selected_nodes(&saved, None, &incomplete);
        let one_snapshot_plan = reconcile_selected_nodes(&saved, None, &complete);

        assert_eq!(incomplete_plan.selected, saved);
        assert_eq!(incomplete_plan.repaired_count, 0);
        assert_eq!(one_snapshot_plan.selected, saved);
        assert_eq!(one_snapshot_plan.repaired_count, 0);
    }

    #[test]
    fn recovers_when_group_appears_in_second_snapshot() {
        let saved = vec![selected("group", "saved")];
        let incomplete = Proxies::default();
        let complete = proxies(vec![("group", &["current", "saved"], Some("current"))]);

        let plan = reconcile_selected_nodes(&saved, Some(&incomplete), &complete);

        assert_eq!(plan.selected, saved);
        assert_eq!(plan.activations, vec![("group".into(), "saved".into())]);
        assert_eq!(plan.repaired_count, 0);
    }

    #[test]
    fn keeps_last_selection_for_duplicate_group_entries() {
        let saved = vec![selected("group", "old"), selected("group", "new")];
        let snapshot = proxies(vec![("group", &["old", "new"], Some("old"))]);

        let plan = reconcile_selected_nodes(&saved, None, &snapshot);

        assert_eq!(plan.selected, vec![selected("group", "new")]);
        assert_eq!(plan.activations, vec![("group".into(), "new".into())]);
        assert_eq!(plan.repaired_count, 1);
    }

    #[test]
    fn activates_valid_nodes_before_confirming_invalid_records() {
        let saved = vec![selected("valid-group", "saved"), selected("stale-group", "missing")];
        let first_snapshot = proxies(vec![
            ("valid-group", &["current", "saved"], Some("current")),
            ("stale-group", &["fallback"], Some("fallback")),
        ]);

        assert!(selected_nodes_need_confirmation(&saved, &first_snapshot));
        let immediate_plan = reconcile_selected_nodes(&saved, None, &first_snapshot);

        assert_eq!(immediate_plan.selected, saved);
        assert_eq!(immediate_plan.activations, vec![("valid-group".into(), "saved".into())]);
        assert_eq!(immediate_plan.repaired_count, 0);
    }

    #[test]
    fn skips_only_activations_that_already_succeeded() {
        let activations = vec![
            ("first-group".into(), "saved".into()),
            ("second-group".into(), "new".into()),
            ("first-group".into(), "replacement".into()),
        ];
        let completed = HashMap::from([("first-group".into(), "saved".into())]);

        assert_eq!(
            remaining_activations(&activations, &completed),
            vec![
                ("second-group".into(), "new".into()),
                ("first-group".into(), "replacement".into()),
            ]
        );
    }
}
