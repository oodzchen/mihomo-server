//! Pure derivation of the tray menu from what the service reports; no I/O.
//! The reading rules mirror the web UI's mode and TUN controls.
use crate::i18n::Strings;
use management_client::view;
use serde_json::{Map, Value};

/// Nodes listed per group; the rest stay in the dashboard.
pub const MAX_NODES: usize = 300;
const MAX_LABEL: usize = 64;
const MODES: [&str; 3] = ["direct", "rule", "global"];
const SELECTABLE: [&str; 3] = ["Selector", "URLTest", "Fallback"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Mode(&'static str),
    Tun(bool),
    Select { group: String, node: String },
    Unfix { group: String },
    TestGroup { group: String },
    Profile { uid: String },
    Core(CoreOp),
    StartService,
    OpenServicePage,
    OpenDashboard,
    Autostart(bool),
    Quit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoreOp {
    Start,
    Stop,
    Restart,
}

impl CoreOp {
    pub fn command(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    Item {
        label: String,
        enabled: bool,
        action: Option<Action>,
    },
    Check {
        label: String,
        enabled: bool,
        checked: bool,
        action: Option<Action>,
    },
    Submenu {
        label: String,
        enabled: bool,
        children: Vec<Entry>,
    },
    Separator,
}

/// What a menu position is; equal shapes are patched in place, not rebuilt.
/// Submenu labels are part of the shape: the appindicator bridge does not
/// export a changed submenu label, so a new one needs a rebuilt menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Item,
    Check,
    Submenu(usize, String),
    Separator,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Normal,
    Tun,
    Offline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuModel {
    pub entries: Vec<Entry>,
    pub icon: Icon,
    pub tooltip: String,
}

impl MenuModel {
    /// Entries in depth-first order; menu ids are positions in this order.
    pub fn flatten(&self) -> Vec<&Entry> {
        fn walk<'a>(entries: &'a [Entry], out: &mut Vec<&'a Entry>) {
            for entry in entries {
                out.push(entry);
                if let Entry::Submenu { children, .. } = entry {
                    walk(children, out);
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.entries, &mut out);
        out
    }

    pub fn shape(&self) -> Vec<Kind> {
        self.flatten()
            .into_iter()
            .map(|entry| match entry {
                Entry::Item { .. } => Kind::Item,
                Entry::Check { .. } => Kind::Check,
                Entry::Submenu { children, label, .. } => Kind::Submenu(children.len(), label.clone()),
                Entry::Separator => Kind::Separator,
            })
            .collect()
    }

    pub fn actions(&self) -> Vec<Option<Action>> {
        self.flatten()
            .into_iter()
            .map(|entry| match entry {
                Entry::Item { action, .. } | Entry::Check { action, .. } => action.clone(),
                _ => None,
            })
            .collect()
    }
}

/// The running instance as last read through the management API.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// Management address shown to the user.
    pub address: String,
    pub status: Value,
    pub access: Value,
    pub user: Value,
    pub proxies: Value,
    pub profiles: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Service {
    Detecting,
    NotInstalled,
    Inactive,
    Unreachable(String),
    Running(Box<Live>),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub service: Service,
    pub working: bool,
    pub last_error: Option<String>,
    pub autostart: bool,
}

impl Live {
    fn phase(&self) -> &str {
        self.status.get("phase").and_then(Value::as_str).unwrap_or("")
    }

    /// The value the core reports while settled and running, the saved one
    /// while stopped or failed, and unknown while it changes.
    fn settled<'a>(&'a self, reported: &str, saved: &str) -> Option<&'a Value> {
        if self.access.get("has_config").and_then(Value::as_bool) != Some(true) {
            return None;
        }
        let core_running = self.access.get("running").and_then(Value::as_bool) == Some(true);
        match self.phase() {
            // Services before v0.1.9 do not report TUN; use the saved value.
            "running" if core_running => self
                .access
                .pointer(reported)
                .filter(|value| !value.is_null())
                .or_else(|| self.access.pointer(saved)),
            "stopped" | "failed" if !core_running => self.access.pointer(saved),
            _ => None,
        }
    }

    pub fn mode(&self) -> Option<&'static str> {
        let mode = self
            .settled("/reported/mode", "/configured/mode")?
            .as_str()?
            .to_ascii_lowercase();
        MODES.into_iter().find(|known| *known == mode)
    }

    pub fn tun(&self) -> Option<bool> {
        self.settled("/reported/tun_enabled", "/tun_enabled")?.as_bool()
    }
}

/// Escape menu text: muda treats `&` as a mnemonic marker.
fn label(text: &str) -> String {
    let mut text: String = text.chars().filter(|c| !c.is_control()).collect();
    if text.chars().count() > MAX_LABEL {
        text = text.chars().take(MAX_LABEL - 1).collect::<String>() + "…";
    }
    text.replace('&', "&&")
}

fn item(label: impl Into<String>, action: Option<Action>) -> Entry {
    Entry::Item {
        label: label.into(),
        enabled: action.is_some(),
        action,
    }
}

fn info(label: impl Into<String>) -> Entry {
    item(label, None)
}

fn check(label: impl Into<String>, checked: bool, enabled: bool, action: Action) -> Entry {
    Entry::Check {
        label: label.into(),
        enabled,
        checked,
        action: Some(action),
    }
}

fn delay(proxies: &Map<String, Value>, name: &str, strings: &Strings) -> String {
    let last = proxies
        .get(name)
        .and_then(|proxy| proxy.get("history"))
        .and_then(Value::as_array)
        .and_then(|history| history.last())
        .and_then(|entry| entry.get("delay"))
        .and_then(Value::as_u64);
    match last {
        None => String::new(),
        Some(0) => format!("  {}", strings.timeout),
        Some(ms) => format!("  {ms} ms"),
    }
}

fn group_menu(group: &view::Group, proxies: &Map<String, Value>, strings: &Strings) -> Entry {
    let selected = group.fixed.as_ref().or(group.now.as_ref());
    let mut children = Vec::new();
    if group.kind != "Selector" {
        children.push(check(
            strings.automatic,
            group.fixed.is_none(),
            true,
            Action::Unfix {
                group: group.name.clone(),
            },
        ));
        children.push(Entry::Separator);
    }
    for node in group.all.iter().take(MAX_NODES) {
        children.push(check(
            label(node) + &delay(proxies, node, strings),
            Some(node) == selected,
            true,
            Action::Select {
                group: group.name.clone(),
                node: node.clone(),
            },
        ));
    }
    if group.all.len() > MAX_NODES {
        let more = strings
            .more_nodes
            .replace("{count}", &(group.all.len() - MAX_NODES).to_string());
        children.push(item(more, Some(Action::OpenDashboard)));
    }
    children.push(Entry::Separator);
    children.push(item(
        strings.test_delay,
        Some(Action::TestGroup {
            group: group.name.clone(),
        }),
    ));
    Entry::Submenu {
        label: format!(
            "{}: {}",
            label(&group.name),
            selected.map_or("-".into(), |node| label(node))
        ),
        enabled: true,
        children,
    }
}

fn phase_label<'a>(phase: &'a str, strings: &'a Strings) -> &'a str {
    match phase {
        "running" => strings.phase_running,
        "stopped" | "shutdown" => strings.phase_stopped,
        "starting" => strings.phase_starting,
        "stopping" => strings.phase_stopping,
        "recovering" => strings.phase_recovering,
        "failed" => strings.phase_failed,
        other => other,
    }
}

fn core_entries(live: &Live, strings: &Strings, entries: &mut Vec<Entry>) {
    let phase = live.phase();
    let mode = live.mode();
    let mode_action = |value: &'static str| (mode != Some(value)).then_some(Action::Mode(value));
    for (value, text) in [
        ("direct", strings.direct),
        ("rule", strings.rule),
        ("global", strings.global),
    ] {
        entries.push(Entry::Check {
            label: text.into(),
            enabled: mode.is_some(),
            checked: mode == Some(value),
            action: mode_action(value),
        });
    }

    let tun = live.tun();
    let state = view::tun_state(&live.access, &live.user);
    let (tun_label, permitted) = match (&state.held_by, state.capable) {
        (Some(holder), _) if tun != Some(true) => (strings.tun_held.replace("{name}", &label(holder)), false),
        (_, Some(false)) if tun != Some(true) => (strings.tun_unavailable.into(), false),
        _ => (strings.tun.into(), true),
    };
    entries.push(check(
        tun_label,
        tun == Some(true),
        permitted && tun.is_some(),
        Action::Tun(tun != Some(true)),
    ));
    entries.push(Entry::Separator);

    let proxies = live
        .proxies
        .get("proxies")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let groups: Vec<view::Group> = view::groups(&proxies)
        .into_iter()
        .filter(|group| SELECTABLE.contains(&group.kind.as_str()))
        .filter(|group| group.name != "GLOBAL" || mode == Some("global"))
        .collect();
    if !groups.is_empty() {
        if let Some(main) = view::default_group(&groups, mode.unwrap_or("rule")) {
            entries.push(group_menu(main, &proxies, strings));
        }
        entries.push(Entry::Submenu {
            label: strings.groups.into(),
            enabled: true,
            children: groups
                .iter()
                .map(|group| group_menu(group, &proxies, strings))
                .collect(),
        });
    }
    let subscriptions = view::subscriptions(&live.profiles);
    if !subscriptions.is_empty() {
        entries.push(Entry::Submenu {
            label: strings.profiles.into(),
            enabled: true,
            children: subscriptions
                .into_iter()
                .map(|subscription| Entry::Check {
                    label: label(&subscription.name),
                    enabled: true,
                    checked: subscription.current,
                    action: (!subscription.current).then_some(Action::Profile { uid: subscription.uid }),
                })
                .collect(),
        });
    }
    entries.push(Entry::Separator);
    let enabled = |allowed: &[&str]| allowed.contains(&phase);
    for (op, text, allowed) in [
        (CoreOp::Start, strings.start_core, &["stopped", "failed"][..]),
        (CoreOp::Stop, strings.stop_core, &["running"][..]),
        (CoreOp::Restart, strings.restart_core, &["running", "failed"][..]),
    ] {
        entries.push(Entry::Item {
            label: text.into(),
            enabled: enabled(allowed),
            action: Some(Action::Core(op)),
        });
    }
}

pub fn derive(snapshot: &Snapshot, strings: &Strings) -> MenuModel {
    let mut entries = Vec::new();
    let mut icon = Icon::Offline;
    let status = match &snapshot.service {
        Service::Detecting => strings.detecting.to_owned(),
        Service::NotInstalled => strings.not_installed.to_owned(),
        Service::Inactive => strings.inactive.to_owned(),
        Service::Unreachable(reason) => strings.unreachable.replace("{reason}", &label(reason)),
        Service::Running(live) => {
            let phase = live.phase();
            if phase == "running" {
                icon = if live.tun() == Some(true) {
                    Icon::Tun
                } else {
                    Icon::Normal
                };
            }
            let version = live.status.get("version").and_then(Value::as_str);
            [Some(live.address.as_str()), Some(phase_label(phase, strings)), version]
                .into_iter()
                .flatten()
                .filter(|part| !part.is_empty())
                .collect::<Vec<_>>()
                .join(" · ")
        }
    };
    entries.push(info(if snapshot.working {
        format!("{status} — {}", strings.working)
    } else {
        status.clone()
    }));
    if let Some(error) = &snapshot.last_error {
        entries.push(info(strings.last_error.replace("{reason}", &label(error))));
    }
    entries.push(Entry::Separator);
    match &snapshot.service {
        Service::Running(live) => core_entries(live, strings, &mut entries),
        Service::NotInstalled => entries.push(item(strings.install, Some(Action::OpenServicePage))),
        Service::Inactive => entries.push(item(strings.start_service, Some(Action::StartService))),
        Service::Detecting | Service::Unreachable(_) => {}
    }
    if !matches!(entries.last(), Some(Entry::Separator)) {
        entries.push(Entry::Separator);
    }
    let running = matches!(snapshot.service, Service::Running(_));
    entries.push(Entry::Item {
        label: strings.open_dashboard.into(),
        enabled: running,
        action: Some(Action::OpenDashboard),
    });
    entries.push(item(strings.service_page, Some(Action::OpenServicePage)));
    entries.push(check(
        strings.autostart,
        snapshot.autostart,
        true,
        Action::Autostart(!snapshot.autostart),
    ));
    entries.push(item(strings.quit, Some(Action::Quit)));
    MenuModel {
        entries,
        icon,
        tooltip: status,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::Language;
    use serde_json::json;

    fn strings() -> &'static Strings {
        Language::En.strings()
    }

    fn live(phase: &str, access: Value) -> Live {
        Live {
            address: "http://127.0.0.1:9090".into(),
            status: json!({"phase": phase, "version": "v1.19.0"}),
            access,
            user: json!({"tun_capable": true}),
            proxies: json!({"proxies": {
                "GLOBAL": {"type": "Selector", "now": "DIRECT", "all": ["DIRECT", "Proxies", "Auto", "Balance"]},
                "Proxies": {"type": "Selector", "now": "Node B", "all": ["Node A", "Node B", "A&B"]},
                "Auto": {"type": "URLTest", "now": "Node A", "all": ["Node A", "Node B"]},
                "Balance": {"type": "LoadBalance", "now": "Node A", "all": ["Node A", "Node B"]},
                "Node A": {"type": "Vless", "history": [{"delay": 42}]},
                "Node B": {"type": "Vless", "history": [{"delay": 0}]},
            }}),
            profiles: json!({"current": "R1", "items": [
                {"uid": "R1", "type": "remote", "name": "Main"},
                {"uid": "L1", "type": "local", "name": "Backup"},
                {"uid": "M", "type": "merge"},
            ]}),
        }
    }

    fn running(mode: &str, tun: bool) -> Live {
        live(
            "running",
            json!({"has_config": true, "running": true, "configured": {"mode": "rule"},
                   "reported": {"mode": mode, "tun_enabled": tun}, "tun_enabled": tun}),
        )
    }

    fn snapshot(service: Service) -> Snapshot {
        Snapshot {
            service,
            working: false,
            last_error: None,
            autostart: false,
        }
    }

    fn find<'a>(model: &'a MenuModel, text: &str) -> &'a Entry {
        model
            .flatten()
            .into_iter()
            .find(|entry| match entry {
                Entry::Item { label, .. } | Entry::Check { label, .. } | Entry::Submenu { label, .. } => {
                    label.starts_with(text)
                }
                Entry::Separator => false,
            })
            .unwrap_or_else(|| panic!("no entry {text}"))
    }

    fn checked(entry: &Entry) -> (bool, bool) {
        match entry {
            Entry::Check { checked, enabled, .. } => (*checked, *enabled),
            other => panic!("not a check item: {other:?}"),
        }
    }

    #[test]
    fn mode_follows_the_core_while_running_and_the_saved_value_while_stopped() {
        let model = derive(
            &snapshot(Service::Running(Box::new(running("Global", false)))),
            strings(),
        );
        assert_eq!(checked(find(&model, "Global")), (true, true));
        assert_eq!(checked(find(&model, "Rule")), (false, true));
        let Entry::Check { action, .. } = find(&model, "Global") else {
            panic!()
        };
        assert_eq!(*action, None, "choosing the current mode is a no-op");

        let stopped = live(
            "stopped",
            json!({"has_config": true, "running": false, "configured": {"mode": "direct"}, "reported": null}),
        );
        let model = derive(&snapshot(Service::Running(Box::new(stopped))), strings());
        assert_eq!(checked(find(&model, "Direct")), (true, true));

        // Transitional phases have no confirmed value: nothing is selectable.
        let starting = live(
            "starting",
            json!({"has_config": true, "running": true, "reported": {"mode": "rule"}}),
        );
        let model = derive(&snapshot(Service::Running(Box::new(starting))), strings());
        assert_eq!(checked(find(&model, "Rule")), (false, false));
        assert_eq!(checked(find(&model, "TUN mode")), (false, false));
    }

    #[test]
    fn older_services_without_reported_tun_use_the_saved_value() {
        let old = live(
            "running",
            json!({"has_config": true, "running": true, "tun_enabled": false, "reported": {"mode": "rule"}}),
        );
        let model = derive(&snapshot(Service::Running(Box::new(old))), strings());
        assert_eq!(checked(find(&model, "TUN mode")), (false, true));
    }

    #[test]
    fn tun_reflects_holder_and_permission() {
        let model = derive(&snapshot(Service::Running(Box::new(running("rule", true)))), strings());
        assert_eq!(checked(find(&model, "TUN mode")), (true, true));
        assert_eq!(model.icon, Icon::Tun);

        let mut held = running("rule", false);
        held.access["tun_holder"] = json!({"name": "alice", "self": false});
        let model = derive(&snapshot(Service::Running(Box::new(held))), strings());
        assert_eq!(checked(find(&model, "TUN mode (held by alice)")), (false, false));

        let mut incapable = running("rule", false);
        incapable.user = json!({"tun_capable": false});
        let model = derive(&snapshot(Service::Running(Box::new(incapable))), strings());
        assert_eq!(checked(find(&model, "TUN mode (not permitted)")), (false, false));
        assert_eq!(model.icon, Icon::Normal);
    }

    #[test]
    fn groups_offer_selectable_nodes_with_delays() {
        let model = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        let Entry::Submenu { label, children, .. } = find(&model, "Proxies: Node B") else {
            panic!()
        };
        assert_eq!(label, "Proxies: Node B");
        assert_eq!(checked(&children[1]), (true, true));
        let Entry::Check { label, action, .. } = &children[0] else {
            panic!()
        };
        assert_eq!(label, "Node A  42 ms");
        assert_eq!(
            *action,
            Some(Action::Select {
                group: "Proxies".into(),
                node: "Node A".into()
            })
        );
        let Entry::Check { label, .. } = &children[2] else {
            panic!()
        };
        assert_eq!(label, "A&&B", "mnemonic marker is escaped");
        let Entry::Check { label, .. } = &children[1] else {
            panic!()
        };
        assert_eq!(label, "Node B  timeout");

        let Entry::Submenu { children: groups, .. } = find(&model, "Proxy groups") else {
            panic!()
        };
        let names: Vec<_> = groups
            .iter()
            .map(|group| match group {
                Entry::Submenu { label, .. } => label.split(':').next().unwrap().to_owned(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(names, ["Proxies", "Auto"], "no LoadBalance, GLOBAL only in global mode");
        let Entry::Submenu { children, .. } = &groups[1] else {
            panic!()
        };
        assert_eq!(
            checked(&children[0]),
            (true, true),
            "URLTest without a pin is automatic"
        );

        let model = derive(
            &snapshot(Service::Running(Box::new(running("global", false)))),
            strings(),
        );
        assert!(matches!(find(&model, "GLOBAL: DIRECT"), Entry::Submenu { .. }));
    }

    #[test]
    fn long_groups_are_capped_and_point_to_the_dashboard() {
        let mut big = running("rule", false);
        let nodes: Vec<String> = (0..MAX_NODES + 5).map(|index| format!("n{index}")).collect();
        big.proxies["proxies"]["Proxies"]["all"] = json!(nodes);
        let model = derive(&snapshot(Service::Running(Box::new(big))), strings());
        let Entry::Submenu { children, .. } = find(&model, "Proxies:") else {
            panic!()
        };
        assert_eq!(children.len(), MAX_NODES + 3);
        assert!(
            matches!(&children[MAX_NODES], Entry::Item { label, action: Some(Action::OpenDashboard), .. }
            if label.starts_with("5 more"))
        );
    }

    #[test]
    fn core_buttons_and_subscriptions_follow_state() {
        let enabled = |model: &MenuModel, text| match find(model, text) {
            Entry::Item { enabled, .. } => *enabled,
            _ => unreachable!(),
        };
        let model = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        assert!(!enabled(&model, "Start core") && enabled(&model, "Stop core") && enabled(&model, "Restart core"));
        let Entry::Submenu { children, .. } = find(&model, "Subscriptions") else {
            panic!()
        };
        assert_eq!(children.len(), 2, "enhancement items are not subscriptions");
        assert_eq!(checked(&children[0]), (true, true));

        let failed = live("failed", json!({"has_config": true, "running": false}));
        let model = derive(&snapshot(Service::Running(Box::new(failed))), strings());
        assert!(enabled(&model, "Start core") && !enabled(&model, "Stop core") && enabled(&model, "Restart core"));
        assert_eq!(model.icon, Icon::Offline);
    }

    #[test]
    fn missing_service_offers_install_or_start() {
        let model = derive(&snapshot(Service::NotInstalled), strings());
        assert!(matches!(
            find(&model, "Install"),
            Entry::Item {
                action: Some(Action::OpenServicePage),
                ..
            }
        ));
        assert!(matches!(
            find(&model, "Open dashboard"),
            Entry::Item { enabled: false, .. }
        ));
        let model = derive(&snapshot(Service::Inactive), strings());
        assert!(matches!(
            find(&model, "Start service"),
            Entry::Item {
                action: Some(Action::StartService),
                ..
            }
        ));
        let mut failing = snapshot(Service::Unreachable("refused".into()));
        failing.last_error = Some("boom".into());
        failing.working = true;
        let model = derive(&failing, strings());
        assert_eq!(model.tooltip, "Cannot reach mihomo-server: refused");
        assert!(matches!(find(&model, "Cannot reach"), Entry::Item { label, .. } if label.ends_with("Working…")));
        assert!(matches!(
            find(&model, "Failed: boom"),
            Entry::Item { enabled: false, .. }
        ));
    }

    #[test]
    fn state_changes_patch_the_menu_and_selections_rebuild_it() {
        let before = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        // Mode, TUN and delays change text and checks only.
        let mut changed = running("direct", true);
        changed.proxies["proxies"]["Node A"]["history"] = json!([{"delay": 99}]);
        let after = derive(&snapshot(Service::Running(Box::new(changed))), strings());
        assert_ne!(before, after);
        assert_eq!(before.shape(), after.shape());
        assert_eq!(before.actions().len(), before.shape().len());

        // A new selection renames the group's submenu; GLOBAL appears in global mode.
        let mut selected = running("rule", false);
        selected.proxies["proxies"]["Proxies"]["now"] = json!("Node A");
        let selected = derive(&snapshot(Service::Running(Box::new(selected))), strings());
        assert_ne!(before.shape(), selected.shape());
        let global = derive(
            &snapshot(Service::Running(Box::new(running("global", false)))),
            strings(),
        );
        assert_ne!(before.shape(), global.shape());
    }
}
