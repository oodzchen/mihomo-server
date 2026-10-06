//! TUN capability launcher for a shared, multi-user installation.
//!
//! The installer keeps a copy of this binary named `mihomo-tun-exec` that only
//! the TUN group may run, with `cap_net_admin,cap_net_bind_service,cap_net_raw+ep`
//! file capabilities. Started under that name, it raises those capabilities as
//! ambient and executes the user's own managed core, so web core upgrades keep
//! TUN without the administrator. Group membership is the security boundary,
//! as with a capable core: a member can run any binary with these capabilities.
use std::{
    ffi::OsString,
    os::unix::process::CommandExt as _,
    path::{Path, PathBuf},
};

use anyhow::{Context as _, Result, bail, ensure};

/// The installed name of the capable copy; the only name that enters launcher mode.
pub const NAME: &str = "mihomo-tun-exec";

const CAPABILITIES: [u32; 3] = [
    12, // CAP_NET_ADMIN
    10, // CAP_NET_BIND_SERVICE
    13, // CAP_NET_RAW
];
const VERSION_3: u32 = 0x2008_0522;

#[repr(C)]
struct Header {
    version: u32,
    pid: libc::c_int,
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct Data {
    effective: u32,
    permitted: u32,
    inheritable: u32,
}

/// Whether this process was started as the capable launcher.
pub fn invoked() -> bool {
    std::env::args_os()
        .next()
        .and_then(|zero| Path::new(&zero).file_name().map(|name| name == NAME))
        .unwrap_or(false)
}

/// `mihomo-tun-exec PARENT_PID CORE [ARGS...]`: never returns on success.
pub fn run() -> Result<()> {
    let mut arguments = std::env::args_os().skip(1);
    let parent: libc::pid_t = arguments
        .next()
        .and_then(|value| value.into_string().ok())
        .and_then(|value| value.parse().ok())
        .context("usage: mihomo-tun-exec PARENT_PID CORE [ARGS...]")?;
    let core = PathBuf::from(arguments.next().context("missing core path")?);
    ensure!(core.is_absolute(), "core path must be absolute");
    let rest: Vec<OsString> = arguments.collect();
    raise_ambient()?;
    // Executing a file with capabilities cleared the parent-death signal.
    // SAFETY: plain prctl/getppid calls with constant arguments.
    unsafe {
        if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
            return Err(std::io::Error::last_os_error()).context("set parent-death signal");
        }
        if libc::getppid() != parent {
            bail!("supervisor exited before the core started");
        }
    }
    Err(std::process::Command::new(&core).args(rest).exec()).with_context(|| format!("execute {}", core.display()))
}

fn raise_ambient() -> Result<()> {
    let mask = CAPABILITIES.iter().fold(0_u32, |mask, cap| mask | 1 << cap);
    let mut header = Header {
        version: VERSION_3,
        pid: 0,
    };
    let mut data = [Data::default(); 2];
    // SAFETY: header and a two-element data array match the version 3 ABI.
    if unsafe { libc::syscall(libc::SYS_capget, &mut header, data.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("read capabilities");
    }
    ensure!(
        data[0].permitted & mask == mask,
        "TUN capabilities are missing; the administrator must reinstall to restore them"
    );
    data[0].inheritable |= mask;
    // SAFETY: as above; only inheritable bits already in the permitted set are added.
    if unsafe { libc::syscall(libc::SYS_capset, &mut header, data.as_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error()).context("set inheritable capabilities");
    }
    for cap in CAPABILITIES {
        // SAFETY: PR_CAP_AMBIENT_RAISE takes a capability number and zero padding.
        if unsafe {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_RAISE,
                cap as libc::c_ulong,
                0,
                0,
            )
        } != 0
        {
            return Err(std::io::Error::last_os_error()).context("raise ambient capability");
        }
    }
    Ok(())
}

/// The installed launcher beside the running service, when this user may run it.
pub(crate) fn available() -> Option<PathBuf> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let candidates: [Option<PathBuf>; 3] = [
        std::env::var_os("MIHOMO_TUN_EXEC").map(PathBuf::from),
        Some(PathBuf::from("/run/wrappers/bin").join(NAME)),
        std::env::current_exe().ok().map(|exe| exe.with_file_name(NAME)),
    ];

    for candidate in candidates.into_iter().flatten() {
        let Ok(metadata) = std::fs::symlink_metadata(&candidate) else {
            continue;
        };
        // Root-owned and not writable by others, like the rest of the shared bundle.
        if !metadata.is_file() || metadata.uid() != 0 || metadata.permissions().mode() & 0o022 != 0 {
            continue;
        }
        let Ok(name) = std::ffi::CString::new(candidate.as_os_str().as_encoded_bytes()) else {
            continue;
        };
        // SAFETY: valid C string; access checks this process's credentials.
        let executable = unsafe { libc::access(name.as_ptr(), libc::X_OK) } == 0;
        if executable && crate::secure_fs::file_grants_net_admin(&candidate) {
            return Some(candidate);
        }
    }
    None
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_requires_installed_file_capabilities() {
        // Test binaries are never the installed, root-owned capable copy.
        assert!(available().is_none());
        assert!(!invoked());
        if crate::secure_fs::euid() != 0 {
            let error = raise_ambient().unwrap_err();
            assert!(format!("{error:#}").contains("capabilities are missing"), "{error:#}");
        }
    }
}
