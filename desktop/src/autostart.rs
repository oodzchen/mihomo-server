//! The client's "start at login": an XDG autostart entry that launches this
//! client (`mihomo-server-desktop`, or its AppImage) into the tray. The
//! service starts on its own as a systemd user unit.
use anyhow::{Context as _, Result};
use std::path::PathBuf;

const FILE: &str = "mihomo-server-desktop.desktop";

fn path() -> Option<PathBuf> {
    Some(crate::settings::config_home()?.join("autostart").join(FILE))
}

pub fn enabled() -> bool {
    path().is_some_and(|path| path.is_file())
}

/// Quote an argument for a desktop entry `Exec` key.
pub fn quote(argument: &str) -> String {
    if !argument.contains(|c: char| c.is_whitespace() || "\"'\\`$;&|<>()*?#~".contains(c)) {
        return argument.to_owned();
    }
    let mut quoted = String::from('"');
    for c in argument.chars() {
        if matches!(c, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    quoted.push('"');
    quoted
}

fn entry(executable: &str) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Mihomo Server Desktop\nComment=mihomo-server desktop client (tray)\nExec={} --hidden\nIcon=mihomo-server-desktop\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
        quote(executable)
    )
}

pub fn set(enabled: bool) -> Result<()> {
    let path = path().context("cannot locate the autostart directory")?;
    if !enabled {
        return match std::fs::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        };
    }
    let executable = crate::relaunch::executable()?;
    std::fs::create_dir_all(path.parent().context("autostart directory")?)?;
    std::fs::write(&path, entry(&executable.to_string_lossy()))
        .with_context(|| format!("cannot write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exec_lines_quote_unusual_paths() {
        assert!(entry("/usr/bin/mihomo-server-desktop").contains("Exec=/usr/bin/mihomo-server-desktop --hidden\n"));
        assert!(entry("/home/a b/$x.AppImage").contains("Exec=\"/home/a b/\\$x.AppImage\" --hidden\n"));
    }
}
