//! Client sessions.
//!
//! Annotations are scoped to a session. Multiple clients may hold sessions at once, and
//! each has its own namespace and cannot see or clear another's annotations.

use arin_protocol::SessionId;
use std::time::Instant;

/// One connected client.
#[derive(Debug, Clone)]
pub struct Session {
    /// Opaque identifier handed back on `session_start`.
    pub id: SessionId,
    /// What the client called itself. Diagnostics only, never trusted for authorisation.
    pub client_name: String,
    /// When the session opened.
    pub started: Instant,
}

impl Session {
    /// Open a session for a named client.
    pub fn new(client_name: impl Into<String>) -> Self {
        Self {
            id: crate::annotation::next_session_id(),
            client_name: client_name.into(),
            started: Instant::now(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_get_distinct_ids() {
        let a = Session::new("claude-code");
        let b = Session::new("claude-code");
        assert_ne!(a.id, b.id);
        assert_eq!(a.client_name, "claude-code");
    }
}
