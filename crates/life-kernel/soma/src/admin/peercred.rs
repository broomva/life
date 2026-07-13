//! SO_PEERCRED extractor for soma's admin custody UDS (Spec D D-Sub-E).
//!
//! Mirrors the lifegw implementation at
//! `crates/life-runtime/lifegw/src/admin/peercred.rs`. soma's admin
//! plane reaches `unsafe` only for the SO_PEERCRED syscall — `lib.rs`
//! denies unsafe at the crate root, this module re-allows it locally.
//! Group lookups go through the safe reentrant `nix::unistd::Group`
//! wrapper (`getgrnam_r`), not raw `getgrnam`.

#![allow(unsafe_code)]

#[cfg(target_os = "linux")]
mod imp {
    use std::os::fd::{AsRawFd, RawFd};
    use tokio::net::UnixStream;

    /// SO_PEERCRED ucred — `(pid, uid, gid)` of the connecting peer.
    ///
    /// On Linux the kernel exposes this via `getsockopt(SOL_SOCKET,
    /// SO_PEERCRED)`; on macOS we fall back to the local `getuid()` /
    /// `getgid()` pair (production deploys are Linux-only per Spec D
    /// §"Phasing > D-Sub-E", so this is a dev-box convenience).
    #[derive(Debug, Clone, Copy)]
    pub struct PeerCred {
        pub pid: i32,
        pub uid: u32,
        pub gid: u32,
    }

    pub fn peer_cred(stream: &UnixStream) -> std::io::Result<PeerCred> {
        // SAFETY: getsockopt with SO_PEERCRED on a connected unix
        // stream is well-defined and the resulting `ucred` has
        // initialised `pid`/`uid`/`gid` fields.
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
        // Dev fallback on non-Linux platforms: trust the local user.
        // Production is Linux-only.
        Ok(PeerCred {
            pid: 0,
            uid: nix_uid_compat(),
            gid: nix_gid_compat(),
        })
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

pub use imp::{PeerCred, peer_cred};

/// Look up the GID of a named group via the **reentrant** `getgrnam_r(3)`
/// (through [`nix::unistd::Group`]).
///
/// `getgrnam(3)` (the non-reentrant variant) returns a pointer into a
/// process-shared static buffer, so a concurrent group or passwd lookup on
/// another thread can clobber the record between the call and the `gr_gid`
/// read — a data race that intermittently flaked the sibling
/// `group_gid_root_resolves` test under `cargo test`'s parallel runner
/// (BRO-1861) and is a latent hazard on the admin-auth path itself.
/// `getgrnam_r` fills a caller-owned buffer, so it is thread-safe and no
/// longer needs `unsafe`.
pub fn group_gid(name: &str) -> std::io::Result<Option<u32>> {
    let group = nix::unistd::Group::from_name(name)
        .map_err(|e| std::io::Error::other(format!("getgrnam_r({name}): {e}")))?;
    Ok(group.map(|g| g.gid.as_raw()))
}

/// Sub-phase E parity: query supplementary groups for a uid.
///
/// Linux: `getgrouplist(3)` via the safe `nix` wrapper. Returns `Err`
/// when the uid is not in `/etc/passwd` so the policy can fail-CLOSED.
/// macOS / non-Linux: returns `Ok(Vec::new())` so callers fall back
/// to the primary-gid path. Production is Linux-only.
#[cfg(target_os = "linux")]
pub fn supplementary_gids_of_uid(uid: u32) -> std::io::Result<Vec<u32>> {
    use std::ffi::CString;
    let user = nix::unistd::User::from_uid(nix::unistd::Uid::from_raw(uid))
        .map_err(|e| std::io::Error::other(format!("getpwuid_r({uid}): {e}")))?
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("uid {uid} not in /etc/passwd"),
            )
        })?;
    let cname = CString::new(user.name.as_bytes())
        .map_err(|e| std::io::Error::other(format!("user name has nul: {e}")))?;
    let groups = nix::unistd::getgrouplist(&cname, user.gid)
        .map_err(|e| std::io::Error::other(format!("getgrouplist({uid}): {e}")))?;
    Ok(groups.into_iter().map(|g| g.as_raw()).collect())
}

#[cfg(not(target_os = "linux"))]
pub fn supplementary_gids_of_uid(_uid: u32) -> std::io::Result<Vec<u32>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_gid_root_resolves() {
        let gid = group_gid("root").expect("syscall ok");
        if let Some(g) = gid {
            assert!(g <= 100);
        }
    }

    #[test]
    fn group_gid_missing_returns_none() {
        let gid = group_gid("definitely-not-a-real-group-zzz")
            .expect("getgrnam should not error on missing entry");
        assert!(gid.is_none());
    }

    #[test]
    fn supplementary_gids_of_unknown_user_errors_or_empty() {
        let outcome = supplementary_gids_of_uid(u32::MAX);
        if cfg!(target_os = "linux") {
            assert!(outcome.is_err(), "missing uid must error on Linux");
        } else {
            assert!(
                outcome.unwrap().is_empty(),
                "non-Linux fallback returns empty"
            );
        }
    }
}
