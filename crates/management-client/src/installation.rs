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
