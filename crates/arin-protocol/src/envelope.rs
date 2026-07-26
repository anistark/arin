//! The `v` field that wraps every message.

use crate::version::Version;
use serde::{Deserialize, Serialize};

/// A message plus the protocol version it was written against.
///
/// The version lives beside the body rather than inside it, so version negotiation can
/// happen before the body is interpreted. On the wire the two are flat:
///
/// ```json
/// {"v":"0.1","type":"session_end"}
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<T> {
    /// The `v` field.
    #[serde(rename = "v")]
    pub version: Version,
    /// The message itself.
    #[serde(flatten)]
    pub body: T,
}

impl<T> Envelope<T> {
    /// Wrap a body at the version this build speaks.
    pub fn current(body: T) -> Self {
        Self {
            version: crate::PROTOCOL_VERSION,
            body,
        }
    }

    /// Wrap a body at an explicit version.
    pub const fn new(version: Version, body: T) -> Self {
        Self { version, body }
    }

    /// Replace the body, keeping the version.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> Envelope<U> {
        Envelope {
            version: self.version,
            body: f(self.body),
        }
    }
}
