//! A desktop entry for runs no package installed (AppImage, development
//! builds). Wayland compositors take a window's icon from the desktop entry
//! named after its app_id (the program name), so without one the window shows
//! a generic icon. Packages install that entry system-wide; then ours is removed.
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

/// The window's app_id: GTK uses the program name, as do the packages for
/// the entry and icon names.
const APP_ID: &str = "mihomo-server-desktop";
/// Marks entries written here; any other entry of that name is left alone.
const MARKER: &str = "X-Mihomo-Server-Desktop-Generated=true";
const ICON: &[u8] = include_bytes!("../icons/128x128@2x.png");

fn data_home() -> Option<PathBuf> {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share")))
}

fn system_dirs() -> Vec<PathBuf> {
    let dirs = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|dirs| !dirs.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    std::env::split_paths(&dirs).filter(|path| path.is_absolute()).collect()
}

fn entry(exec: &str, icon: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Mihomo Server\nExec={}\nIcon={}\nTerminal=false\nNoDisplay=true\nStartupWMClass={APP_ID}\n{MARKER}\n",
        crate::autostart::quote(exec),
        icon.display()
    )
}

/// Write, refresh or remove the generated entry; `exec` is this program.
fn ensure_in(data_home: &Path, system: &[PathBuf], exec: &str) -> Result<()> {
    let file = data_home.join("applications").join(format!("{APP_ID}.desktop"));
    let icon = data_home.join(APP_ID).join("icon.png");
    let existing = std::fs::read_to_string(&file).ok();
    if existing.as_ref().is_some_and(|text| !text.contains(MARKER)) {
        return Ok(());
    }
    let packaged = system
        .iter()
        .any(|dir| dir.join("applications").join(format!("{APP_ID}.desktop")).is_file());
    if packaged {
        if existing.is_some() {
            std::fs::remove_file(&file)?;
            let _ = std::fs::remove_file(&icon);
        }
        return Ok(());
    }
    if std::fs::read(&icon).ok().as_deref() != Some(ICON) {
        std::fs::create_dir_all(icon.parent().context("icon directory")?)?;
        std::fs::write(&icon, ICON).with_context(|| format!("cannot write {}", icon.display()))?;
    }
    let wanted = entry(exec, &icon);
    if existing.as_deref() != Some(wanted.as_str()) {
        std::fs::create_dir_all(file.parent().context("applications directory")?)?;
        std::fs::write(&file, wanted).with_context(|| format!("cannot write {}", file.display()))?;
    }
    Ok(())
}

pub fn ensure() -> Result<()> {
    let data_home = data_home().context("cannot locate the data directory")?;
    // An AppImage runs from a temporary mount; point at the image itself.
    let exec = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .map_or_else(std::env::current_exe, Ok)?;
    ensure_in(&data_home, &system_dirs(), &exec.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unpackaged_runs_get_a_hidden_entry_until_a_package_provides_one() {
        let root = std::env::temp_dir().join(format!("msd-integration-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let (home, system) = (root.join("home"), root.join("usr/share"));
        let file = home.join("applications/mihomo-server-desktop.desktop");
        let icon = home.join("mihomo-server-desktop/icon.png");

        ensure_in(&home, std::slice::from_ref(&system), "/tmp/a b/app").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("Exec=\"/tmp/a b/app\"\n") && text.contains("NoDisplay=true\n"));
        assert!(text.contains(&format!("Icon={}\n", icon.display())));
        assert_eq!(std::fs::read(&icon).unwrap(), ICON);

        // A package's system-wide entry replaces ours.
        std::fs::create_dir_all(system.join("applications")).unwrap();
        std::fs::write(system.join("applications/mihomo-server-desktop.desktop"), "x").unwrap();
        ensure_in(&home, std::slice::from_ref(&system), "/tmp/app").unwrap();
        assert!(!file.exists() && !icon.exists());

        // An entry the user (or an AppImage integrator) wrote is never touched.
        std::fs::write(&file, "[Desktop Entry]\nName=Mine\n").unwrap();
        ensure_in(&home, &[], "/tmp/app").unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "[Desktop Entry]\nName=Mine\n");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
