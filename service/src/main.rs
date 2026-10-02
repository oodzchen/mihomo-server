use std::{net::SocketAddr, path::PathBuf, time::Duration};

use anyhow::{Context as _, Result};
use clap::{Arg, ArgAction, ArgMatches, Command, parser::ValueSource, value_parser};
use headless_core::enhance::isolation::Isolation;
use mihomo_server::{
    core_manager::{CoreManager, CoreOptions},
    management::{
        Management,
        auth::Authentication,
        http::{HttpState, router},
    },
    resources::Resources,
    shutdown::ShutdownSignals,
};

fn main() -> Result<()> {
    if std::env::args().len() == 2 && std::env::args().nth(1).as_deref() == Some("--script-worker") {
        return mihomo_server::script::worker_stdio();
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

#[cfg(target_os = "linux")]
fn isolation(arguments: &ArgMatches) -> Result<Isolation> {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let uid = unsafe { libc::geteuid() };
    let slot = match arguments.get_one::<u16>("slot") {
        Some(slot) => *slot,
        None => {
            let registry = arguments
                .get_one::<PathBuf>("slot-registry")
                .cloned()
                .unwrap_or_else(|| mihomo_server::multi_user::DEFAULT_SLOT_REGISTRY.into());
            mihomo_server::multi_user::claim_slot(&registry, uid)?
        }
    };
    Isolation::new(uid, slot)
}

#[cfg(not(target_os = "linux"))]
fn isolation(_: &ArgMatches) -> Result<Isolation> {
    anyhow::bail!("multi-user mode requires Linux")
}

async fn run() -> Result<()> {
    let arguments = Command::new("mihomo-server")
        .version(env!("CARGO_PKG_VERSION"))
        .about("Headless Mihomo supervisor with an authenticated management API")
        .arg(
            Arg::new("resource-dir")
                .long("resource-dir")
                .value_parser(value_parser!(PathBuf))
                .conflicts_with("mihomo")
                .help("Pinned bundle resources; initializes and preserves a managed core"),
        )
        .arg(
            Arg::new("core-dir")
                .long("core-dir")
                .value_parser(value_parser!(PathBuf))
                .requires("resource-dir")
                .help("Persistent managed core directory; defaults to <data-dir>/core"),
        )
        .arg(
            Arg::new("multi-user")
                .long("multi-user")
                .action(ArgAction::SetTrue)
                .requires("resource-dir")
                .conflicts_with("core-dir")
                .help("Shared system installation: run the bundle core in place and isolate ports/TUN per user"),
        )
        .arg(
            Arg::new("slot")
                .long("slot")
                .value_parser(value_parser!(u16))
                .requires("multi-user")
                .help("Fixed multi-user slot instead of a registry claim"),
        )
        .arg(
            Arg::new("slot-registry")
                .long("slot-registry")
                .value_parser(value_parser!(PathBuf))
                .requires("multi-user")
                .conflicts_with("slot")
                .help("Root-owned sticky directory of slot claims [default: /var/lib/mihomo-server/slots]"),
        )
        .arg(
            Arg::new("listen")
                .long("listen")
                .value_parser(value_parser!(SocketAddr))
                .default_value("127.0.0.1:9090")
                .help("Management HTTP address; requires an explicit nonzero port"),
        )
        .arg(
            Arg::new("web-dir")
                .long("web-dir")
                .value_parser(value_parser!(PathBuf))
                .help("Built Web asset directory; omit for API-only operation"),
        )
        .arg(
            Arg::new("public-origin")
                .long("public-origin")
                .help("Exact browser origin/Host; required with a wildcard listener or reverse proxy"),
        )
        .arg(
            Arg::new("mihomo")
                .long("mihomo")
                .value_parser(value_parser!(PathBuf))
                .help("Mihomo binary path or executable name"),
        )
        .arg(
            Arg::new("data-dir")
                .long("data-dir")
                .value_parser(value_parser!(PathBuf))
                .default_value("data")
                .help("Explicit service data directory"),
        )
        .arg(
            Arg::new("config")
                .long("config")
                .value_parser(value_parser!(PathBuf))
                .help("Bootstrap YAML; committed config takes precedence; defaults to <data-dir>/runtime.yaml"),
        )
        .arg(
            Arg::new("import-config")
                .long("import-config")
                .conflicts_with_all(["import-profile", "select-profile"])
                .value_parser(value_parser!(PathBuf))
                .help("Validate and persist a replacement runtime YAML before startup"),
        )
        .arg(
            Arg::new("import-profile")
                .long("import-profile")
                .value_parser(value_parser!(PathBuf))
                .conflicts_with("select-profile")
                .help("Save a local subscription YAML, select it, then start the core"),
        )
        .arg(
            Arg::new("profile-name")
                .long("profile-name")
                .requires("import-profile")
                .help("Display name for the imported local subscription"),
        )
        .arg(
            Arg::new("select-profile")
                .long("select-profile")
                .help("Validate and apply an existing subscription UID before startup"),
        )
        .arg(
            Arg::new("select-node")
                .long("select-node")
                .num_args(2)
                .value_names(["GROUP", "NODE"])
                .action(ArgAction::Append)
                .conflicts_with("no-start")
                .help("Select and persist a node after startup; may be repeated"),
        )
        .arg(
            Arg::new("unfix-node")
                .long("unfix-node")
                .action(ArgAction::Append)
                .conflicts_with("no-start")
                .help("Unfix an automatic group and forget its saved selection"),
        )
        .arg(
            Arg::new("no-start")
                .long("no-start")
                .action(ArgAction::SetTrue)
                .help("Keep the service running with the core stopped"),
        )
        .get_matches();
    let binary = arguments
        .get_one::<PathBuf>("mihomo")
        .cloned()
        .unwrap_or_else(|| "verge-mihomo".into());
    let resources = arguments
        .get_one::<PathBuf>("resource-dir")
        .map(|path| Resources::open(path))
        .transpose()?;
    let data_dir = arguments
        .get_one::<PathBuf>("data-dir")
        .context("missing data directory")?
        .clone();
    let config = match arguments.get_one::<PathBuf>("config") {
        Some(path) => path.clone(),
        None => match &resources {
            Some(resources) => resources.bootstrap()?,
            None => data_dir.join("runtime.yaml"),
        },
    };
    let web_dir = arguments
        .get_one::<PathBuf>("web-dir")
        .cloned()
        .or_else(|| resources.as_ref().map(Resources::web_dir));
    let isolation = if arguments.get_flag("multi-user") {
        Some(isolation(&arguments)?)
    } else {
        None
    };
    let mut signals = ShutdownSignals::register()?;
    let mut options = CoreOptions::new(binary, data_dir.clone(), config);
    options.resources = resources;
    options.core_dir = arguments.get_one::<PathBuf>("core-dir").cloned();
    options.isolation = isolation;
    let manager = CoreManager::spawn(options)?;
    let mut listen = *arguments
        .get_one::<SocketAddr>("listen")
        .context("missing listener argument")?;
    if let Some(isolation) = isolation {
        if arguments.value_source("listen") == Some(ValueSource::DefaultValue) {
            listen.set_port(isolation.management_port());
        }
        let user = manager.multi_user().context("multi-user state missing")?;
        eprintln!(
            "multi-user slot {} (uid {}): mixed port {}, TUN device {} ({})",
            user.slot,
            user.uid,
            user.mixed_port,
            user.tun_device,
            if user.tun_capable {
                "available"
            } else {
                "unavailable: not in the TUN group"
            }
        );
    }
    // Bind the management surface before beginning any core startup operation.
    let setup = async {
        let authentication = Authentication::load_or_create(
            &data_dir.join("management-token"),
            listen,
            arguments.get_one::<String>("public-origin").map(String::as_str),
        )?;
        let socket = if listen.is_ipv6() {
            tokio::net::TcpSocket::new_v6()?
        } else {
            tokio::net::TcpSocket::new_v4()?
        };
        socket.set_reuseaddr(true)?;
        #[cfg(unix)]
        socket.set_reuseport(true)?;
        socket.bind(listen).context("bind management listener")?;
        let listener = socket.listen(1024).context("listen management listener")?;
        let mut state = HttpState::new(Management::new(manager.clone(), authentication));
        if let Some(directory) = &web_dir {
            state = state.with_web_assets(directory)?;
        }
        Ok::<_, anyhow::Error>((listener, state))
    }
    .await;
    let (listener, http_state) = match setup {
        Ok(setup) => setup,
        Err(error) => {
            manager.shutdown().await?;
            return Err(error);
        }
    };
    let (http_stop, mut http_stop_rx) = tokio::sync::watch::channel(false);
    let mut http_server = tokio::spawn(
        axum::serve(listener, router(http_state.clone()))
            .with_graceful_shutdown(async move {
                while !*http_stop_rx.borrow_and_update() {
                    if http_stop_rx.changed().await.is_err() {
                        break;
                    }
                }
            })
            .into_future(),
    );
    eprintln!(
        "management API listening on {listen}; token file: {}",
        data_dir.join("management-token").display()
    );
    let mut state = manager.subscribe_status();
    println!("{}", serde_json::to_string(&manager.status())?);
    let import = arguments.get_one::<PathBuf>("import-config").cloned();
    let profile_import = arguments.get_one::<PathBuf>("import-profile").cloned();
    let profile_name = arguments.get_one::<String>("profile-name").cloned();
    let profile_selection = arguments.get_one::<String>("select-profile").cloned();
    let no_start = arguments.get_flag("no-start");
    let node_selections = arguments
        .get_many::<String>("select-node")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let unfix = arguments
        .get_many::<String>("unfix-node")
        .map(|values| values.cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let startup = if no_start && import.is_none() && profile_import.is_none() && profile_selection.is_none() {
        None
    } else {
        let manager = manager.clone();
        Some(tokio::spawn(async move {
            let result = async {
                if let Some(path) = import {
                    manager.import_config(path).await?;
                }
                let selection = if let Some(path) = profile_import {
                    let profile = manager.import_profile(path, profile_name).await?;
                    let uid = profile.uid.context("imported profile has no UID")?.to_string();
                    eprintln!("imported local profile: {uid}");
                    Some(uid)
                } else {
                    profile_selection
                };
                if let Some(uid) = selection {
                    manager.select_profile(uid).await?;
                }
                if !no_start {
                    manager.start().await?;
                }
                for pair in node_selections.as_chunks::<2>().0 {
                    manager.select_node(pair[0].clone(), pair[1].clone()).await?;
                }
                for group in unfix {
                    manager.unfix_node(group).await?;
                }
                Ok::<(), anyhow::Error>(())
            }
            .await;
            if let Err(error) = result {
                eprintln!("startup operation failed: {error:#}");
            }
        }))
    };
    let mut server_finished = false;
    let mut server_result = Ok(());
    loop {
        tokio::select! {
            biased;
            _ = signals.wait() => break,
            result = &mut http_server => {
                server_finished = true;
                server_result = result.context("management server task failed")
                    .and_then(|result| result.context("management server failed"));
                break;
            }
            changed = state.changed() => {
                if changed.is_err() {
                    server_result = Err(anyhow::anyhow!("core state stream closed"));
                    break;
                }
                println!("{}", serde_json::to_string(&*state.borrow_and_update())?);
            }
        }
    }
    http_state.close();
    http_stop.send_replace(true);
    let result = manager.shutdown().await;
    if let Some(startup) = startup {
        startup.await.context("startup task failed")?;
    }
    {
        match tokio::time::timeout(Duration::from_secs(10), async {
            if server_finished {
                http_state.drain_websockets().await;
                Ok(Ok(()))
            } else {
                let (joined, ()) = tokio::join!(&mut http_server, http_state.drain_websockets());
                joined
            }
        })
        .await
        {
            Ok(joined) => {
                if !server_finished {
                    server_result = joined
                        .context("management server task failed")
                        .and_then(|result| result.context("management server failed"));
                }
            }
            Err(_) => {
                if !server_finished {
                    http_server.abort();
                    let _ = http_server.await;
                }
                server_result = Err(anyhow::anyhow!("management HTTP/WebSocket drain timed out"));
            }
        }
    }
    println!("{}", serde_json::to_string(&manager.status())?);
    result.and(server_result)
}
