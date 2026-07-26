//! Peer credential checks.
//!
//! The socket is created mode 0600 inside a directory only the user can traverse, so
//! filesystem permissions already do most of the work. This is the belt to that pair of
//! braces: it asks the kernel who is actually on the other end, which cannot be spoofed
//! by a client that talks its way past a path check.
//!
//! This is the baseline, not the finished security model. Whether `session_start` also
//! carries a token, and where such a token would live, is still open and blocks tagging
//! the protocol.

use crate::error::{Error, Result};

/// Who is on the other end of a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PeerCredentials {
    /// Effective user id of the connecting process.
    pub uid: u32,
    /// Effective group id, when the platform reports one.
    pub gid: Option<u32>,
    /// Process id, when the platform reports one. Advisory only: it can be recycled.
    pub pid: Option<i32>,
}

impl PeerCredentials {
    /// Reject the peer unless it runs as the same user as this daemon.
    pub fn require_same_uid(&self) -> Result<()> {
        let ours = current_uid();
        if self.uid == ours {
            Ok(())
        } else {
            Err(Error::PeerRejected(format!(
                "peer uid {} does not match daemon uid {ours}",
                self.uid
            )))
        }
    }
}

/// The effective uid of this process.
#[cfg(unix)]
pub fn current_uid() -> u32 {
    // SAFETY: `geteuid` takes no arguments, touches no memory, and cannot fail.
    #[allow(unsafe_code)]
    unsafe {
        libc::geteuid()
    }
}

/// The effective uid of this process.
#[cfg(not(unix))]
pub fn current_uid() -> u32 {
    0
}

/// Read the credentials of the process on the other end of a connected Unix socket.
#[cfg(any(target_os = "linux", target_os = "android"))]
pub fn credentials(sock: &tokio::net::UnixStream) -> Result<PeerCredentials> {
    use std::os::fd::AsRawFd;

    let mut cred = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = size_of::<libc::ucred>() as libc::socklen_t;

    // SAFETY: `cred` and `len` are live, correctly sized, and match what SO_PEERCRED
    // writes. The fd is owned by `sock` and outlives this call.
    #[allow(unsafe_code)]
    let rc = unsafe {
        libc::getsockopt(
            sock.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &raw mut len,
        )
    };

    if rc != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(PeerCredentials {
        uid: cred.uid,
        gid: Some(cred.gid),
        pid: Some(cred.pid),
    })
}

/// Read the credentials of the process on the other end of a connected Unix socket.
#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
))]
pub fn credentials(sock: &tokio::net::UnixStream) -> Result<PeerCredentials> {
    use std::os::fd::AsRawFd;

    let mut uid: libc::uid_t = 0;
    let mut gid: libc::gid_t = 0;

    // SAFETY: both out-pointers are live, correctly typed locals. The fd is owned by
    // `sock` and outlives this call.
    #[allow(unsafe_code)]
    let rc = unsafe { libc::getpeereid(sock.as_raw_fd(), &raw mut uid, &raw mut gid) };

    if rc != 0 {
        return Err(Error::Io(std::io::Error::last_os_error()));
    }
    Ok(PeerCredentials {
        uid,
        gid: Some(gid),
        // getpeereid does not report a pid, and LOCAL_PEERPID is not worth the extra
        // syscall for a value that is advisory anyway.
        pid: None,
    })
}

/// Read the credentials of the process on the other end of a connected Unix socket.
#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd"
)))]
pub fn credentials(_sock: &tokio::net::UnixStream) -> Result<PeerCredentials> {
    Err(Error::PeerRejected(
        "peer credentials are not available on this platform".into(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_local_peer_is_us() {
        let (a, _b) = tokio::net::UnixStream::pair().unwrap();
        let cred = credentials(&a).expect("credentials on a socketpair");
        assert_eq!(cred.uid, current_uid());
        cred.require_same_uid().unwrap();
    }

    #[test]
    fn a_foreign_uid_is_rejected() {
        let stranger = PeerCredentials {
            uid: current_uid().wrapping_add(1),
            gid: None,
            pid: None,
        };
        assert!(matches!(
            stranger.require_same_uid(),
            Err(Error::PeerRejected(_))
        ));
    }
}
