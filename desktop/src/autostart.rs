//! "Start at login" as an XDG autostart entry that launches into the tray.
use anyhow::{Context as _, Result};
use std::path::PathBuf;

const FILE: &str = "mihomo-server-desktop.desktop";

fn path() -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))?;
    Some(config.join("autostart").join(FILE))
}

pub fn enabled() -> bool {
    path().is_some_and(|path| path.is_file())
}

/// Quote an argument for a desktop entry `Exec` key.
fn quote(argument: &str) -> String {
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
        "[Desktop Entry]\nType=Application\nName=Mihomo Server\nExec={} --hidden\nIcon=mihomo-server-desktop\nTerminal=false\nX-GNOME-Autostart-enabled=true\n",
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
    // An AppImage runs from a temporary mount; start the image itself.
    let executable = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)?;
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
