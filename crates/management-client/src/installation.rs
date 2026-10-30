//! Installation ownership follows the executable, never the host distribution.
use std::path::{Path, PathBuf};

fn root(executable: &Path) -> Option<PathBuf> {
    let executable = executable.canonicalize().ok()?;
    let bin = executable.parent()?;
    (bin.file_name()? == "bin").then(|| bin.parent().map(Path::to_path_buf))?
}

pub fn nix_root() -> Option<PathBuf> {
    nix_root_for(&std::env::current_exe().ok()?)
}

fn nix_root_for(executable: &Path) -> Option<PathBuf> {
    let root = root(executable)?;
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(root.join("nix-installation.json")).ok()?).ok()?;
    (manifest.get("kind")?.as_str()? == "nix").then_some(root)
}

pub fn nix_managed() -> bool {
    nix_root().is_some()
}

pub const NIX_UPDATE_HINT: &str = "Managed by Nix: update the mihomo-server input in your system flake, then run nixos-rebuild switch. Remove services.mihomo-server from the NixOS configuration to uninstall; user data is retained.";

/// Members may run the host's system-wide TUN (see the installer).
pub const TUN_GROUP: &str = "mihomo-tun";

/// The account running this program.
pub fn current_user() -> anyhow::Result<String> {
    let output = std::process::Command::new("id").arg("-un").output()?;
    anyhow::ensure!(output.status.success(), "cannot determine the current user");
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}

/// Whether USER belongs to the TUN group in the account database, which a
/// running service's credentials may predate (`None`: no such group here).
pub fn tun_member(user: &str) -> Option<bool> {
    let group = std::process::Command::new("getent")
        .args(["group", TUN_GROUP])
        .output()
        .ok()?;
    if !group.status.success() {
        return None;
    }
    let groups = std::process::Command::new("id")
        .args(["-nG", "--", user])
        .output()
        .ok()?;
    Some(
        groups.status.success()
            && String::from_utf8_lossy(&groups.stdout)
                .split_whitespace()
                .any(|name| name == TUN_GROUP),
    )
}

/// The one-time authorization that adds USER to the TUN group, through polkit:
/// pkexec shows the desktop's authentication dialog, or asks in a terminal.
/// The user's service gains the group when it restarts.
pub fn authorize_tun(user: &str) -> anyhow::Result<std::process::Command> {
    anyhow::ensure!(
        !nix_managed(),
        "TUN users are declared by NixOS: add this user to services.mihomo-server.users with services.mihomo-server.tun.enable, then run nixos-rebuild switch"
    );
    anyhow::ensure!(
        tun_member(user).is_some(),
        "this installation has no TUN group ({TUN_GROUP}); run the installer again"
    );
    let usermod = ["/usr/sbin/usermod", "/sbin/usermod", "/usr/bin/usermod"]
        .into_iter()
        .map(Path::new)
        .find(|path| path.is_file())
        .ok_or_else(|| anyhow::anyhow!("usermod is missing"))?;
    let mut command = std::process::Command::new("pkexec");
    command.arg(usermod).args(["-aG", TUN_GROUP, "--", user]);
    Ok(command)
}

/// Why `authorize_tun`'s command failed, from its exit status.
pub fn authorization_failure(code: Option<i32>) -> String {
    match code {
        // pkexec(1): 126 the dialog was dismissed, 127 not authorized.
        Some(126) => "authorization was cancelled".into(),
        Some(127) => "authorization was refused or unavailable (no polkit authentication agent?)".into(),
        Some(code) => format!("adding the user to {TUN_GROUP} failed (exit status {code})"),
        None => format!("adding the user to {TUN_GROUP} was interrupted"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ownership_requires_metadata_beside_the_executable() {
        let root = std::env::temp_dir().join(format!("ms-installation-{}", std::process::id()));
        std::fs::create_dir_all(root.join("bin")).unwrap();
        let binary = root.join("bin/mihomo-server");
        std::fs::write(&binary, b"fixture").unwrap();
        assert_eq!(nix_root_for(&binary), None);
        std::fs::write(root.join("nix-installation.json"), br#"{"kind":"nix"}"#).unwrap();
        assert_eq!(nix_root_for(&binary), Some(root.clone()));
        std::fs::write(root.join("nix-installation.json"), br#"{"kind":"installer"}"#).unwrap();
        assert_eq!(nix_root_for(&binary), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
