//! `mihomo-server COMMAND`: day-to-day control of the user's instance.
//!
//! The same executable is the service (`mihomo-server serve`, or the legacy
//! form starting with a service option); see [`route`].
mod api;
mod system;
mod view;

use anyhow::{Context as _, Result, bail};
use api::{Api, Endpoint};
use clap::{Arg, ArgAction, ArgMatches, Command, value_parser};
use serde_json::{Map, Value, json};
use std::{
    ffi::OsString,
    io::{IsTerminal as _, Write as _},
    path::PathBuf,
};

/// What an invocation runs.
#[derive(Debug, PartialEq, Eq)]
pub enum Route {
    /// The management service, with its own arguments (program name first).
    Serve(Vec<OsString>),
    Cli,
}

/// Options that belong to the command line rather than to the service.
const CLI_OPTIONS: [&str; 7] = ["-h", "--help", "-V", "--version", "--json", "--api", "--token-file"];

pub fn route(arguments: &[OsString]) -> Route {
    let Some(first) = arguments.get(1).and_then(|first| first.to_str()) else {
        return Route::Cli;
    };
    if first == "serve" {
        let mut service = vec![arguments[0].clone()];
        service.extend_from_slice(&arguments[2..]);
        return Route::Serve(service);
    }
    let option = first.split_once('=').map_or(first, |(name, _)| name);
    if first.starts_with('-') && !CLI_OPTIONS.contains(&option) {
        return Route::Serve(arguments.to_vec());
    }
    Route::Cli
}

#[derive(Debug)]
struct UsageError(String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn usage(message: impl Into<String>) -> anyhow::Error {
    UsageError(message.into()).into()
}

fn operand(name: &'static str, value_name: &'static str, help: &'static str) -> Arg {
    Arg::new(name).value_name(value_name).help(help)
}

fn yes() -> Arg {
    Arg::new("yes")
        .short('y')
        .long("yes")
        .action(ArgAction::SetTrue)
        .help("Do not ask for confirmation")
}

pub fn command() -> Command {
    Command::new("mihomo-server")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Control your headless Mihomo instance")
        .arg_required_else_help(true)
        .subcommand_required(true)
        .arg(
            Arg::new("json")
                .long("json")
                .global(true)
                .action(ArgAction::SetTrue)
                .help("Print machine-readable JSON instead of text"),
        )
        .arg(
            Arg::new("api")
                .long("api")
                .global(true)
                .value_name("URL")
                .help("Management API of a service outside systemd [env: MIHOMO_SERVER_API]"),
        )
        .arg(
            Arg::new("token-file")
                .long("token-file")
                .global(true)
                .value_name("FILE")
                .value_parser(value_parser!(PathBuf))
                .help("Management token file [env: MIHOMO_SERVER_TOKEN_FILE]"),
        )
        .subcommand(Command::new("status").about("Show service, core, subscription, mode, TUN and addresses"))
        .subcommand(Command::new("start").about("Start your instance and wait until it is ready"))
        .subcommand(Command::new("stop").about("Stop your instance"))
        .subcommand(Command::new("restart").about("Restart your instance and wait until it is ready"))
        .subcommand(
            Command::new("enable")
                .about("Start your instance now and at boot (init is an alias)")
                .alias("init"),
        )
        .subcommand(Command::new("disable").about("Stop your instance and do not start it at boot; data is kept"))
        .subcommand(Command::new("info").about("Show management address, token, proxy port and file paths"))
        .subcommand(Command::new("token").about("Print your management token"))
        .subcommand(
            Command::new("logs")
                .about("Follow service logs; extra options go to journalctl (e.g. -n 100)")
                .arg(
                    Arg::new("journalctl")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true)
                        .value_name("OPTION"),
                ),
        )
        .subcommand(
            Command::new("sub")
                .about("List, select, update, add and remove subscriptions")
                .visible_alias("subscription")
                .alias("profile")
                .subcommand(
                    Command::new("list")
                        .about("List subscriptions (default)")
                        .visible_alias("ls"),
                )
                .subcommand(
                    Command::new("use")
                        .about("Use a subscription (asks when omitted)")
                        .visible_alias("select")
                        .arg(operand("subscription", "SUBSCRIPTION", "Name, UID or list number")),
                )
                .subcommand(
                    Command::new("update")
                        .about("Download remote subscriptions again (all when omitted)")
                        .visible_alias("refresh")
                        .arg(operand("subscription", "SUBSCRIPTION", "Name, UID or list number").num_args(0..)),
                )
                .subcommand(
                    Command::new("add")
                        .about("Import a subscription URL or a local YAML file")
                        .visible_alias("import")
                        .arg(operand("source", "SOURCE", "http(s) URL or YAML file").required(true))
                        .arg(
                            Arg::new("name")
                                .short('n')
                                .long("name")
                                .value_name("NAME")
                                .help("Display name"),
                        )
                        .arg(
                            Arg::new("use")
                                .long("use")
                                .action(ArgAction::SetTrue)
                                .help("Use it after importing"),
                        ),
                )
                .subcommand(
                    Command::new("remove")
                        .about("Delete a subscription")
                        .visible_alias("rm")
                        .arg(operand("subscription", "SUBSCRIPTION", "Name, UID or list number").required(true))
                        .arg(yes()),
                ),
        )
        .subcommand(
            Command::new("proxy")
                .about("List groups and nodes, test delays and select nodes")
                .visible_alias("node")
                .subcommand(
                    Command::new("list")
                        .about("List proxy groups, or the nodes of GROUP (default)")
                        .visible_alias("ls")
                        .arg(operand("group", "GROUP", "Group name or list number")),
                )
                .subcommand(
                    Command::new("select")
                        .about("Select NODE in GROUP (default: the main group; asks when omitted)")
                        .visible_alias("use")
                        .arg(operand("group", "GROUP", "Group, when two operands are given"))
                        .arg(operand("node", "NODE", "Node name, unique part of it, or list number")),
                )
                .subcommand(
                    Command::new("test")
                        .about("Test the delay of a group's nodes or of one node (default: the main group)")
                        .visible_alias("delay")
                        .arg(operand("target", "TARGET", "Group or node"))
                        .arg(
                            Arg::new("url")
                                .long("url")
                                .value_name("URL")
                                .help("Test URL [default: http://www.gstatic.com/generate_204]"),
                        )
                        .arg(
                            Arg::new("timeout")
                                .long("timeout")
                                .value_name("MS")
                                .value_parser(value_parser!(u32).range(100..=60_000))
                                .default_value("5000")
                                .help("Per-node timeout in milliseconds"),
                        ),
                )
                .subcommand(
                    Command::new("unfix")
                        .about("Let an automatic group (url-test/fallback) choose again")
                        .arg(operand("group", "GROUP", "Group name").required(true)),
                ),
        )
        .subcommand(Command::new("mode").about("Show or set the proxy mode").arg(operand(
            "mode",
            "MODE",
            "rule, global or direct",
        )))
        .subcommand(
            Command::new("tun")
                .about("Show or switch the system-wide TUN")
                .arg(operand("state", "STATE", "on or off")),
        )
        .subcommand(
            Command::new("core")
                .about("Show or upgrade the Mihomo core")
                .subcommand(Command::new("version").about("Show the installed core version (default)"))
                .subcommand(
                    Command::new("update")
                        .about("Upgrade the core to the latest release")
                        .visible_alias("upgrade")
                        .arg(
                            Arg::new("alpha")
                                .long("alpha")
                                .action(ArgAction::SetTrue)
                                .help("Use the alpha channel"),
                        )
                        .arg(
                            Arg::new("force")
                                .long("force")
                                .action(ArgAction::SetTrue)
                                .help("Reinstall even when up to date"),
                        ),
                ),
        )
        .subcommand(
            Command::new("update")
                .about("Update mihomo-server to the latest release (runs the installer)")
                .visible_alias("upgrade")
                .arg(
                    Arg::new("force")
                        .long("force")
                        .action(ArgAction::SetTrue)
                        .help("Reinstall even when up to date"),
                ),
        )
        .subcommand(
            Command::new("uninstall")
                .about("Remove the shared installation for all users (runs the installer)")
                .arg(
                    Arg::new("purge")
                        .long("purge")
                        .action(ArgAction::SetTrue)
                        .help("Also delete every user's data and configuration"),
                )
                .arg(yes()),
        )
        .subcommand(
            Command::new("purge")
                .about("Delete your own instance data, configuration and slot")
                .arg(yes()),
        )
        .subcommand(
            Command::new("serve")
                .about("Run the management service in the foreground (see: mihomo-server serve --help)")
                .disable_help_flag(true)
                .arg(
                    Arg::new("options")
                        .num_args(0..)
                        .trailing_var_arg(true)
                        .allow_hyphen_values(true),
                ),
        )
        .after_help(
            "Run 'mihomo-server COMMAND --help' for details. Without --api, commands act on \
             your systemd user instance of the shared installation.\n\nExamples:\n  \
             mihomo-server status\n  mihomo-server sub use amy\n  mihomo-server proxy test\n  \
             mihomo-server proxy select '日本 03'\n  mihomo-server mode global\n  mihomo-server tun on",
        )
}

/// Run the command line; returns the process exit status.
pub fn main(arguments: Vec<OsString>) -> i32 {
    // Like other utilities, end quietly when a reader such as `head` exits.
    #[cfg(unix)]
    // SAFETY: restoring the default disposition before any thread is started.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
    let matches = match command().try_get_matches_from(arguments) {
        Ok(matches) => matches,
        Err(error) => {
            let _ = error.print();
            return error.exit_code();
        }
    };
    let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
        Ok(runtime) => runtime,
        Err(error) => {
            eprintln!("mihomo-server: {error}");
            return 1;
        }
    };
    match runtime.block_on(dispatch(&matches)) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("mihomo-server: {error:#}");
            if error.is::<UsageError>() { 2 } else { 1 }
        }
    }
}

struct Context {
    json: bool,
    api: Option<String>,
    token_file: Option<PathBuf>,
}

impl Context {
    fn new(matches: &ArgMatches) -> Self {
        let environment = |name| std::env::var(name).ok().filter(|value: &String| !value.is_empty());
        Self {
            json: matches.get_flag("json"),
            api: matches
                .get_one::<String>("api")
                .cloned()
                .or_else(|| environment("MIHOMO_SERVER_API")),
            token_file: matches
                .get_one::<PathBuf>("token-file")
                .cloned()
                .or_else(|| environment("MIHOMO_SERVER_TOKEN_FILE").map(PathBuf::from)),
        }
    }

    fn connect(&self) -> Result<Api> {
        if let Some(api) = &self.api {
            let token = self.token_file.clone().unwrap_or_else(api::default_token_file);
            return Api::connect(Endpoint::explicit(api, token)?);
        }
        refuse_root()?;
        let mut endpoint = api::discover_running(&api::service_state()?)?;
        if let Some(token) = &self.token_file {
            endpoint.token_file = token.clone();
        }
        Api::connect(endpoint)
    }

    fn print_json(&self, value: &Value) -> Result<()> {
        println!("{}", serde_json::to_string_pretty(value)?);
        Ok(())
    }
}

fn refuse_root() -> Result<()> {
    #[cfg(unix)]
    // SAFETY: geteuid has no preconditions and cannot fail.
    if unsafe { libc::geteuid() } == 0 {
        bail!("root has no instance; run this as the user who owns it");
    }
    Ok(())
}

async fn dispatch(matches: &ArgMatches) -> Result<i32> {
    let context = Context::new(matches);
    let (name, arguments) = matches.subcommand().context("missing command")?;
    match name {
        "start" | "stop" | "restart" if context.api.is_some() => {
            let api = context.connect()?;
            let value = api.command(name, json!({})).await?;
            if context.json {
                context.print_json(&value)?;
            } else {
                println!("core: {}", value.get("phase").and_then(Value::as_str).unwrap_or("done"));
            }
            Ok(0)
        }
        "start" | "stop" | "restart" | "enable" | "disable" | "info" | "token" => system::exec_helper(&[name.into()]),
        "logs" => {
            let mut forwarded: Vec<OsString> = vec!["logs".into()];
            forwarded.extend(
                arguments
                    .get_many::<String>("journalctl")
                    .into_iter()
                    .flatten()
                    .map(OsString::from),
            );
            system::exec_helper(&forwarded)
        }
        "purge" => {
            confirm(
                "Delete your instance data, subscriptions, settings and token?",
                arguments.get_flag("yes"),
            )?;
            system::exec_helper(&["purge".into(), "--yes".into()])
        }
        "status" => status(&context).await,
        "sub" => subscriptions(&context, arguments).await,
        "proxy" => proxies(&context, arguments).await,
        "mode" => mode(&context, arguments).await,
        "tun" => tun(&context, arguments).await,
        "core" => core(&context, arguments).await,
        "update" => system::update(arguments.get_flag("force")).await,
        "uninstall" => {
            let purge = arguments.get_flag("purge");
            confirm(
                if purge {
                    "Remove mihomo-server for all users and delete everyone's data?"
                } else {
                    "Remove mihomo-server for all users (their data is kept)?"
                },
                arguments.get_flag("yes"),
            )?;
            system::uninstall(purge).await
        }
        "serve" => Err(usage("serve must be the first argument")),
        _ => Err(usage(format!("unknown command {name}"))),
    }
}

fn confirm(question: &str, yes: bool) -> Result<()> {
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        return Err(usage("confirmation required; pass --yes"));
    }
    eprint!("{question} [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if matches!(answer.trim().to_lowercase().as_str(), "y" | "yes") {
        Ok(())
    } else {
        bail!("cancelled")
    }
}

/// Ask for one of `items` on a terminal; `what` names the operand otherwise.
fn choose<'a>(items: &'a [String], current: Option<&str>, what: &str) -> Result<&'a str> {
    if !std::io::stdin().is_terminal() {
        return Err(usage(format!("specify a {what}")));
    }
    anyhow::ensure!(!items.is_empty(), "nothing to choose from");
    for (index, item) in items.iter().enumerate() {
        let mark = if Some(item.as_str()) == current { '*' } else { ' ' };
        eprintln!("{mark}{:>4}  {item}", index + 1);
    }
    eprint!("Choose a {what} [1-{}, empty to cancel]: ", items.len());
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    let answer = answer.trim();
    if answer.is_empty() {
        bail!("cancelled");
    }
    view::pick_name(items, answer, what)
}

fn label(name: &str, value: impl std::fmt::Display) {
    println!("{:<14}{value}", format!("{name}:"));
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or(0)
}

fn current_mode(access: &Value) -> String {
    access
        .pointer("/reported/mode")
        .or_else(|| access.pointer("/configured/mode"))
        .and_then(Value::as_str)
        .unwrap_or("rule")
        .to_lowercase()
}

fn tun_summary(access: &Value, user: &Value) -> String {
    let enabled = access.get("tun_enabled").and_then(Value::as_bool).unwrap_or(false);
    let holder = access.get("tun_holder").filter(|holder| !holder.is_null());
    let device = user.get("tun_device").and_then(Value::as_str);
    if let Some(holder) = holder
        && holder.get("self").and_then(Value::as_bool) != Some(true)
    {
        let name = holder.get("name").and_then(Value::as_str).unwrap_or("another user");
        return format!("off (the system-wide TUN is held by {name})");
    }
    if enabled {
        let system = user.get("tun_system").and_then(Value::as_bool) == Some(true);
        return format!(
            "on{}{}",
            device.map(|device| format!(" (device {device}")).unwrap_or_default(),
            match (device.is_some(), system) {
                (true, true) => ", system-wide)",
                (true, false) => ")",
                _ => "",
            }
        );
    }
    if user.get("tun_capable").and_then(Value::as_bool) == Some(false) {
        return "off (unavailable: ask the administrator to add you to group mihomo-tun)".into();
    }
    "off".into()
}

fn mixed_port(access: &Value) -> Option<u64> {
    access
        .get("ports")?
        .as_array()?
        .iter()
        .find(|port| port.get("key").and_then(Value::as_str) == Some("mixed-port"))?
        .get("actual")?
        .as_u64()
        .filter(|port| *port != 0)
}

async fn status(context: &Context) -> Result<i32> {
    let service = if context.api.is_none() {
        refuse_root()?;
        let state = api::service_state()?;
        if !state.running() {
            if context.json {
                context.print_json(&json!({"service": {"active": state.active, "enabled": state.enabled}}))?;
            } else {
                label("service", format!("{} ({})", state.active, state.enabled));
                println!("Start it with: mihomo-server start");
            }
            // LSB convention: program is not running.
            return Ok(3);
        }
        Some(state)
    } else {
        None
    };
    let api = context.connect()?;
    let core = api.command("status", json!({})).await?;
    let access = api.command("proxy_access", json!({})).await.unwrap_or(Value::Null);
    let user = api.command("multi_user", json!({})).await.unwrap_or(Value::Null);
    let profiles = api.command("profiles", json!({})).await.unwrap_or(Value::Null);
    let subscription = view::subscriptions(&profiles).into_iter().find(|item| item.current);
    if context.json {
        return context
            .print_json(&json!({
                "service": service.as_ref().map(|state| json!({
                    "active": state.active, "sub": state.sub, "enabled": state.enabled,
                })),
                "core": core,
                "proxy_access": access,
                "multi_user": user,
                "subscription": subscription.as_ref().map(|item| json!({"uid": item.uid, "name": item.name})),
                "management_url": api.endpoint.management_url,
            }))
            .map(|()| 0);
    }
    if let Some(state) = &service {
        label(
            "service",
            format!("{} ({}), {}", state.active, state.sub, state.enabled),
        );
    }
    let phase = core.get("phase").and_then(Value::as_str).unwrap_or("unknown");
    match core.get("version").and_then(Value::as_str) {
        Some(version) => label("core", format!("{phase}, Mihomo {version}")),
        None => label("core", phase),
    }
    for key in ["error", "selection_error"] {
        if let Some(error) = core.get(key).and_then(Value::as_str) {
            label("error", error);
        }
    }
    label(
        "subscription",
        subscription.map_or_else(|| "(none)".to_owned(), |item| item.name),
    );
    if !access.is_null() {
        label("mode", current_mode(&access));
        label("tun", tun_summary(&access, &user));
        if let Some(port) = mixed_port(&access) {
            let lan = access.pointer("/configured/allow_lan").and_then(Value::as_bool) == Some(true);
            label(
                "proxy",
                format!("HTTP/SOCKS 127.0.0.1:{port}{}", if lan { " (LAN allowed)" } else { "" }),
            );
        }
    }
    label("manage", &api.endpoint.management_url);
    Ok(0)
}

async fn subscriptions(context: &Context, matches: &ArgMatches) -> Result<i32> {
    let api = context.connect()?;
    let profiles = api.command("profiles", json!({})).await?;
    let items = view::subscriptions(&profiles);
    let find = |query: &str| view::pick(&items, query, "subscription", |item| vec![&item.name, &item.uid]);
    match matches.subcommand().unwrap_or(("list", matches)) {
        ("list", _) => {
            if context.json {
                return context.print_json(&profiles).map(|()| 0);
            }
            if items.is_empty() {
                println!("No subscriptions; add one with: mihomo-server sub add URL");
                return Ok(0);
            }
            let now = now();
            println!("     #  TYPE    UPDATED      USED / TOTAL           EXPIRES     NAME");
            for (index, item) in items.iter().enumerate() {
                let (usage, expires) = match item.usage {
                    Some((used, total, expire)) => (
                        format!("{} / {}", view::bytes(used), view::bytes(total)),
                        if expire == 0 { "-".into() } else { view::date(expire) },
                    ),
                    None => ("-".into(), "-".into()),
                };
                println!(
                    "{} {:>4}  {:<6}  {:<11}  {:<21}  {:<10}  {}",
                    if item.current { '*' } else { ' ' },
                    index + 1,
                    item.kind,
                    item.updated
                        .map_or_else(|| "-".into(), |updated| view::age(now, updated)),
                    usage,
                    expires,
                    item.name
                );
            }
            Ok(0)
        }
        ("use", arguments) => {
            let item = match arguments.get_one::<String>("subscription") {
                Some(query) => find(query)?,
                None => {
                    let names: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
                    let current = items.iter().find(|item| item.current).map(|item| item.name.as_str());
                    let name = choose(&names, current, "subscription")?;
                    find(name)?
                }
            };
            let value = api.command("select_profile", json!({"uid": item.uid})).await?;
            if context.json {
                return context.print_json(&value).map(|()| 0);
            }
            println!("Using subscription {}", item.name);
            Ok(0)
        }
        ("update", arguments) => {
            let selected: Vec<&view::Subscription> = match arguments.get_many::<String>("subscription") {
                Some(queries) => queries.map(|query| find(query)).collect::<Result<_>>()?,
                None => items.iter().filter(|item| item.kind == "remote").collect(),
            };
            if selected.is_empty() {
                println!("No remote subscriptions to update");
                return Ok(0);
            }
            let mut failed = 0;
            for item in selected {
                if item.kind != "remote" {
                    eprintln!("{}: local file, nothing to download", item.name);
                    continue;
                }
                match api.command("refresh_profile", json!({"uid": item.uid})).await {
                    Ok(_) => println!("Updated {}", item.name),
                    Err(error) => {
                        failed += 1;
                        eprintln!("{}: {error:#}", item.name);
                    }
                }
            }
            Ok(i32::from(failed > 0))
        }
        ("add", arguments) => {
            let source = arguments.get_one::<String>("source").context("missing source")?;
            let name = arguments.get_one::<String>("name").cloned();
            let value = if source.starts_with("https://") || source.starts_with("http://") {
                api.command("import_remote_profile", json!({"url": source, "name": name}))
                    .await?
            } else {
                let path = PathBuf::from(source);
                let yaml = std::fs::read_to_string(&path).with_context(|| format!("cannot read {source}"))?;
                let name = name.unwrap_or_else(|| {
                    path.file_stem()
                        .map_or_else(|| source.clone(), |stem| stem.to_string_lossy().into_owned())
                });
                api.command("import_profile", json!({"name": name, "yaml": yaml}))
                    .await?
            };
            let uid = value
                .get("uid")
                .or_else(|| value.pointer("/item/uid"))
                .and_then(Value::as_str)
                .map(str::to_owned);
            let display = value
                .get("name")
                .or_else(|| value.pointer("/item/name"))
                .and_then(Value::as_str)
                .or(uid.as_deref())
                .unwrap_or(source)
                .to_owned();
            if arguments.get_flag("use") {
                let uid = uid
                    .clone()
                    .context("the service did not return the new subscription's UID")?;
                api.command("select_profile", json!({"uid": uid})).await?;
            }
            if context.json {
                return context.print_json(&value).map(|()| 0);
            }
            println!(
                "Added subscription {display}{}",
                if arguments.get_flag("use") { " and using it" } else { "" }
            );
            Ok(0)
        }
        ("remove", arguments) => {
            let item = find(
                arguments
                    .get_one::<String>("subscription")
                    .context("missing subscription")?,
            )?;
            confirm(
                &format!("Delete subscription {}?", item.name),
                arguments.get_flag("yes"),
            )?;
            let value = api.command("delete_profile", json!({"uid": item.uid})).await?;
            if context.json {
                return context.print_json(&value).map(|()| 0);
            }
            println!("Removed subscription {}", item.name);
            Ok(0)
        }
        (other, _) => Err(usage(format!("unknown sub command {other}"))),
    }
}

struct ProxyView {
    proxies: Map<String, Value>,
    groups: Vec<view::Group>,
    mode: String,
}

impl ProxyView {
    async fn load(api: &Api) -> Result<Self> {
        let value = api.command("proxies", json!({})).await?;
        let proxies = value
            .get("proxies")
            .and_then(Value::as_object)
            .cloned()
            .context("unexpected proxies response")?;
        let access = api.command("proxy_access", json!({})).await.unwrap_or(Value::Null);
        Ok(Self {
            groups: view::groups(&proxies),
            mode: current_mode(&access),
            proxies,
        })
    }

    fn group(&self, query: &str) -> Result<&view::Group> {
        view::pick(&self.groups, query, "group", |group| vec![&group.name])
    }

    fn default_group(&self) -> Result<&view::Group> {
        view::default_group(&self.groups, &self.mode).context("the configuration has no proxy groups")
    }

    fn is_group(&self, name: &str) -> bool {
        self.groups.iter().any(|group| group.name == name)
    }

    /// Exact (case-insensitive) group name, without index or substring rules.
    fn named_group(&self, query: &str) -> Option<&view::Group> {
        self.groups.iter().find(|group| group.name.eq_ignore_ascii_case(query))
    }
}

async fn proxies(context: &Context, matches: &ArgMatches) -> Result<i32> {
    let api = context.connect()?;
    let state = ProxyView::load(&api).await?;
    match matches.subcommand().unwrap_or(("list", matches)) {
        ("list", arguments) => {
            let Some(query) = arguments.try_get_one::<String>("group").ok().flatten() else {
                if context.json {
                    return context
                        .print_json(&json!(
                            state
                                .groups
                                .iter()
                                .map(|group| json!({
                                    "name": group.name, "type": group.kind, "now": group.now,
                                    "fixed": group.fixed, "all": group.all,
                                }))
                                .collect::<Vec<_>>()
                        ))
                        .map(|()| 0);
                }
                let main = state.default_group().ok().map(|group| group.name.as_str());
                println!("     #  TYPE         GROUP -> SELECTED");
                for (index, group) in state.groups.iter().enumerate() {
                    println!(
                        "{} {:>4}  {:<11}  {} -> {}{}",
                        if Some(group.name.as_str()) == main { '*' } else { ' ' },
                        index + 1,
                        group.kind,
                        group.name,
                        group.now.as_deref().unwrap_or("-"),
                        if group.fixed.is_some() { " (fixed)" } else { "" }
                    );
                }
                println!(
                    "\n* main group in {} mode; nodes: mihomo-server proxy list GROUP",
                    state.mode
                );
                return Ok(0);
            };
            let group = state.group(query)?;
            if context.json {
                let nodes: Vec<Value> = group
                    .all
                    .iter()
                    .map(|name| json!({"name": name, "delay": view::last_delay(&state.proxies, name)}))
                    .collect();
                return context
                    .print_json(&json!({"name": group.name, "type": group.kind, "now": group.now, "nodes": nodes}))
                    .map(|()| 0);
            }
            println!(
                "{} ({}) -> {}",
                group.name,
                group.kind,
                group.now.as_deref().unwrap_or("-")
            );
            println!("     #  DELAY     NAME");
            for (index, name) in group.all.iter().enumerate() {
                println!(
                    "{} {:>4}  {:<8}  {name}{}",
                    if Some(name) == group.now.as_ref() { '*' } else { ' ' },
                    index + 1,
                    view::delay_label(view::last_delay(&state.proxies, name)),
                    if state.is_group(name) { "  [group]" } else { "" }
                );
            }
            Ok(0)
        }
        ("select", arguments) => {
            let first = arguments.get_one::<String>("group");
            let second = arguments.get_one::<String>("node");
            let (group, node) = match (first, second) {
                (Some(group), Some(node)) => {
                    let group = state.group(group)?;
                    (group, view::pick_name(&group.all, node, "node")?)
                }
                (Some(node), None) => {
                    let main = state.default_group()?;
                    match view::pick_name(&main.all, node, "node") {
                        Ok(found) => (main, found),
                        Err(error) => match state.named_group(node) {
                            Some(group) => (group, choose(&group.all, group.now.as_deref(), "node")?),
                            None => return Err(error.context(format!("in group {}", main.name))),
                        },
                    }
                }
                _ => {
                    let main = state.default_group()?;
                    eprintln!("{} ({}):", main.name, main.kind);
                    (main, choose(&main.all, main.now.as_deref(), "node")?)
                }
            };
            if matches!(group.kind.as_str(), "LoadBalance" | "Relay") {
                bail!("{} ({}) does not support manual selection", group.name, group.kind);
            }
            let value = api
                .command("select_node", json!({"group": group.name, "node": node}))
                .await?;
            if context.json {
                return context.print_json(&value).map(|()| 0);
            }
            println!("{} -> {node}", group.name);
            Ok(0)
        }
        ("test", arguments) => {
            let url = arguments.get_one::<String>("url");
            let timeout = *arguments.get_one::<u32>("timeout").context("missing timeout")?;
            let target = arguments.get_one::<String>("target");
            let group = match target {
                None => Some(state.default_group()?),
                Some(query) => state.named_group(query),
            };
            let leaves: Vec<String> = state
                .proxies
                .keys()
                .filter(|name| !state.is_group(name))
                .cloned()
                .collect();
            let (group, node) = match (group, target) {
                (Some(group), _) => (Some(group), None),
                (None, Some(query)) => match view::pick_name(&leaves, query, "node") {
                    Ok(node) => (None, Some(node)),
                    Err(error) => (Some(state.group(query).map_err(|_| error)?), None),
                },
                (None, None) => unreachable!("no target resolves to the default group"),
            };
            if let Some(node) = node {
                let result = api
                    .command("delay_proxy", json!({"name": node, "url": url, "timeout": timeout}))
                    .await;
                if context.json {
                    let value = match &result {
                        Ok(value) => value.clone(),
                        Err(error) => json!({"error": format!("{error:#}")}),
                    };
                    context.print_json(&json!({"name": node, "result": value}))?;
                    return Ok(i32::from(result.is_err()));
                }
                return match result {
                    Ok(value) => {
                        println!(
                            "{}  {node}",
                            view::delay_label(value.get("delay").and_then(Value::as_u64))
                        );
                        Ok(0)
                    }
                    Err(error) => {
                        println!("timeout  {node}  ({error:#})");
                        Ok(1)
                    }
                };
            }
            let group = group.context("no test target")?;
            if !context.json {
                eprintln!(
                    "Testing {} nodes of {} (timeout {timeout} ms)...",
                    group.all.len(),
                    group.name
                );
            }
            let results = api
                .command(
                    "delay_group",
                    json!({"group": group.name, "url": url, "timeout": timeout}),
                )
                .await?;
            if context.json {
                return context.print_json(&results).map(|()| 0);
            }
            let mut rows: Vec<(Option<u64>, &String)> = group
                .all
                .iter()
                .map(|name| {
                    (
                        results.get(name).and_then(Value::as_u64).filter(|delay| *delay > 0),
                        name,
                    )
                })
                .collect();
            rows.sort_by_key(|(delay, _)| delay.unwrap_or(u64::MAX));
            println!("  DELAY     NAME");
            for (delay, name) in rows {
                println!(
                    "{} {:<8}  {name}",
                    if Some(name) == group.now.as_ref() { '*' } else { ' ' },
                    view::delay_label(Some(delay.unwrap_or(0)))
                );
            }
            Ok(0)
        }
        ("unfix", arguments) => {
            let group = state.group(arguments.get_one::<String>("group").context("missing group")?)?;
            let value = api.command("unfix_node", json!({"group": group.name})).await?;
            if context.json {
                return context.print_json(&value).map(|()| 0);
            }
            println!("{} chooses automatically again", group.name);
            Ok(0)
        }
        (other, _) => Err(usage(format!("unknown proxy command {other}"))),
    }
}

async fn mode(context: &Context, matches: &ArgMatches) -> Result<i32> {
    let api = context.connect()?;
    let Some(requested) = matches.get_one::<String>("mode") else {
        let access = api.command("proxy_access", json!({})).await?;
        let mode = current_mode(&access);
        if context.json {
            return context.print_json(&json!({"mode": mode})).map(|()| 0);
        }
        println!("{mode}");
        return Ok(0);
    };
    let mode = view::parse_mode(requested).map_err(|error| usage(error.to_string()))?;
    let value = api.command("set_proxy_mode", json!({"mode": mode})).await?;
    if context.json {
        return context.print_json(&value).map(|()| 0);
    }
    println!("Proxy mode: {mode}");
    Ok(0)
}

async fn tun(context: &Context, matches: &ArgMatches) -> Result<i32> {
    let api = context.connect()?;
    if let Some(requested) = matches.get_one::<String>("state") {
        let enabled = view::parse_switch(requested).map_err(|error| usage(error.to_string()))?;
        if !context.json {
            eprintln!("{} TUN...", if enabled { "Enabling" } else { "Disabling" });
        }
        api.command("set_tun_enabled", json!({"enabled": enabled})).await?;
    }
    let access = api.command("proxy_access", json!({})).await?;
    let user = api.command("multi_user", json!({})).await.unwrap_or(Value::Null);
    if context.json {
        return context
            .print_json(&json!({
                "enabled": access.get("tun_enabled"),
                "holder": access.get("tun_holder"),
                "capable": user.get("tun_capable"),
                "device": user.get("tun_device"),
            }))
            .map(|()| 0);
    }
    label("tun", tun_summary(&access, &user));
    Ok(0)
}

async fn core(context: &Context, matches: &ArgMatches) -> Result<i32> {
    let api = context.connect()?;
    match matches.subcommand().unwrap_or(("version", matches)) {
        ("version", _) => {
            // Only bundle-managed cores have an installed version; `--mihomo` cores do not.
            let installed = api
                .command("installed_core_version", json!({}))
                .await
                .ok()
                .and_then(|version| version.as_str().map(str::to_owned));
            let status = api.command("status", json!({})).await?;
            let running = status.get("version").and_then(Value::as_str);
            if context.json {
                return context
                    .print_json(&json!({"installed": installed, "running": running}))
                    .map(|()| 0);
            }
            match (installed.as_deref(), running) {
                (Some(installed), Some(running)) if running != installed => {
                    println!("Mihomo {installed} (running {running}; restart to switch)")
                }
                (Some(installed), Some(_)) => println!("Mihomo {installed} (running)"),
                (Some(installed), None) => println!("Mihomo {installed} (not running)"),
                (None, Some(running)) => println!("Mihomo {running} (running, external core)"),
                (None, None) => bail!("the core version is unknown while the core is not running"),
            }
            Ok(0)
        }
        ("update", arguments) => {
            let alpha = arguments.get_flag("alpha");
            if !context.json {
                eprintln!(
                    "Checking the latest {} core release...",
                    if alpha { "alpha" } else { "stable" }
                );
            }
            let report = api
                .command(
                    if alpha {
                        "upgrade_alpha_core"
                    } else {
                        "upgrade_clash_core"
                    },
                    json!({"force": arguments.get_flag("force")}),
                )
                .await?;
            if context.json {
                return context.print_json(&report).map(|()| 0);
            }
            let text = |key| report.get(key).and_then(Value::as_str).unwrap_or("?");
            if report.get("upgraded").and_then(Value::as_bool) == Some(true) {
                println!("Core upgraded: {} -> {}", text("from"), text("to"));
            } else {
                println!("Core is up to date ({})", text("from"));
            }
            Ok(0)
        }
        (other, _) => Err(usage(format!("unknown core command {other}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route_of(line: &str) -> Route {
        let arguments: Vec<OsString> = line.split(' ').map(OsString::from).collect();
        route(&arguments)
    }

    #[test]
    fn service_options_and_serve_run_the_service() {
        assert_eq!(
            route_of("mihomo-server --data-dir d --listen 127.0.0.1:1"),
            Route::Serve(
                ["mihomo-server", "--data-dir", "d", "--listen", "127.0.0.1:1"]
                    .map(OsString::from)
                    .into()
            )
        );
        assert_eq!(
            route_of("mihomo-server serve --data-dir d"),
            Route::Serve(["mihomo-server", "--data-dir", "d"].map(OsString::from).into())
        );
        for line in [
            "mihomo-server",
            "mihomo-server status",
            "mihomo-server --help",
            "mihomo-server -V",
            "mihomo-server --json status",
            "mihomo-server --api=http://x status",
        ] {
            assert_eq!(route_of(line), Route::Cli, "{line}");
        }
    }

    #[test]
    fn command_line_parses_common_forms() {
        command().debug_assert();
        let parse = |line: &str| command().try_get_matches_from(line.split(' ')).unwrap();
        let matches = parse("mihomo-server proxy select AI 日本 --json");
        let (_, proxy) = matches.subcommand().unwrap();
        let (name, select) = proxy.subcommand().unwrap();
        assert_eq!(name, "select");
        assert_eq!(select.get_one::<String>("group").unwrap(), "AI");
        assert_eq!(select.get_one::<String>("node").unwrap(), "日本");
        assert!(matches.get_flag("json"));
        let logs = parse("mihomo-server logs -n 5 --no-pager");
        let forwarded: Vec<_> = logs
            .subcommand()
            .unwrap()
            .1
            .get_many::<String>("journalctl")
            .unwrap()
            .collect();
        assert_eq!(forwarded, ["-n", "5", "--no-pager"]);
        assert!(
            command()
                .try_get_matches_from(["mihomo-server", "proxy", "test", "--timeout", "5"])
                .is_err()
        );
        assert!(command().try_get_matches_from(["mihomo-server", "frobnicate"]).is_err());
    }

    #[test]
    fn tun_summary_reports_holder_capability_and_device() {
        let user = json!({"tun_device": "ms1000", "tun_capable": true, "tun_system": true});
        assert_eq!(
            tun_summary(
                &json!({"tun_enabled": true, "tun_holder": {"name": "me", "self": true}}),
                &user
            ),
            "on (device ms1000, system-wide)"
        );
        assert_eq!(
            tun_summary(
                &json!({"tun_enabled": false, "tun_holder": {"name": "alice", "self": false}}),
                &user
            ),
            "off (the system-wide TUN is held by alice)"
        );
        assert!(tun_summary(&json!({"tun_enabled": false}), &json!({"tun_capable": false})).contains("mihomo-tun"));
        assert_eq!(tun_summary(&json!({}), &Value::Null), "off");
        let access = json!({"ports": [{"key": "port", "actual": 0}, {"key": "mixed-port", "actual": 7890}],
                            "reported": {"mode": "Global"}});
        assert_eq!(mixed_port(&access), Some(7890));
        assert_eq!(current_mode(&access), "global");
    }
}
