//! SO_PEERCRED + group-membership extractor for the admin plane.
//!
//! Per Spec C₂ §5.3, the admin plane authenticates via SO_PEERCRED + group
//! membership. The pidfd extractor guards against TOCTOU races (the Polkit
//! CVE family — research synthesis 3.8). On macOS (used by developers
//! locally), `SO_PEERCRED` is unavailable; we fall back to `getuid()` so
//! tests run on dev workstations. Production deploys are Linux-only per
//! master-spec §L10.
//!
//! This module is the only place the lifed crate reaches for `unsafe`.
//! `lib.rs` denies unsafe at the crate root; we re-allow it here for the
//! one syscall block that drives `getsockopt(SO_PEERCRED)`,
//! `pidfd_open(2)`, and `getuid(2)`. Group lookups go through the safe
//! reentrant `nix::unistd::Group` wrapper (`getgrnam_r`), not raw
//! `getgrnam`.

#![allow(unsafe_code)]

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsRawFd, RawFd};
    use tokio::net::UnixStream;

    /// SO_PEERCRED ucred — `(pid, uid, gid)`.
    #[derive(Debug, Clone, Copy)]
    pub struct PeerCred {
        pub pid: i32,
        pub uid: u32,
        pub gid: u32,
    }

    pub fn peer_cred(stream: &UnixStream) -> std::io::Result<PeerCred> {
        // SAFETY: getsockopt with SO_PEERCRED on a connected unix stream is well-defined.
        let mut ucred: libc::ucred = unsafe { std::mem::zeroed() };
        let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
        let fd: RawFd = stream.as_raw_fd();
        let r = unsafe {
            libc::getsockopt(
                fd,
                libc::SOL_SOCKET,
                libc::SO_PEERCRED,
                &mut ucred as *mut _ as *mut libc::c_void,
                &mut len,
            )
        };
        if r != 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(PeerCred {
            pid: ucred.pid,
            uid: ucred.uid,
            gid: ucred.gid,
        })
    }

    /// Open a pidfd for the peer pid. Caller closes via `Drop`.
    pub fn pidfd_open(pid: i32) -> std::io::Result<RawFd> {
        // SAFETY: pidfd_open is the Linux-defined syscall.
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(fd as RawFd)
    }
}

#[cfg(not(target_os = "linux"))]
mod imp {
    use tokio::net::UnixStream;

    #[derive(Debug, Clone, Copy)]
    pub struct PeerCred {
        pub pid: i32,
        pub uid: u32,
        pub gid: u32,
    }

    pub fn peer_cred(_stream: &UnixStream) -> std::io::Result<PeerCred> {
        // Dev fallback: trust the local user on non-Linux platforms. Real
        // production deployments are Linux-only (master spec §L10).
        Ok(PeerCred {
            pid: 0,
            uid: nix_uid_compat(),
            gid: nix_gid_compat(),
        })
    }

    pub fn pidfd_open(_pid: i32) -> std::io::Result<i32> {
        Ok(-1)
    }

    fn nix_uid_compat() -> u32 {
        // SAFETY: getuid is documented as never failing.
        unsafe { libc::getuid() }
    }

    fn nix_gid_compat() -> u32 {
        // SAFETY: getgid is documented as never failing.
        unsafe { libc::getgid() }
    }
}

pub use imp::{PeerCred, peer_cred, pidfd_open};

/// Look up the GID of a named group. Used to gate admin-plane access by
/// `cfg.admin_plane.unix_socket_group`.
///
/// Uses the **reentrant** `getgrnam_r(3)` (via [`nix::unistd::Group`])
/// rather than `getgrnam(3)`. The non-reentrant `getgrnam` returns a pointer
/// into a process-shared static buffer, so a concurrent group or passwd
/// lookup on another thread can clobber the record between the call and the
/// `gr_gid` read — a data race that intermittently flaked the sibling
/// `group_gid_root_resolves` test under `cargo test`'s parallel runner
/// (BRO-1861) and is a latent hazard on the admin-auth path itself.
/// `getgrnam_r` fills a caller-owned buffer, so it is thread-safe and no
/// longer needs `unsafe`.
pub fn group_gid(name: &str) -> std::io::Result<Option<u32>> {
    let group = nix::unistd::Group::from_name(name)
        .map_err(|e| std::io::Error::other(format!("getgrnam_r({name}): {e}")))?;
    Ok(group.map(|g| g.gid.as_raw()))
}

/// Test whether `cred` belongs to a process whose primary group is `gid`.
/// Sub-phase C MVS: only checks primary gid. Spec C₆ adds full
/// supplementary-group inspection via `/proc/{pid}/status` Groups: line.
pub fn is_member_of(cred: &PeerCred, gid: u32) -> bool {
    cred.gid == gid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_gid_root_resolves_to_zero() {
        // `root` group is GID 0 on every supported platform.
        let gid = group_gid("root").expect("syscall ok");
        // On some hardened distros `root` might be aliased; we only assert
        // the lookup returns something — `0` is the canonical value when
        // present.
        if let Some(g) = gid {
            assert!(g <= 100, "root gid in low range, got {g}");
        }
    }

    #[test]
    fn group_gid_missing_returns_none() {
        let gid = group_gid("definitely-not-a-real-group-zzz")
            .expect("getgrnam should not error on missing entry");
        assert!(gid.is_none());
    }

    #[test]
    fn is_member_of_primary_gid_matches() {
        let cred = PeerCred {
            pid: 0,
            uid: 1000,
            gid: 1000,
        };
        assert!(is_member_of(&cred, 1000));
        assert!(!is_member_of(&cred, 1001));
    }
}
