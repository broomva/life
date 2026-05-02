//! SO_PEERCRED + group-membership extractor for the lifegw admin
//! plane. Sub-phase D (D2).
//!
//! Mirrors the lifed implementation at
//! `crates/life-runtime/lifed/src/auth/peercred.rs` — see the
//! comments there for the security rationale (SO_PEERCRED on Linux,
//! `getuid()` fallback on macOS for dev-box convenience).
//!
//! This module is the only place lifegw reaches for `unsafe`.
//! `lib.rs` denies unsafe at the crate root; we re-allow it here for
//! the syscall block.

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
        // SAFETY: getsockopt with SO_PEERCRED on a connected unix
        // stream is well-defined.
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
        // Dev fallback: trust the local user on non-Linux platforms.
        // Production deployments are Linux-only (master spec §L10).
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

/// Look up the GID of a named group. Used to gate admin-plane access
/// by `cfg.admin_plane.unix_socket_group`.
pub fn group_gid(name: &str) -> std::io::Result<Option<u32>> {
    use std::ffi::CString;
    let cname = CString::new(name)
        .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidInput, "group name has nul"))?;
    // SAFETY: getgrnam returns a raw pointer that points to
    // thread-static memory; we copy out the gid before another call
    // could clobber it.
    let g = unsafe { libc::getgrnam(cname.as_ptr()) };
    if g.is_null() {
        return Ok(None);
    }
    Ok(Some(unsafe { (*g).gr_gid }))
}

/// Test whether `cred` belongs to a process whose primary group is
/// `gid`. Sub-phase D MVS only checked the primary gid; Sub-phase E
/// sweep (item #12) extends this with [`supplementary_gids_of_uid`].
pub fn is_member_of(cred: &PeerCred, gid: u32) -> bool {
    cred.gid == gid
}

/// Sub-phase E sweep (item #12): query the supplementary groups for a
/// uid via `getgrouplist(3)` (not by reading `/etc/group` directly).
/// Returns the full list of supplementary GIDs for the user, including
/// the user's primary GID.
///
/// **Linux/BSD only.** `getgrouplist` is not available on Apple
/// platforms (the libc has it but the nix wrapper opts out — see
/// `nix::unistd::getgrouplist` cfg gate). On macOS / non-Linux the
/// function returns `Ok(Vec::new())` so callers fall back to the
/// primary-gid path. Production deploys are Linux-only per master spec
/// §L10 so the macOS fallback is dev-box-only.
///
/// Sub-phase E sweep (item #13): on lookup error (Linux user not in
/// `/etc/passwd`, or `getgrouplist` syscall failure), the function
/// returns `Err`. Callers use `unwrap_or_default()` only when their
/// fail-mode policy is permit; admin policy now fails CLOSED via the
/// `gateway.admin.rejected_total{reason="group_lookup"}` counter.
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

/// macOS / non-Linux fallback. Returns `Ok(Vec::new())` so callers can
/// proceed with primary-gid checks. Production is Linux-only.
#[cfg(not(target_os = "linux"))]
pub fn supplementary_gids_of_uid(_uid: u32) -> std::io::Result<Vec<u32>> {
    Ok(Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn group_gid_root_resolves() {
        // `root` group is GID 0 on every supported platform.
        let gid = group_gid("root").expect("syscall ok");
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

    /// Sub-phase E sweep (item #12): supplementary group lookup. On
    /// Linux this returns the actual group list; on macOS it returns
    /// an empty vec (the policy table falls back to primary-GID
    /// checks).
    #[test]
    #[cfg(target_os = "linux")]
    fn supplementary_gids_of_uid_resolves_root() {
        // root (uid 0) always exists and is at minimum a member of the
        // root group (gid 0). The test proves the syscall pipeline
        // works without depending on /etc/group format.
        let groups = supplementary_gids_of_uid(0).expect("getgrouplist(root)");
        assert!(
            groups.contains(&0),
            "root must be in its own primary group (gid 0): got {groups:?}"
        );
    }

    #[test]
    fn supplementary_gids_of_uid_unknown_user_errors_or_empty() {
        // uid `4294967295` (u32::MAX) cannot exist in /etc/passwd. On
        // Linux this errors with NotFound; on macOS the fallback
        // returns an empty vec (no syscall). Both behaviours are
        // acceptable per the function contract.
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
