//! Protocol version, carried on every message as `v`.

use serde::{Deserialize, Deserializer, Serialize, Serializer, de::Error as _};
use std::fmt;

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: Version = Version { major: 0, minor: 1 };

/// A `major.minor` protocol version.
///
/// Serialized as a string, for example `"0.1"`. Patch numbers do not exist here: the
/// protocol only changes in ways that matter at the minor level or above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Breaking-change generation. A mismatch is unrecoverable.
    pub major: u16,
    /// Additive revision within a major. Higher minors stay readable by lower ones.
    pub minor: u16,
}

impl Version {
    /// Construct a version.
    pub const fn new(major: u16, minor: u16) -> Self {
        Self { major, minor }
    }

    /// Whether a peer speaking `self` can be understood by an implementation of `other`.
    ///
    /// Majors must match exactly. Minors may differ in either direction, because unknown
    /// fields are ignored and changes within a major are additive.
    pub const fn is_compatible_with(self, other: Version) -> bool {
        self.major == other.major
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.major, self.minor)
    }
}

/// Returned when a `v` field is not a `major.minor` pair.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("malformed protocol version {0:?}, expected \"major.minor\"")]
pub struct VersionParseError(pub String);

impl std::str::FromStr for Version {
    type Err = VersionParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let err = || VersionParseError(s.to_owned());
        let (major, minor) = s.split_once('.').ok_or_else(err)?;
        Ok(Self {
            major: major.parse().map_err(|_| err())?,
            minor: minor.parse().map_err(|_| err())?,
        })
    }
}

impl Serialize for Version {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Version {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        raw.parse().map_err(D::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_as_a_string() {
        let json = serde_json::to_string(&PROTOCOL_VERSION).unwrap();
        assert_eq!(json, r#""0.1""#);
        assert_eq!(
            serde_json::from_str::<Version>(&json).unwrap(),
            PROTOCOL_VERSION
        );
    }

    #[test]
    fn minor_differences_stay_compatible() {
        assert!(Version::new(0, 7).is_compatible_with(PROTOCOL_VERSION));
        assert!(Version::new(0, 0).is_compatible_with(PROTOCOL_VERSION));
        assert!(!Version::new(1, 0).is_compatible_with(PROTOCOL_VERSION));
    }

    #[test]
    fn rejects_malformed_versions() {
        for bad in ["", "1", "1.", ".1", "x.y", "0.1.2"] {
            assert!(bad.parse::<Version>().is_err(), "{bad:?} should not parse");
        }
    }
}
