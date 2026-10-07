//! Restart through the installed entry point, retaining the process identity
//! on Unix so a systemd application unit does not kill the replacement.
use std::{ffi::OsString, io, path::PathBuf, process::Command};

// Ask Tauri to finish its event loop and release the single-instance name;
// its built-in restart spawns a child and then exits the service's main PID.
pub const EXIT_CODE: i32 = 75;

fn select_launcher(
    image: Option<OsString>,
    packaged: Option<OsString>,
    current: impl FnOnce() -> io::Result<PathBuf>,
) -> io::Result<PathBuf> {
    image
        .filter(|path| !path.is_empty())
        .or_else(|| packaged.filter(|path| !path.is_empty()))
        .map(PathBuf::from)
        .map_or_else(current, Ok)
}

/// AppImages use the image, Nix packages use their wrapper via PATH, and
/// ordinary installations use the executable on disk.
pub fn executable() -> io::Result<PathBuf> {
    select_launcher(
        std::env::var_os("APPIMAGE"),
        std::env::var_os("MIHOMO_DESKTOP_LAUNCHER"),
        std::env::current_exe,
    )
}

fn replace(mut command: Command) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        Err(command.exec())
    }
    #[cfg(not(unix))]
    command.spawn().map(drop)
}

pub fn restart() -> io::Result<()> {
    let mut command = Command::new(executable()?);
    command.args(std::env::args_os().skip(1));
    replace(command)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installed_launcher_takes_precedence_over_the_wrapped_binary() {
        let current = || Ok(PathBuf::from("/nix/store/old/bin/.mihomo-server-desktop-wrapped"));
        assert_eq!(
            select_launcher(None, Some("mihomo-server-desktop".into()), current).unwrap(),
            PathBuf::from("mihomo-server-desktop")
        );
        assert_eq!(
            select_launcher(
                Some("/home/a b/client.AppImage".into()),
                Some("mihomo-server-desktop".into()),
                current,
            )
            .unwrap(),
            PathBuf::from("/home/a b/client.AppImage")
        );
        assert_eq!(
            select_launcher(None, None, || Ok(PathBuf::from("/usr/bin/mihomo-server-desktop"))).unwrap(),
            PathBuf::from("/usr/bin/mihomo-server-desktop")
        );
    }

    #[cfg(unix)]
    #[test]
    fn exec_probe() {
        if std::env::var_os("MIHOMO_REEXEC_PROBE").is_some() {
            let mut command = Command::new("sh");
            command.args([
                "-c",
                "printf 'exec-result:%s:%s:%s' \"$$\" \"$1\" \"$2\"",
                "probe",
                "--hidden",
                "a b",
            ]);
            panic!("exec failed: {}", replace(command).unwrap_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn exec_preserves_pid_and_arguments() {
        let child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "relaunch::tests::exec_probe", "--nocapture"])
            .env("MIHOMO_REEXEC_PROBE", "1")
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let pid = child.id();
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success());
        assert!(
            String::from_utf8(output.stdout)
                .unwrap()
                .contains(&format!("exec-result:{pid}:--hidden:a b"))
        );
    }
}
