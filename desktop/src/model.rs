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
    StartService,
    StopService,
    RestartService,
    OpenServicePage,
    OpenDashboard,
    RestartClient,
    Quit,
}

/// What the client is doing in the background; one task at a time.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Task {
    Idle,
    Installing,
    Starting,
    Stopping,
    Restarting,
    /// A management command such as a mode switch or a latency test.
    Command,
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
    /// The service's own version (not the core's, which `status` reports).
    pub service_version: Option<String>,
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
    pub task: Task,
    pub last_error: Option<String>,
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

fn proxy_entries(live: &Live, strings: &Strings, entries: &mut Vec<Entry>) {
    let mode = live.mode();
    entries.push(Entry::Submenu {
        label: match mode {
            Some(current) => format!("{}: {}", strings.mode, mode_label(current, strings)),
            None => strings.mode.into(),
        },
        enabled: true,
        children: MODES
            .into_iter()
            .map(|value| Entry::Check {
                label: mode_label(value, strings).into(),
                enabled: mode.is_some(),
                checked: mode == Some(value),
                action: (mode != Some(value)).then_some(Action::Mode(value)),
            })
            .collect(),
    });

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
    if !matches!(entries.last(), Some(Entry::Separator)) {
        entries.push(Entry::Separator);
    }
}

fn mode_label<'a>(mode: &str, strings: &'a Strings) -> &'a str {
    match mode {
        "direct" => strings.direct,
        "global" => strings.global,
        _ => strings.rule,
    }
}

/// The service's state in a few words: what a running task is doing, else
/// what systemd and the management API report. Never the core's state.
fn service_status<'a>(snapshot: &Snapshot, strings: &'a Strings) -> &'a str {
    match (snapshot.task, &snapshot.service) {
        (Task::Installing, _) => strings.service_installing,
        (Task::Starting, _) => strings.service_starting,
        (Task::Stopping, _) => strings.service_stopping,
        (Task::Restarting, _) => strings.service_restarting,
        (_, Service::Detecting) => strings.service_detecting,
        (_, Service::NotInstalled) => strings.service_not_installed,
        (_, Service::Inactive) => strings.service_stopped,
        (_, Service::Unreachable(_)) => strings.service_unreachable,
        (_, Service::Running(_)) => strings.service_running,
    }
}

/// Details that would widen the menu go to the tooltip (where the platform
/// shows one); failures are also sent as notifications.
fn tooltip(snapshot: &Snapshot, status: &str, strings: &Strings) -> String {
    let mut lines = vec![status.to_owned()];
    if let Service::Unreachable(reason) = &snapshot.service {
        lines.push(reason.clone());
    }
    if snapshot.task == Task::Command {
        lines.push(strings.working.into());
    }
    if let Some(error) = &snapshot.last_error {
        lines.push(format!("{}: {error}", strings.failed));
    }
    lines.join("\n")
}

pub fn derive(snapshot: &Snapshot, strings: &Strings) -> MenuModel {
    let status = service_status(snapshot, strings);
    // One compact line; no separator below it, which would add more height.
    let mut entries = vec![info(status)];
    let mut icon = Icon::Offline;
    if let Service::Running(live) = &snapshot.service {
        if live.phase() == "running" {
            icon = if live.tun() == Some(true) {
                Icon::Tun
            } else {
                Icon::Normal
            };
        }
        proxy_entries(live, strings, &mut entries);
    }
    // Falls back to the status page while the service is not reachable.
    entries.push(item(strings.open_dashboard, Some(Action::OpenDashboard)));
    let service = |label: &str, action| Entry::Item {
        label: label.into(),
        enabled: snapshot.task == Task::Idle,
        action: Some(action),
    };
    match &snapshot.service {
        Service::Running(_) | Service::Unreachable(_) => {
            entries.push(service(strings.restart_service, Action::RestartService));
            entries.push(service(strings.stop_service, Action::StopService));
        }
        Service::Inactive => entries.push(service(strings.start_service, Action::StartService)),
        Service::NotInstalled => entries.push(service(strings.install, Action::OpenServicePage)),
        Service::Detecting => {}
    }
    entries.push(Entry::Separator);
    entries.push(item(strings.restart_client, Some(Action::RestartClient)));
    entries.push(item(strings.quit, Some(Action::Quit)));
    MenuModel {
        entries,
        icon,
        tooltip: tooltip(snapshot, status, strings),
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
            service_version: Some("v0.1.9".into()),
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
            task: Task::Idle,
            last_error: None,
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
    fn mode_is_a_submenu_of_checked_choices() {
        let model = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        let Entry::Submenu { label, children, .. } = find(&model, "Proxy mode") else {
            panic!()
        };
        assert_eq!(label, "Proxy mode: Rule");
        let states: Vec<_> = children.iter().map(checked).collect();
        assert_eq!(states, [(false, true), (true, true), (false, true)]);

        let unknown = live("starting", json!({"has_config": true, "running": true}));
        let model = derive(&snapshot(Service::Running(Box::new(unknown))), strings());
        assert!(matches!(find(&model, "Proxy mode"), Entry::Submenu { label, .. } if label == "Proxy mode"));
    }

    #[test]
    fn the_header_is_one_line_of_service_state() {
        let header = |model: &MenuModel| match &model.entries[..2] {
            [
                Entry::Item {
                    label, enabled: false, ..
                },
                Entry::Submenu { .. } | Entry::Item { .. },
            ] => label.clone(),
            other => panic!("not a single info line: {other:?}"),
        };
        let mut failed = snapshot(Service::Running(Box::new(live(
            "failed",
            json!({"has_config": true, "running": false}),
        ))));
        failed.last_error = Some("a long reason that must not widen the menu".into());
        let model = derive(&failed, strings());
        // The core failed, but the service is running: no core state here.
        assert_eq!(header(&model), "Service running");
        assert!(model.tooltip.contains("a long reason"));
        assert_eq!(model.icon, Icon::Offline);

        let mut installing = snapshot(Service::NotInstalled);
        installing.task = Task::Installing;
        assert_eq!(header(&derive(&installing, strings())), "Installing service…");
        let model = derive(&snapshot(Service::Unreachable("refused".into())), strings());
        assert_eq!(header(&model), "Service unreachable");
        assert_eq!(model.tooltip, "Service unreachable\nrefused");
    }

    #[test]
    fn service_lifecycle_follows_its_state_and_the_core_is_not_offered() {
        let action = |model: &MenuModel, text| match find(model, text) {
            Entry::Item { action, enabled, .. } => (action.clone(), *enabled),
            other => panic!("not an item: {other:?}"),
        };
        let model = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        assert_eq!(action(&model, "Restart service"), (Some(Action::RestartService), true));
        assert_eq!(action(&model, "Stop service"), (Some(Action::StopService), true));
        assert!(
            !model.flatten().iter().any(|entry| matches!(entry,
                Entry::Item { label, .. } | Entry::Check { label, .. } | Entry::Submenu { label, .. }
                if label.to_lowercase().contains("core"))),
            "the menu manages the desktop client and the service, never the core"
        );
        let Entry::Submenu { children, .. } = find(&model, "Subscriptions") else {
            panic!()
        };
        assert_eq!(children.len(), 2, "enhancement items are not subscriptions");
        assert_eq!(checked(&children[0]), (true, true));

        let model = derive(&snapshot(Service::NotInstalled), strings());
        assert_eq!(action(&model, "Install"), (Some(Action::OpenServicePage), true));
        assert_eq!(action(&model, "Open dashboard"), (Some(Action::OpenDashboard), true));
        let mut starting = snapshot(Service::Inactive);
        assert_eq!(
            action(&derive(&starting, strings()), "Start service"),
            (Some(Action::StartService), true)
        );
        starting.task = Task::Starting;
        assert!(!action(&derive(&starting, strings()), "Start service").1);
        let model = derive(&snapshot(Service::Unreachable("refused".into())), strings());
        assert_eq!(action(&model, "Restart service").0, Some(Action::RestartService));
    }

    #[test]
    fn state_changes_patch_the_menu_and_selections_rebuild_it() {
        let before = derive(&snapshot(Service::Running(Box::new(running("rule", false)))), strings());
        // TUN and delays change text and checks only.
        let mut changed = running("rule", true);
        changed.proxies["proxies"]["Node A"]["history"] = json!([{"delay": 99}]);
        let after = derive(&snapshot(Service::Running(Box::new(changed))), strings());
        assert_ne!(before, after);
        assert_eq!(before.shape(), after.shape());
        assert_eq!(before.actions().len(), before.shape().len());

        // A new mode or selection renames a submenu; GLOBAL appears in global mode.
        let direct = derive(
            &snapshot(Service::Running(Box::new(running("direct", false)))),
            strings(),
        );
        assert_ne!(before.shape(), direct.shape());
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

    #[test]
    fn restart_client_sits_right_above_quit_in_every_state() {
        for service in [
            Service::Detecting,
            Service::NotInstalled,
            Service::Inactive,
            Service::Running(Box::new(running("rule", false))),
        ] {
            let model = derive(&snapshot(service), strings());
            let tail: Vec<_> = model.entries.iter().rev().take(2).collect();
            assert!(matches!(tail[0], Entry::Item { label, action: Some(Action::Quit), .. } if label == "Quit client"));
            assert!(
                matches!(tail[1], Entry::Item { label, enabled: true, action: Some(Action::RestartClient) } if label == "Restart client")
            );
        }
    }
}
