//! Positions named rather than measured.
//!
//! A client that has not taken a screenshot has no idea how big the display is, so it
//! cannot name a coordinate. It can still say where it means. `"top-left"` and
//! `"50%,30%"` both resolve against whatever display they are sent to, which is work only
//! the daemon can do, since it is the one that knows the geometry.
//!
//! # Deliberately approximate
//!
//! A named anchor is a region of the screen, not a target in it. `"top-left"` resolves to
//! a tenth of the way in from each edge rather than to the very corner, because a mark at
//! the literal origin sits half off the display and points at nothing. Anything needing
//! precision sends coordinates, or from 0.3 a query.

use crate::geom::LogicalPoint;
use crate::validate::ValidationError;

/// How far in from an edge a named anchor sits, as a fraction of the display.
///
/// The corner itself is the wrong answer twice over: an orb centred there is clipped by
/// the edge, and no interface puts anything at the exact origin. A tenth reads as "over
/// in that corner" at every display size, which a fixed number of points would not.
const INSET: f64 = 0.10;

/// A position expressed relative to a display rather than in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Position {
    /// Horizontal position as a fraction of the display's width, `0.0..=1.0`.
    pub x: f64,
    /// Vertical position as a fraction of the display's height, `0.0..=1.0`.
    pub y: f64,
}

impl Position {
    /// Build from fractions of the display.
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }

    /// Parse `"top-left"`, `"center"`, `"50%,30%"` and the rest.
    ///
    /// Case and surrounding space are ignored, and both `center` and `centre` are
    /// accepted, since a client should not have to guess which spelling the daemon was
    /// written in.
    pub fn parse(raw: &str) -> Result<Self, ValidationError> {
        let text = raw.trim().to_ascii_lowercase();
        if text.is_empty() {
            return Err(ValidationError::Empty { field: "at" });
        }

        let (near, mid, far) = (INSET, 0.5, 1.0 - INSET);
        let named = match text.replace('_', "-").as_str() {
            "top-left" => Some((near, near)),
            "top" | "top-center" | "top-centre" => Some((mid, near)),
            "top-right" => Some((far, near)),
            "left" | "center-left" | "centre-left" => Some((near, mid)),
            "center" | "centre" | "middle" => Some((mid, mid)),
            "right" | "center-right" | "centre-right" => Some((far, mid)),
            "bottom-left" => Some((near, far)),
            "bottom" | "bottom-center" | "bottom-centre" => Some((mid, far)),
            "bottom-right" => Some((far, far)),
            _ => None,
        };
        if let Some((x, y)) = named {
            return Ok(Self::new(x, y));
        }

        Self::parse_percentages(&text, raw)
    }

    /// Parse the `"50%,30%"` form.
    fn parse_percentages(text: &str, raw: &str) -> Result<Self, ValidationError> {
        let (left, right) = text
            .split_once(',')
            .ok_or(ValidationError::UnknownPosition {
                got: raw.to_owned(),
            })?;

        // Both sides must carry the sign. Accepting a bare number here would make
        // `"50,30"` mean something different from the `x` and `y` fields, which is a trap
        // worth refusing outright.
        let percentage = |part: &str| -> Result<f64, ValidationError> {
            let value = part
                .trim()
                .strip_suffix('%')
                .ok_or(ValidationError::UnknownPosition {
                    got: raw.to_owned(),
                })?;
            let value: f64 =
                value
                    .trim()
                    .parse()
                    .map_err(|_| ValidationError::UnknownPosition {
                        got: raw.to_owned(),
                    })?;
            if !value.is_finite() || !(0.0..=100.0).contains(&value) {
                return Err(ValidationError::PositionOutOfRange {
                    got: part.trim().to_owned(),
                });
            }
            Ok(value / 100.0)
        };

        Ok(Self::new(percentage(left)?, percentage(right)?))
    }

    /// Turn a fraction of a display into a point on it.
    pub fn resolve(self, logical_size: [f64; 2]) -> LogicalPoint {
        LogicalPoint::new(self.x * logical_size[0], self.y * logical_size[1])
    }

    /// Every name that parses, for documentation and for tests.
    pub const NAMES: &'static [&'static str] = &[
        "top-left",
        "top",
        "top-right",
        "left",
        "center",
        "right",
        "bottom-left",
        "bottom",
        "bottom-right",
    ];
}

#[cfg(test)]
mod tests {
    use super::*;

    const DISPLAY: [f64; 2] = [1512.0, 982.0];

    #[test]
    fn every_documented_name_parses() {
        for name in Position::NAMES {
            assert!(Position::parse(name).is_ok(), "{name} should parse");
        }
    }

    #[test]
    fn the_corners_are_inset_rather_than_at_the_origin() {
        // A mark at the literal corner is half off the screen.
        let at = Position::parse("top-left").unwrap().resolve(DISPLAY);
        assert!(at.x > 0.0 && at.y > 0.0, "{at:?} is in the corner itself");
        assert!(at.x < DISPLAY[0] / 2.0 && at.y < DISPLAY[1] / 2.0);

        let at = Position::parse("bottom-right").unwrap().resolve(DISPLAY);
        assert!(
            at.x < DISPLAY[0] && at.y < DISPLAY[1],
            "{at:?} is off screen"
        );
        assert!(at.x > DISPLAY[0] / 2.0 && at.y > DISPLAY[1] / 2.0);
    }

    #[test]
    fn center_is_the_middle_of_the_display() {
        let at = Position::parse("center").unwrap().resolve(DISPLAY);
        assert_eq!(at.x, DISPLAY[0] / 2.0);
        assert_eq!(at.y, DISPLAY[1] / 2.0);
    }

    #[test]
    fn both_spellings_of_centre_agree() {
        assert_eq!(Position::parse("center"), Position::parse("centre"));
        assert_eq!(Position::parse("top"), Position::parse("top-centre"));
    }

    #[test]
    fn case_space_and_underscores_are_forgiven() {
        let expected = Position::parse("top-left").unwrap();
        for spelling in ["  TOP-LEFT ", "Top-Left", "top_left"] {
            assert_eq!(Position::parse(spelling).unwrap(), expected, "{spelling:?}");
        }
    }

    #[test]
    fn percentages_resolve_against_the_display() {
        let at = Position::parse("50%,30%").unwrap().resolve(DISPLAY);
        assert_eq!(at.x, DISPLAY[0] * 0.5);
        assert!((at.y - DISPLAY[1] * 0.3).abs() < 1e-9);
    }

    #[test]
    fn percentages_tolerate_spacing_and_decimals() {
        let at = Position::parse(" 12.5% , 7.5% ").unwrap();
        assert!((at.x - 0.125).abs() < 1e-9);
        assert!((at.y - 0.075).abs() < 1e-9);
    }

    #[test]
    fn the_edges_of_the_range_are_allowed() {
        assert_eq!(Position::parse("0%,0%").unwrap(), Position::new(0.0, 0.0));
        assert_eq!(
            Position::parse("100%,100%").unwrap(),
            Position::new(1.0, 1.0)
        );
    }

    /// `"50,30"` must not quietly mean the same as `"50%,30%"`, since it looks exactly
    /// like the coordinates the `x` and `y` fields take and would be off by a factor of
    /// the display size.
    #[test]
    fn a_bare_number_pair_is_refused() {
        assert!(matches!(
            Position::parse("50,30"),
            Err(ValidationError::UnknownPosition { .. })
        ));
        assert!(matches!(
            Position::parse("50%,30"),
            Err(ValidationError::UnknownPosition { .. })
        ));
    }

    #[test]
    fn nonsense_is_refused_with_what_was_sent() {
        let Err(ValidationError::UnknownPosition { got }) = Position::parse("north-by-northwest")
        else {
            panic!("expected an unknown position");
        };
        assert_eq!(got, "north-by-northwest");
    }

    #[test]
    fn a_percentage_off_the_display_is_refused() {
        assert!(matches!(
            Position::parse("150%,30%"),
            Err(ValidationError::PositionOutOfRange { .. })
        ));
        assert!(matches!(
            Position::parse("-10%,30%"),
            Err(ValidationError::PositionOutOfRange { .. })
        ));
    }

    #[test]
    fn an_empty_position_is_refused() {
        assert!(matches!(
            Position::parse("   "),
            Err(ValidationError::Empty { .. })
        ));
    }

    #[test]
    fn every_name_lands_on_the_display() {
        for name in Position::NAMES {
            let at = Position::parse(name).unwrap().resolve(DISPLAY);
            assert!(
                (0.0..=DISPLAY[0]).contains(&at.x) && (0.0..=DISPLAY[1]).contains(&at.y),
                "{name} resolved off the display to {at:?}"
            );
        }
    }
}
