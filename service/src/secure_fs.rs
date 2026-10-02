//! Shared filesystem primitives for private, link-safe service state.
//!
//! Raw `libc` descriptor calls are confined here. Callers keep their own
//! ownership/permission policies and error messages.
use anyhow::Result;
use std::{
    fs::{File, OpenOptions},
    io,
    path::Path,
};

/// Lowercase hexadecimal encoding.
pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Lowercase hexadecimal SHA-256 digest.
pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

/// Persist directory entry changes; a no-op on platforms without directory handles.
pub(crate) fn sync_directory(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Create a new owner-only file without following or replacing an existing entry.
pub(crate) fn create_private(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    options.open(path)
}

#[cfg(unix)]
pub(crate) use unix::*;

#[cfg(unix)]
mod unix {
    use std::{
        ffi::{CStr, OsString},
        fs::File,
        io,
        os::{
            fd::{AsRawFd as _, FromRawFd as _, IntoRawFd as _},
            unix::ffi::OsStringExt as _,
        },
    };

    fn check(result: libc::c_int) -> io::Result<()> {
        if result == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    /// Effective user ID of this process.
    pub(crate) fn euid() -> u32 {
        // SAFETY: geteuid has no preconditions and cannot fail.
        unsafe { libc::geteuid() }
    }

    /// Whether a file's capability xattr grants effective `CAP_NET_ADMIN` on exec.
    #[cfg(target_os = "linux")]
    pub(crate) fn file_grants_net_admin(path: &std::path::Path) -> bool {
        use std::os::unix::ffi::OsStrExt as _;
        const REVISION_MASK: u32 = 0xff00_0000;
        const FLAGS_EFFECTIVE: u32 = 0x1;
        const CAP_NET_ADMIN: u32 = 1 << 12;
        let Ok(path) = std::ffi::CString::new(path.as_os_str().as_bytes()) else {
            return false;
        };
        // vfs_cap_data revision 1 is 12 bytes, 2 is 20 and 3 is 24.
        let mut data = [0_u8; 24];
        // SAFETY: both C strings are valid and the length matches the buffer.
        let length = unsafe {
            libc::getxattr(
                path.as_ptr(),
                c"security.capability".as_ptr(),
                data.as_mut_ptr().cast(),
                data.len(),
            )
        };
        if length < 12 {
            return false;
        }
        let word = |at: usize| u32::from_le_bytes([data[at], data[at + 1], data[at + 2], data[at + 3]]);
        let (magic, permitted) = (word(0), word(4));
        matches!(magic & REVISION_MASK, 0x0100_0000 | 0x0200_0000 | 0x0300_0000)
            && magic & FLAGS_EFFECTIVE != 0
            && permitted & CAP_NET_ADMIN != 0
    }

    /// `openat(2)` relative to an open directory; `mode` applies only with `O_CREAT`.
    pub(crate) fn open_at(parent: &File, name: &CStr, flags: libc::c_int, mode: libc::mode_t) -> io::Result<File> {
        // SAFETY: both pointers are valid for the call; the variadic mode is a plain integer.
        let fd = unsafe { libc::openat(parent.as_raw_fd(), name.as_ptr(), flags, libc::c_uint::from(mode)) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openat returned a fresh descriptor that nothing else owns.
        Ok(unsafe { File::from_raw_fd(fd) })
    }

    /// `unlinkat(2)`; pass `libc::AT_REMOVEDIR` to remove an empty directory.
    pub(crate) fn unlink_at(parent: &File, name: &CStr, flags: libc::c_int) -> io::Result<()> {
        // SAFETY: the descriptor and C string are valid for the call.
        check(unsafe { libc::unlinkat(parent.as_raw_fd(), name.as_ptr(), flags) })
    }

    /// `mkdirat(2)` relative to an open directory.
    pub(crate) fn mkdir_at(parent: &File, name: &CStr, mode: libc::mode_t) -> io::Result<()> {
        // SAFETY: the descriptor and C string are valid for the call.
        check(unsafe { libc::mkdirat(parent.as_raw_fd(), name.as_ptr(), mode) })
    }

    /// Atomically rename within one directory, failing if the target exists.
    #[cfg(target_os = "linux")]
    pub(crate) fn rename_noreplace_at(directory: &File, from: &CStr, to: &CStr) -> io::Result<()> {
        let fd = directory.as_raw_fd();
        // SAFETY: the descriptor and C strings are valid for the call.
        check(unsafe { libc::renameat2(fd, from.as_ptr(), fd, to.as_ptr(), libc::RENAME_NOREPLACE) })
    }

    /// `fchmod(2)` on an open descriptor.
    pub(crate) fn fchmod(file: &File, mode: libc::mode_t) -> io::Result<()> {
        // SAFETY: the descriptor is valid for the call.
        check(unsafe { libc::fchmod(file.as_raw_fd(), mode) })
    }

    /// Non-blocking exclusive `flock(2)`; `Ok(false)` when another holder owns it.
    pub(crate) fn try_lock_exclusive(file: &File) -> io::Result<bool> {
        // SAFETY: the descriptor is valid for the call.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) {
            return Ok(false);
        }
        Err(error)
    }

    /// Entry names of an open directory, excluding `.` and `..`.
    ///
    /// Returns `Ok(None)` once more than `limit` entries are found.
    pub(crate) fn dir_names(directory: &File, limit: usize) -> io::Result<Option<Vec<OsString>>> {
        // fdopendir owns its descriptor. Reopen '.' so directory offsets are independent.
        let fd = open_at(
            directory,
            c".",
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_DIRECTORY,
            0,
        )?
        .into_raw_fd();
        // SAFETY: fd is an owned directory descriptor; on success the stream takes ownership.
        let raw = unsafe { libc::fdopendir(fd) };
        if raw.is_null() {
            let error = io::Error::last_os_error();
            // SAFETY: fdopendir failed, so fd is still owned here.
            unsafe {
                libc::close(fd);
            }
            return Err(error);
        }
        struct Stream(*mut libc::DIR);
        impl Drop for Stream {
            fn drop(&mut self) {
                // SAFETY: the stream is open and closed exactly once.
                unsafe {
                    libc::closedir(self.0);
                }
            }
        }
        let stream = Stream(raw);
        let mut result = Vec::new();
        loop {
            // readdir reports errors only through errno; clear it to distinguish end of stream.
            #[cfg(target_os = "linux")]
            // SAFETY: errno is thread-local and writable.
            unsafe {
                *libc::__errno_location() = 0;
            }
            // SAFETY: the stream is open for the whole loop.
            let entry = unsafe { libc::readdir(stream.0) };
            if entry.is_null() {
                #[cfg(target_os = "linux")]
                if io::Error::last_os_error().raw_os_error() != Some(0) {
                    return Err(io::Error::last_os_error());
                }
                break;
            }
            // SAFETY: d_name is a NUL-terminated name valid until the next readdir call.
            let bytes = unsafe { CStr::from_ptr((*entry).d_name.as_ptr()) }.to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if result.len() >= limit {
                return Ok(None);
            }
            result.push(OsString::from_vec(bytes.to_vec()));
        }
        Ok(Some(result))
    }
}
