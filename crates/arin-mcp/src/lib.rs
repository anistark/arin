//! MCP server for Arin.
//!
//! Translates MCP tool calls into protocol messages on the daemon socket. It holds no
//! state of its own beyond a session: it is a socket client like any other, and adds no
//! capability the protocol does not already expose.
//!
//! # Status: reserved for 0.2
//!
//! The tool surface below is settled. The server binding is not written yet. 0.2 is the
//! launch release, and this crate plus the demo video are what it consists of.

#![forbid(unsafe_code)]
#![warn(missing_docs)]

/// The MCP tool names this server exposes.
///
/// Named after what an agent is trying to do, not after the protocol message underneath,
/// so intents phrase naturally in a model's own words. `point_at` reads better in a tool
/// call than `point`, and `annotate` is what an agent reaches for when it wants to leave
/// an explanation rather than a mark.
pub mod tools {
    /// Put the orb on a target. Maps to `point`.
    pub const POINT_AT: &str = "point_at";
    /// Outline a region. Maps to `highlight`.
    pub const HIGHLIGHT: &str = "highlight";
    /// Place explanatory text. Maps to `textbox`.
    pub const ANNOTATE: &str = "annotate";
    /// Remove annotations. Maps to `clear`.
    pub const CLEAR: &str = "clear";

    /// Every tool name, for registration and for tests.
    pub const ALL: &[&str] = &[POINT_AT, HIGHLIGHT, ANNOTATE, CLEAR];
}

/// The name this server reports to the daemon on `session_start`.
pub const CLIENT_NAME: &str = "arin-mcp";

#[cfg(test)]
mod tests {
    use super::tools;

    #[test]
    fn tool_names_are_unique() {
        let mut sorted = tools::ALL.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), tools::ALL.len());
    }

    #[test]
    fn tool_names_are_snake_case() {
        for name in tools::ALL {
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not snake_case"
            );
        }
    }
}
