//! Rendering policy.
//!
//! The daemon decides what a resolution looks like, not the client. Clients report
//! intent, and the daemon owns the visual grammar so it stays consistent across every
//! agent driving it.

use serde::{Deserialize, Serialize};

/// Confidence at or above which a resolution is drawn as a precise point.
///
/// Grounding models in this class sit around 85 to 95 percent accuracy. The asymmetry
/// matters: a slightly large highlight reads as intentional, while a confident arrow
/// pointing at the wrong button reads as broken. When in doubt, draw the region.
pub const HIGH_CONFIDENCE: f64 = 0.85;

/// How a resolved target should be drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rendering {
    /// The orb travels and settles on the target.
    Point,
    /// A region is outlined instead, because the exact target is not certain.
    Region,
}

impl Rendering {
    /// Choose a rendering for a confidence value.
    pub fn for_confidence(confidence: f64) -> Self {
        if confidence >= HIGH_CONFIDENCE {
            Self::Point
        } else {
            Self::Region
        }
    }
}

/// What the orb is doing.
///
/// A circle has no facing, so travel reads through stretch along the velocity vector,
/// never through rotation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrbState {
    /// Parked. Slow pulse, sparse embers.
    Idle,
    /// A resolve or stream is in flight. Faster pulse, denser embers, stationary.
    Thinking,
    /// Moving to a target. Stretched along velocity, trail spawns along the arc.
    Traveling,
    /// Settled on a target. Brief bright flare, then the embers calm.
    Pointing,
    /// Winding down. Dims to a faint core, embers stop, fades out.
    Ending,
}

impl Default for OrbState {
    fn default() -> Self {
        Self::Idle
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertain_resolutions_draw_a_region() {
        assert_eq!(Rendering::for_confidence(0.94), Rendering::Point);
        assert_eq!(Rendering::for_confidence(HIGH_CONFIDENCE), Rendering::Point);
        assert_eq!(Rendering::for_confidence(0.60), Rendering::Region);
        assert_eq!(Rendering::for_confidence(0.0), Rendering::Region);
    }
}
