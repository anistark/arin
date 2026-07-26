//! Opaque identifiers.
//!
//! Both are opaque strings on the wire. Clients must not parse or construct them. The
//! daemon is free to change the generation scheme without a protocol bump.

use serde::{Deserialize, Serialize};

macro_rules! opaque_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Wrap an existing identifier.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Borrow the underlying string.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }
    };
}

opaque_id! {
    /// Identifies a client session. Annotations are scoped to one.
    SessionId
}

opaque_id! {
    /// Identifies a single annotation within a session.
    AnnotationId
}
