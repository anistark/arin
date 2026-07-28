//! A socket client.
//!
//! Shared by the CLI and the MCP server, so both speak the wire protocol through exactly
//! one implementation. Lives here rather than in `arin-protocol` because it does IO, and
//! the protocol crate must stay free of it.

use crate::codec::LineReader;
use crate::config::default_socket_path;
use crate::error::{Error, Result};
use arin_protocol::{
    ClientMessage, DaemonMessage, Envelope, Invalidated, PROTOCOL_VERSION, SessionId, SessionStart,
};
use std::path::Path;
use tokio::io::{AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};

/// A connection to a running daemon.
///
/// # Two kinds of message
///
/// The daemon answers every request with an ack or an error, and separately pushes an
/// `invalidated` whenever a mark goes away for a reason nobody asked for: content
/// scrolled, a time to live ran out, or the user cleared the screen. Those arrive
/// whenever they happen, including in the middle of waiting for a reply.
///
/// [`Client::send`] therefore reads until it sees a reply, setting aside any invalidation
/// it passes on the way. Collect them with [`Client::take_invalidations`], or wait for one
/// with [`Client::next_invalidation`] when there is no request in flight.
pub struct Client {
    reader: LineReader<BufReader<OwnedReadHalf>>,
    writer: OwnedWriteHalf,
    session: Option<SessionId>,
    /// Pushes that arrived while waiting for a reply, oldest first.
    invalidations: Vec<Invalidated>,
}

impl Client {
    /// Connect to the default socket path.
    pub async fn connect() -> Result<Self> {
        Self::connect_to(default_socket_path()).await
    }

    /// Connect to a specific socket path.
    pub async fn connect_to(path: impl AsRef<Path>) -> Result<Self> {
        let stream = UnixStream::connect(path.as_ref()).await?;
        let (read_half, writer) = stream.into_split();
        Ok(Self {
            reader: LineReader::new(BufReader::new(read_half)),
            writer,
            session: None,
            invalidations: Vec::new(),
        })
    }

    /// The session this client holds, once started.
    pub fn session(&self) -> Option<&SessionId> {
        self.session.as_ref()
    }

    /// Open a session and remember the id the daemon hands back.
    pub async fn start_session(&mut self, client_name: impl Into<String>) -> Result<SessionId> {
        let reply = self
            .send(ClientMessage::SessionStart(SessionStart {
                client_name: client_name.into(),
            }))
            .await?;

        match reply {
            DaemonMessage::Ack(ack) => {
                let id = ack.session_id.ok_or_else(|| {
                    Error::PeerRejected("daemon acked session_start without a session id".into())
                })?;
                self.session = Some(id.clone());
                Ok(id)
            }
            DaemonMessage::Error(e) => Err(Error::PeerRejected(e.msg)),
            other => Err(Error::PeerRejected(format!(
                "unexpected reply to session_start: {other:?}"
            ))),
        }
    }

    /// Send one message and wait for the reply.
    ///
    /// Invalidations that arrive first are set aside rather than mistaken for the reply.
    /// Reading exactly one line here would return a push to a caller expecting its own
    /// answer, and leave the real answer to be read as the reply to the *next* request,
    /// with the two drifting further apart from then on.
    pub async fn send(&mut self, message: ClientMessage) -> Result<DaemonMessage> {
        let mut line = serde_json::to_vec(&Envelope::new(PROTOCOL_VERSION, &message))?;
        line.push(b'\n');
        self.writer.write_all(&line).await?;
        self.writer.flush().await?;

        loop {
            match self.read_one().await? {
                DaemonMessage::Invalidated(event) => self.invalidations.push(event),
                reply => return Ok(reply),
            }
        }
    }

    /// Take everything that went away since this was last called.
    ///
    /// Empty is the normal answer. A client that never asks simply never learns, which is
    /// why this does not block: the marks are already gone by the time it is told.
    pub fn take_invalidations(&mut self) -> Vec<Invalidated> {
        std::mem::take(&mut self.invalidations)
    }

    /// Wait for the next invalidation.
    ///
    /// Only safe with no request in flight, since it consumes whatever arrives next. Use
    /// it to watch a mark, not to drive one.
    pub async fn next_invalidation(&mut self) -> Result<Invalidated> {
        if !self.invalidations.is_empty() {
            return Ok(self.invalidations.remove(0));
        }
        loop {
            if let DaemonMessage::Invalidated(event) = self.read_one().await? {
                return Ok(event);
            }
            // A reply with nothing waiting on it. Nothing sensible to do with it, and
            // discarding it beats blocking forever on a line that is never coming.
        }
    }

    /// Read one message, whatever it turns out to be.
    async fn read_one(&mut self) -> Result<DaemonMessage> {
        let line = self
            .reader
            .next_line()
            .await?
            .ok_or_else(|| Error::PeerRejected("daemon closed the connection".into()))?;
        let envelope: Envelope<DaemonMessage> = serde_json::from_str(line)?;
        Ok(envelope.body)
    }

    /// Close the session. Annotations fade shortly after.
    pub async fn end_session(&mut self) -> Result<()> {
        if self.session.is_some() {
            self.send(ClientMessage::SessionEnd).await?;
            self.session = None;
        }
        Ok(())
    }
}
