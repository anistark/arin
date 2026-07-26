//! Anchor descriptors.

use crate::geom::{DisplayId, LogicalRect};
use serde::{Deserialize, Serialize};

/// Where an annotation is pinned.
///
/// Every annotation carries one. In 0.1 only `screen_rect` and `display_id` are read.
/// `content_hash` is reserved and always null: scroll tracking populates it later so
/// annotations can be matched to content after it moves. It is serialized now, even as
/// null, so that filling it in is an additive change rather than a new field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Anchor {
    /// The region in logical points, on `display_id`.
    pub screen_rect: LogicalRect,
    /// The display the rect belongs to.
    pub display_id: DisplayId,
    /// Reserved. Always `None` in 0.1.
    #[serde(default)]
    pub content_hash: Option<String>,
}

impl Anchor {
    /// Anchor to a region of a display.
    pub const fn new(screen_rect: LogicalRect, display_id: DisplayId) -> Self {
        Self {
            screen_rect,
            display_id,
            content_hash: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_field_stays_on_the_wire() {
        let anchor = Anchor::new(LogicalRect::new(100.0, 200.0, 340.0, 90.0), DisplayId(1));
        let json = serde_json::to_value(&anchor).unwrap();
        assert!(
            json.get("content_hash")
                .is_some_and(serde_json::Value::is_null),
            "content_hash must serialize as null, not vanish"
        );
    }

    #[test]
    fn absent_reserved_field_deserializes() {
        let json = r#"{"screen_rect":[100,200,340,90],"display_id":1}"#;
        let anchor: Anchor = serde_json::from_str(json).unwrap();
        assert_eq!(anchor.content_hash, None);
    }
}
