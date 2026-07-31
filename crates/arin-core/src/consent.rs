//! Who is allowed to make Arin look at the screen.
//!
//! The security model, settled 2026-07-31. `plan/SECURITY.md` holds the argument and this
//! is the part of it that runs.
//!
//! # The finding this exists for
//!
//! Arin holds the Screen Recording grant. A client does not. So a process with no screen
//! access of its own can send `point {query: "the row showing the account balance"}` and
//! read coordinates and a confidence back out of the ack. That is screen content, laundered
//! through Arin's grant, at whatever bitrate patient questioning affords, and on macOS it is
//! a real privilege escalation because Screen Recording is exactly the permission the system
//! makes a user grant deliberately.
//!
//! # Why drawing is not gated and grounding is
//!
//! Drawing is not a privilege. A process running as the user can make its own always-on-top
//! window and draw whatever it likes, so gating it would cost every client a setup step and
//! buy nothing. Grounding is a privilege, and it is the only one Arin actually holds.
//!
//! That asymmetry is the whole design. It is also why nothing here touches `session_start`:
//! a capability split is enforced on the two message forms that carry a query, so the
//! handshake is unchanged and `arin-protocol` does not need republishing.
//!
//! # Why permission is granted for a window rather than to a client
//!
//! Because there is nothing trustworthy to grant it *to*. `client_name` is self declared,
//! so a hostile client claims to be `claude-code`, and binding to a peer pid is platform
//! specific work in a crate that is supposed to have no platform code. That decision is
//! deliberately deferred.
//!
//! Granting for a window needs no identity at all. It also happens to be the only shape
//! that works with the CLI, where every command is a new session: per-session approval
//! would mean a prompt for every single `arin point "the Submit button"`, which nobody would
//! tolerate for long enough to keep it switched on. A control people turn off is worse than
//! a weaker control they leave on.
//!
//! What it does not do is distinguish clients. While a window is open, any same-uid peer can
//! ground, not just the one that asked. That is a real limit and it is the reason the
//! identity question is deferred rather than dropped.

use std::time::{Duration, Instant};

/// How the daemon decides whether to ground a query.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Consent {
    /// Ask the user, through whatever [`crate::Approver`] the platform wired up.
    ///
    /// The default. With no approver available the answer is no, which is the direction a
    /// security control has to fail in: a gate that opens when nobody is watching is not a
    /// gate.
    #[default]
    Ask,
    /// Ground without asking.
    ///
    /// For a daemon nobody is sitting in front of. It is a real loosening rather than a
    /// convenience, so the daemon says so at startup.
    Always,
    /// Refuse every query, whatever is configured.
    ///
    /// Distinct from starting with no resolver: this leaves a resolver built and reports
    /// the refusal as `not_permitted` rather than `no_resolver`, which is what a client
    /// needs to tell "not set up" from "declined".
    Never,
}

impl Consent {
    /// Read a consent mode from configuration.
    pub fn parse(raw: &str) -> Result<Self, String> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "ask" => Ok(Self::Ask),
            "always" | "allow" => Ok(Self::Always),
            "never" | "deny" | "off" => Ok(Self::Never),
            other => Err(format!(
                "{other:?} is not a consent mode. Use `ask`, `always`, or `never`"
            )),
        }
    }
}

/// What a client wants to do that needs permission.
///
/// Everything an approver could reasonably put in front of a person. The query is included
/// because it is the most informative part: "the Submit button" and "the row showing the
/// account balance" are the same request to the daemon and very different requests to read
/// out loud.
#[derive(Debug, Clone)]
pub struct Request {
    /// What the client called itself.
    ///
    /// Self declared and never trusted for authorisation. Shown to the user as a label,
    /// because a person reading a prompt is better placed to smell a lie than the daemon
    /// is, and worse off with no name at all.
    pub client_name: String,
    /// What the client asked to find.
    pub query: String,
    /// Which resolver would run.
    pub resolver: String,
    /// Whether that resolver sends the screen off the machine.
    ///
    /// The single most important thing on the prompt. Granting a local model a look at the
    /// screen and granting a hosted one a copy of it are not the same decision.
    pub remote: bool,
}

/// What the user said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Ground this one query and ask again next time.
    Once,
    /// Ground anything asked for over this long, then ask again.
    For(Duration),
    /// Refuse, and ask again next time.
    Deny,
}

/// A grant that is running down.
///
/// Held by the daemon, consulted before every grounding request, and deliberately not
/// persisted anywhere. A permission that survives a restart is one nobody remembers giving.
#[derive(Debug, Default)]
pub struct Grant {
    until: Option<Instant>,
}

impl Grant {
    /// Nothing granted.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a grant is currently open.
    pub fn is_open(&self) -> bool {
        self.remaining().is_some()
    }

    /// How much of the grant is left.
    pub fn remaining(&self) -> Option<Duration> {
        self.until
            .map(|until| until.saturating_duration_since(Instant::now()))
            .filter(|left| !left.is_zero())
    }

    /// Record what the user said, and answer whether this request may proceed.
    pub fn record(&mut self, decision: Decision) -> bool {
        match decision {
            Decision::Once => true,
            Decision::For(window) => {
                self.until = Some(Instant::now() + window);
                true
            }
            // Clears any open window as well as refusing. Someone answering no to a prompt
            // means no, and leaving an earlier grant running would make the prompt a lie.
            Decision::Deny => {
                self.until = None;
                false
            }
        }
    }

    /// Revoke any open grant.
    ///
    /// The menu bar's route out, and what makes granting an hour a decision somebody can
    /// take back rather than one they have to wait out.
    pub fn revoke(&mut self) {
        self.until = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_is_to_ask() {
        assert_eq!(Consent::default(), Consent::Ask);
    }

    #[test]
    fn consent_modes_parse_and_nonsense_does_not() {
        assert_eq!(Consent::parse("ask").unwrap(), Consent::Ask);
        assert_eq!(Consent::parse(" ALWAYS ").unwrap(), Consent::Always);
        assert_eq!(Consent::parse("never").unwrap(), Consent::Never);
        assert!(Consent::parse("sometimes").is_err());
    }

    #[test]
    fn a_fresh_grant_is_closed() {
        assert!(!Grant::new().is_open());
        assert_eq!(Grant::new().remaining(), None);
    }

    /// Allowing once must not open a window. It is the answer for somebody who wants this
    /// query grounded and nothing else, and treating it as a grant would silently widen it.
    #[test]
    fn allowing_once_permits_the_request_and_grants_nothing() {
        let mut grant = Grant::new();
        assert!(grant.record(Decision::Once));
        assert!(!grant.is_open(), "once is not a window");
    }

    #[test]
    fn a_window_stays_open_until_it_runs_out() {
        let mut grant = Grant::new();
        assert!(grant.record(Decision::For(Duration::from_secs(3600))));
        assert!(grant.is_open());
        assert!(grant.remaining().unwrap() > Duration::from_secs(3500));

        let mut expired = Grant::new();
        assert!(expired.record(Decision::For(Duration::ZERO)));
        assert!(!expired.is_open(), "a window of no time is not a window");
    }

    /// Answering no has to mean no. Leaving an earlier grant running would make the prompt
    /// somebody just answered into a thing that did nothing.
    #[test]
    fn denying_closes_a_window_that_was_already_open() {
        let mut grant = Grant::new();
        grant.record(Decision::For(Duration::from_secs(3600)));
        assert!(grant.is_open());

        assert!(!grant.record(Decision::Deny));
        assert!(!grant.is_open());
    }

    #[test]
    fn a_grant_can_be_taken_back() {
        let mut grant = Grant::new();
        grant.record(Decision::For(Duration::from_secs(3600)));
        grant.revoke();
        assert!(!grant.is_open());
    }
}
