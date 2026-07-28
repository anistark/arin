//! The Unix domain socket server.
//!
//! There is no network transport and none is planned. The socket lives inside a
//! directory only its owner can traverse, is created mode 0600, and every accepted
//! connection has its peer credentials checked before a byte is read.

use crate::codec::LineReader;
use crate::daemon::{Connection, Daemon};
use crate::error::{Error, Result};
use crate::peer;
use arin_protocol::{ClientMessage, DaemonMessage, Envelope, PROTOCOL_VERSION};
use std::path::Path;
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::sync::broadcast;

/// Accepts client connections and drives the daemon.
pub struct Server {
    daemon: Arc<Daemon>,
    listener: UnixListener,
}

impl Server {
    /// Bind the socket described by the daemon's config.
    ///
    /// Fails if another daemon already holds the socket. A stale socket left by a
    /// crashed daemon is detected and removed.
    pub fn bind(daemon: Arc<Daemon>) -> Result<Self> {
        let path = daemon.config().socket_path.clone();
        check_path_length(&path)?;

        if let Some(dir) = path.parent() {
            // Only lock down a directory we created ourselves. A socket path pointing
            // into an existing directory, the XDG runtime dir or wherever the user
            // configured, is not ours to chmod, and trying fails outright on a shared
            // directory like /tmp.
            if !dir.exists() {
                std::fs::create_dir_all(dir)?;
                restrict(dir, 0o700)?;
            }
        }
        clear_stale_socket(&path)?;

        let listener = UnixListener::bind(&path)?;
        restrict(&path, 0o600)?;

        tracing::info!(socket = %path.display(), "listening");
        Ok(Self { daemon, listener })
    }

    /// The path the server is listening on.
    pub fn socket_path(&self) -> &Path {
        &self.daemon.config().socket_path
    }

    /// Accept connections until cancelled.
    pub async fn run(self) -> Result<()> {
        loop {
            let (stream, _) = self.listener.accept().await?;

            if let Err(e) = self.admit(&stream) {
                tracing::warn!(error = %e, "rejected a connection");
                continue;
            }

            let daemon = Arc::clone(&self.daemon);
            tokio::spawn(async move {
                if let Err(e) = serve(daemon, stream).await {
                    tracing::debug!(error = %e, "connection closed");
                }
            });
        }
    }

    /// Check who is on the other end before reading anything from them.
    fn admit(&self, stream: &UnixStream) -> Result<()> {
        if !self.daemon.config().require_same_uid {
            return Ok(());
        }
        let credentials = peer::credentials(stream)?;
        credentials.require_same_uid()?;
        tracing::debug!(uid = credentials.uid, pid = ?credentials.pid, "admitted");
        Ok(())
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        // Leave nothing behind for the next start to have to reason about.
        let _ = std::fs::remove_file(&self.daemon.config().socket_path);
    }
}

/// Read messages from one client until it goes away, and push what it should know.
///
/// Two things arrive on this connection from opposite directions: requests from the
/// client, and invalidations the daemon raised on its own because content scrolled, a
/// time to live ran out, or the user cleared the screen. Both are handled in one select
/// so there is only ever a single writer, and so a client sitting idle still hears that
/// its marks went away.
async fn serve(daemon: Arc<Daemon>, stream: UnixStream) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = LineReader::new(BufReader::new(read_half));
    let mut announcements = daemon.subscribe();

    // Dropping this at the end of the function ends the session and clears its
    // annotations, so a client that dies mid-explanation cannot leave marks behind.
    let mut connection = Connection::new(daemon);

    loop {
        tokio::select! {
            // Biased so a request already on the wire is answered before a push goes out.
            // Without it a busy daemon could interleave the two in a different order on
            // every run, which makes a failing test a coin toss.
            biased;

            incoming = reader.next_line() => {
                let line = match incoming {
                    Ok(Some(line)) => line.to_owned(),
                    Ok(None) => break,
                    // An oversized line desynchronises the stream: there is no safe point
                    // to resume from, so say why and hang up.
                    Err(e @ Error::PayloadTooLarge) => {
                        let _ = write(&mut write_half, &DaemonMessage::Error(e.to_wire())).await;
                        break;
                    }
                    Err(e) => return Err(e),
                };

                let reply = match serde_json::from_str::<Envelope<ClientMessage>>(&line) {
                    Ok(envelope) => match connection.handle(envelope).await {
                        Ok(reply) => reply,
                        Err(e) => DaemonMessage::Error(e.to_wire()),
                    },
                    // A malformed or unknown message is answered, not fatal. Closing the
                    // connection would punish a client for a single typo mid-session.
                    Err(e) => DaemonMessage::Error(Error::from(e).to_wire()),
                };

                write(&mut write_half, &reply).await?;
            }

            announced = announcements.recv() => {
                match announced {
                    Ok(announcement) => {
                        // Only ever the owner's own marks. A session must not learn that
                        // another session's annotation went away, for the same reason
                        // `clear` answers `not_owner` to both a missing annotation and
                        // someone else's.
                        if connection.session() == Some(&announcement.session) {
                            write(
                                &mut write_half,
                                &DaemonMessage::Invalidated(announcement.event),
                            )
                            .await?;
                        }
                    }
                    // This client stopped reading for long enough to fall behind. The
                    // marks are gone either way, so carry on rather than hang up: a
                    // missed notification is a smaller failure than a dropped session.
                    Err(broadcast::error::RecvError::Lagged(missed)) => {
                        tracing::warn!(missed, "a client fell behind on invalidations");
                    }
                    // The daemon is gone, so there is nothing left to serve.
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }

    Ok(())
}

async fn write<W: AsyncWriteExt + Unpin>(sink: &mut W, message: &DaemonMessage) -> Result<()> {
    let mut line = serde_json::to_vec(&Envelope::new(PROTOCOL_VERSION, message))?;
    line.push(b'\n');
    sink.write_all(&line).await?;
    sink.flush().await?;
    Ok(())
}

/// The longest socket path the platform will accept.
///
/// `sun_path` is a fixed-size array in `sockaddr_un`: 104 bytes on macOS and the BSDs,
/// 108 on Linux, including the terminator. Use the smaller bound everywhere so a config
/// that works on one platform is not silently over the line on another.
const MAX_SOCKET_PATH_BYTES: usize = 103;

/// Reject an over-long socket path with an explanation.
///
/// Worth its own check because the raw failure is `EINVAL` or a bare "path must be
/// shorter than SUN_LEN", neither of which tells anyone what to do about it. Long
/// temporary directories hit this easily.
fn check_path_length(path: &Path) -> Result<()> {
    let len = path.as_os_str().as_encoded_bytes().len();
    if len > MAX_SOCKET_PATH_BYTES {
        return Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "socket path is {len} bytes, over the {MAX_SOCKET_PATH_BYTES} byte limit \
                 for Unix domain sockets: {}",
                path.display()
            ),
        )));
    }
    Ok(())
}

/// Remove a socket file left behind by a crashed daemon.
///
/// Distinguishes stale from live by connecting: if something answers, another daemon
/// owns this path and we must not unlink it.
fn clear_stale_socket(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    match std::os::unix::net::UnixStream::connect(path) {
        Ok(_) => Err(Error::Io(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("another daemon is already listening on {}", path.display()),
        ))),
        Err(_) => {
            tracing::info!(socket = %path.display(), "removing stale socket");
            std::fs::remove_file(path)?;
            Ok(())
        }
    }
}

/// Set permissions on a path, on platforms that have them.
#[cfg(unix)]
fn restrict(path: &Path, mode: u32) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

/// Set permissions on a path, on platforms that have them.
#[cfg(not(unix))]
fn restrict(_path: &Path, _mode: u32) -> Result<()> {
    Ok(())
}
