//! The service's own lifecycle and program upgrade, requested from the Web.
//!
//! Stop and restart go through the user's systemd manager when the service
//! runs as a unit, so systemd records the intent (a stopped unit is not
//! restarted). The upgrade starts the installer's root-owned
//! `mihomo-server-update.service`, which takes no input: it installs the latest
//! release of the built-in repository, and polkit lets TUN-group members start
//! it without a password. The installer then restarts every running instance.
use anyhow::{Context as _, Result, bail, ensure};
use serde::Serialize;
use std::{io::Read as _, io::Seek as _, path::Path, process::Command, time::Duration};

pub const UPDATE_UNIT: &str = "mihomo-server-update.service";
/// Written by the update unit (systemd `StandardOutput=truncate:`), world-readable.
const UPDATE_LOG: &str = "/var/lib/mihomo-server/update.log";
const LOG_LINES: usize = 40;
const LOG_BYTES: u64 = 16 * 1024;

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct ServiceInfo {
    pub version: &'static str,
    /// Release directory this program runs from; `None` outside the shared installation.
    pub release: Option<String>,
    /// The systemd unit running this service; `None` when started directly.
    pub unit: Option<String>,
    pub upgrade: UpgradeState,
}

#[derive(Debug, Default, Serialize, PartialEq, Eq)]
pub struct UpgradeState {
    /// The shared installation provides the update unit.
    pub available: bool,
    /// systemd `ActiveState` of the update unit (`activating` while it runs).
    pub state: String,
    /// systemd `Result` of the last run (`success`, `exit-code`, …).
    pub result: String,
    /// Tail of the last run's output.
    pub log: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Stop,
    Restart,
}

impl Action {
    fn verb(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Restart => "restart",
        }
    }
}

pub fn info() -> ServiceInfo {
    let release = crate::cli::system::installed_release();
    let mut upgrade = UpgradeState::default();
    if let Some(properties) = show(UPDATE_UNIT) {
        upgrade.available = release.is_some() && property(&properties, "LoadState") == Some("loaded");
        upgrade.state = property(&properties, "ActiveState").unwrap_or_default().to_owned();
        upgrade.result = property(&properties, "Result").unwrap_or_default().to_owned();
    }
    if upgrade.available {
        upgrade.log = tail(Path::new(UPDATE_LOG));
    }
    ServiceInfo {
        version: env!("CARGO_PKG_VERSION"),
        release,
        unit: unit().map(|unit| unit.name),
        upgrade,
    }
}

/// Stop or restart this service shortly after the request has been answered.
pub fn schedule(action: Action) -> Result<()> {
    let unit = unit();
    if unit.is_none() {
        ensure!(
            action == Action::Stop,
            "this service was started directly, not by systemd; restart it where it was started"
        );
    }
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        match &unit {
            Some(unit) => {
                let mut arguments = unit.scope().to_vec();
                arguments.extend(["--no-block", "--no-ask-password", action.verb(), unit.name.as_str()]);
                if let Err(error) = systemctl(&arguments) {
                    eprintln!("[service] {} failed: {error:#}", action.verb());
                }
            }
            // The same graceful path as an external SIGTERM.
            #[cfg(unix)]
            None => unsafe {
                libc::kill(libc::getpid(), libc::SIGTERM);
            },
            #[cfg(not(unix))]
            None => eprintln!("[service] stop is not supported on this platform"),
        }
    });
    Ok(())
}

/// Start the update unit; progress is read back through [`info`].
pub fn upgrade() -> Result<()> {
    ensure!(
        crate::cli::system::installed_release().is_some(),
        "upgrades need the shared installation; this program does not run from it"
    );
    let properties = show(UPDATE_UNIT).context("cannot read the update unit through systemctl")?;
    ensure!(
        property(&properties, "LoadState") == Some("loaded"),
        "the shared installation has no {UPDATE_UNIT}; upgrade once with `mihomo-server update`"
    );
    systemctl(&["--no-block", "--no-ask-password", "start", UPDATE_UNIT])
}

/// The unit is read from this process's cgroup, which systemd names after it.
/// An `INVOCATION_ID` inherited from a terminal does not count: the unit's main
/// process must be this one or an ancestor (the launcher may run it via `sg`).
fn unit() -> Option<Unit> {
    std::env::var_os("INVOCATION_ID")?;
    let unit = unit_from_cgroup(&std::fs::read_to_string("/proc/self/cgroup").ok()?)?;
    let output = Command::new("systemctl")
        .args(unit.scope())
        .args(["show", unit.name.as_str(), "-p", "MainPID", "--value"])
        .output()
        .ok()?;
    let main: u32 = String::from_utf8_lossy(&output.stdout).trim().parse().ok()?;
    let mut pid = std::process::id();
    for _ in 0..4 {
        if pid == main {
            return Some(unit);
        }
        pid = parent(pid)?;
    }
    None
}

#[derive(Debug, PartialEq, Eq)]
struct Unit {
    name: String,
    /// Run by a user manager (`systemctl --user`) rather than the system one.
    user: bool,
}

impl Unit {
    fn scope(&self) -> &'static [&'static str] {
        if self.user { &["--user"] } else { &[] }
    }
}

fn unit_from_cgroup(cgroup: &str) -> Option<Unit> {
    let path = cgroup.lines().find_map(|line| line.strip_prefix("0::"))?;
    let name = path.rsplit('/').next()?;
    (name.ends_with(".service") && !name.starts_with("user@") && name.len() > ".service".len()).then(|| Unit {
        name: name.to_owned(),
        user: path.contains("/user@"),
    })
}

fn parent(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name may contain spaces and parentheses; fields follow the last ')'.
    stat.rsplit_once(')')?.1.split_whitespace().nth(1)?.parse().ok()
}

fn systemctl(arguments: &[&str]) -> Result<()> {
    let output = Command::new("systemctl")
        .args(arguments)
        .output()
        .context("cannot run systemctl")?;
    if output.status.success() {
        return Ok(());
    }
    let message = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    bail!(
        "systemctl {} failed: {}",
        arguments.join(" "),
        if message.is_empty() {
            output.status.to_string()
        } else {
            message
        }
    )
}

fn show(unit: &str) -> Option<String> {
    let output = Command::new("systemctl")
        .args(["show", unit, "-p", "LoadState,ActiveState,Result"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn property<'a>(properties: &'a str, name: &str) -> Option<&'a str> {
    properties
        .lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix('='))
}

/// Last lines of a bounded file tail; an unreadable log is empty.
fn tail(path: &Path) -> Vec<String> {
    let Ok(mut file) = std::fs::File::open(path) else {
        return Vec::new();
    };
    let length = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    let start = length.saturating_sub(LOG_BYTES);
    if file.seek(std::io::SeekFrom::Start(start)).is_err() {
        return Vec::new();
    }
    let mut bytes = Vec::new();
    if file.take(LOG_BYTES).read_to_end(&mut bytes).is_err() {
        return Vec::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<&str> = text.lines().collect();
    // A cut first line is partial.
    if start > 0 && !lines.is_empty() {
        lines.remove(0);
    }
    // Drop terminal control sequences (curl progress bars, colors).
    let lines = lines
        .iter()
        .map(|line| line.rsplit('\r').next().unwrap_or(line))
        .map(|line| {
            line.chars()
                .filter(|character| !character.is_control())
                .collect::<String>()
        })
        .filter(|line| !line.trim().is_empty())
        .collect::<Vec<_>>();
    lines[lines.len().saturating_sub(LOG_LINES)..].to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_is_named_by_the_cgroup() {
        assert_eq!(
            unit_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service/app.slice/mihomo-server.service\n"),
            Some(Unit {
                name: "mihomo-server.service".into(),
                user: true
            })
        );
        assert_eq!(
            unit_from_cgroup("0::/system.slice/mihomo-server.service\n"),
            Some(Unit {
                name: "mihomo-server.service".into(),
                user: false
            })
        );
        assert_eq!(
            unit_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service/init.scope\n"),
            None
        );
        assert_eq!(
            unit_from_cgroup("0::/user.slice/user-1000.slice/user@1000.service\n"),
            None
        );
        assert_eq!(unit_from_cgroup("1:name=systemd:/x.service\n"), None);
    }

    #[test]
    fn parent_process_is_read_from_proc() {
        assert_eq!(parent(std::process::id()), Some(std::os::unix::process::parent_id()));
    }

    #[test]
    fn systemctl_properties_are_read_by_name() {
        let properties = "Result=exit-code\nLoadState=loaded\nActiveState=failed\n";
        assert_eq!(property(properties, "LoadState"), Some("loaded"));
        assert_eq!(property(properties, "ActiveState"), Some("failed"));
        assert_eq!(property(properties, "Result"), Some("exit-code"));
        assert_eq!(property(properties, "Load"), None);
    }

    #[test]
    fn update_log_tail_is_bounded_and_printable() {
        let directory = std::env::temp_dir().join(format!("mihomo-server-update-log-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("update.log");
        let mut text = String::from("==> downloading\r###   10%\r#### 100%\n\x1b[1mbold\n");
        for index in 0..100 {
            text.push_str(&format!("line {index}\n"));
        }
        std::fs::write(&path, text).unwrap();
        let lines = tail(&path);
        assert_eq!(lines.len(), LOG_LINES);
        assert_eq!(lines.last().map(String::as_str), Some("line 99"));
        std::fs::write(&path, "==> downloading\r###   10%\r#### 100%\n\x1b[1mbold\n").unwrap();
        assert_eq!(tail(&path), ["#### 100%", "[1mbold"]);
        std::fs::write(&path, "x".repeat(LOG_BYTES as usize * 2) + "\nlast\n").unwrap();
        assert_eq!(tail(&path), ["last"]);
        assert!(tail(&directory.join("missing")).is_empty());
        std::fs::remove_dir_all(&directory).unwrap();
    }
}
