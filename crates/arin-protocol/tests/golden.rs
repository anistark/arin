//! Golden wire-format tests.
//!
//! Every case here is lifted from the protocol spec. They run headless with no display,
//! which is the point: the contract is testable without a screen.
//!
//! If a change breaks one of these, it is a protocol change. Bump the version and update
//! the spec in the same commit, or find another way.

use arin_protocol::*;

/// Parse a spec example, re-serialize it, and assert nothing was lost or invented.
#[track_caller]
fn round_trip<T>(json: &str) -> T
where
    T: serde::de::DeserializeOwned + serde::Serialize,
{
    let parsed: T = serde_json::from_str(json).unwrap_or_else(|e| panic!("parsing {json}: {e}"));
    let reserialized = serde_json::to_value(&parsed).unwrap();
    let original: serde_json::Value = serde_json::from_str(json).unwrap();
    assert_eq!(
        reserialized, original,
        "round trip changed the message\n  in:  {original}\n  out: {reserialized}"
    );
    parsed
}

mod client_to_daemon {
    use super::*;

    #[test]
    fn session_start() {
        let msg: Envelope<ClientMessage> =
            round_trip(r#"{"v":"0.1","type":"session_start","client_name":"claude-code"}"#);
        assert_eq!(msg.version, PROTOCOL_VERSION);
        let ClientMessage::SessionStart(start) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(start.client_name, "claude-code");
        assert!(msg.body.validate().is_ok());
    }

    #[test]
    fn point_with_coordinates() {
        let msg: Envelope<ClientMessage> = round_trip(
            r#"{"v":"0.1","type":"point","x":412.0,"y":88.0,"display_id":1,"label":"Save"}"#,
        );
        let ClientMessage::Point(point) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(point.display_id, DisplayId(1));
        assert_eq!(point.label.as_deref(), Some("Save"));
        assert_eq!(
            point.target().unwrap(),
            PointTarget::Coords(LogicalPoint::new(412.0, 88.0))
        );
    }

    #[test]
    fn point_with_query() {
        let msg: Envelope<ClientMessage> =
            round_trip(r#"{"v":"0.1","type":"point","query":"the Submit button","display_id":1}"#);
        let ClientMessage::Point(point) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(
            point.target().unwrap(),
            PointTarget::Query("the Submit button")
        );
    }

    #[test]
    fn highlight_with_rect() {
        let msg: Envelope<ClientMessage> = round_trip(
            r#"{"v":"0.1","type":"highlight","rect":[100.0,200.0,340.0,90.0],"display_id":1,"label":"the counterargument"}"#,
        );
        let ClientMessage::Highlight(highlight) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(
            highlight.target().unwrap(),
            HighlightTarget::Rect(LogicalRect::new(100.0, 200.0, 340.0, 90.0))
        );
    }

    #[test]
    fn highlight_with_query() {
        let msg: Envelope<ClientMessage> = round_trip(
            r#"{"v":"0.1","type":"highlight","query":"the error message","display_id":1}"#,
        );
        let ClientMessage::Highlight(highlight) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(
            highlight.target().unwrap(),
            HighlightTarget::Query("the error message")
        );
    }

    #[test]
    fn textbox() {
        let msg: Envelope<ClientMessage> = round_trip(
            r#"{"v":"0.1","type":"textbox","anchor":{"screen_rect":[100.0,200.0,340.0,90.0],"display_id":1,"content_hash":null},"text":"This paragraph is the counterargument."}"#,
        );
        let ClientMessage::Textbox(textbox) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        let anchor = textbox.resolved_anchor().unwrap();
        assert_eq!(anchor.display_id, DisplayId(1));
        assert_eq!(anchor.content_hash, None);
    }

    #[test]
    fn draw() {
        let msg: Envelope<ClientMessage> = round_trip(
            r#"{"v":"0.1","type":"draw","display_id":1,"path":[[100.0,200.0],[140.0,210.0],[180.0,190.0]],"style":{"width":3.0}}"#,
        );
        let ClientMessage::Draw(draw) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(draw.points().count(), 3);
        assert_eq!(draw.style.as_ref().unwrap().width, Some(3.0));
        assert!(msg.body.validate().is_ok());
    }

    #[test]
    fn clear_one_and_clear_all() {
        let one: Envelope<ClientMessage> =
            round_trip(r#"{"v":"0.1","type":"clear","annotation_id":"a_7f3"}"#);
        assert!(one.body.validate().is_ok());

        let all: Envelope<ClientMessage> = round_trip(r#"{"v":"0.1","type":"clear","all":true}"#);
        assert!(all.body.validate().is_ok());
    }

    #[test]
    fn session_end() {
        let msg: Envelope<ClientMessage> = round_trip(r#"{"v":"0.1","type":"session_end"}"#);
        assert_eq!(msg.body, ClientMessage::SessionEnd);
    }
}

mod daemon_to_client {
    use super::*;

    #[test]
    fn ack_for_a_resolved_query() {
        let msg: Envelope<DaemonMessage> = round_trip(
            r#"{"v":"0.1","type":"ack","annotation_id":"a_7f3","resolved_coords":{"x":412.0,"y":88.0},"confidence":0.94,"display":{"id":1,"scale":2.0,"logical_size":[1728.0,1117.0]}}"#,
        );
        let DaemonMessage::Ack(ack) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(ack.confidence, Some(0.94));
        assert_eq!(ack.display.unwrap().scale, 2.0);
    }

    #[test]
    fn invalidated() {
        let msg: Envelope<DaemonMessage> = round_trip(
            r#"{"v":"0.1","type":"invalidated","annotation_id":"a_7f3","reason":"scroll"}"#,
        );
        let DaemonMessage::Invalidated(inv) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(inv.reason, InvalidationReason::Scroll);
    }

    #[test]
    fn error() {
        let msg: Envelope<DaemonMessage> = round_trip(
            r#"{"v":"0.1","type":"error","code":"no_resolver","msg":"query form requires a configured resolver"}"#,
        );
        let DaemonMessage::Error(err) = &msg.body else {
            panic!("wrong variant: {:?}", msg.body);
        };
        assert_eq!(err.code, ErrorCode::NoResolver);
    }
}

mod compatibility {
    use super::*;

    #[test]
    fn unknown_fields_are_ignored() {
        let msg: Envelope<ClientMessage> = serde_json::from_str(
            r#"{"v":"0.9","type":"point","x":1,"y":2,"display_id":1,"invented_in_0_9":true}"#,
        )
        .expect("a field from a future minor must not break parsing");
        assert!(msg.version.is_compatible_with(PROTOCOL_VERSION));
    }

    #[test]
    fn a_future_major_is_incompatible() {
        let msg: Envelope<ClientMessage> =
            serde_json::from_str(r#"{"v":"1.0","type":"session_end"}"#).unwrap();
        assert!(!msg.version.is_compatible_with(PROTOCOL_VERSION));
    }

    /// A client built before ttl existed sends no such field, and its messages have to
    /// keep parsing. The reverse matters too: a message with no ttl must not start
    /// serialising one, or every pinned example above changes shape.
    #[test]
    fn a_ttl_is_optional_in_both_directions() {
        let parsed: Envelope<ClientMessage> =
            serde_json::from_str(r#"{"v":"0.1","type":"point","x":1,"y":2,"display_id":1}"#)
                .expect("a point without a ttl must still parse");
        let ClientMessage::Point(point) = parsed.body else {
            panic!("expected a point");
        };
        assert_eq!(point.ttl_ms, None);

        let json = serde_json::to_string(&Point::at(1.0, 2.0, DisplayId(1))).unwrap();
        assert!(!json.contains("ttl_ms"), "got {json}");
    }

    /// A named position must go over the wire as a field, not be resolved client side,
    /// since the client is exactly the party that does not know the display size.
    #[test]
    fn a_named_position_survives_the_wire() {
        let json = serde_json::to_string(&Point::named("50%,30%", DisplayId(1))).unwrap();
        assert!(json.contains(r#""at":"50%,30%""#), "got {json}");

        let parsed: Envelope<ClientMessage> = serde_json::from_str(
            r#"{"v":"0.1","type":"point","at":"bottom-right","display_id":1}"#,
        )
        .expect("a named position must parse");
        let ClientMessage::Point(point) = parsed.body else {
            panic!("expected a point");
        };
        assert_eq!(point.at.as_deref(), Some("bottom-right"));
        assert_eq!(point.x, None);
    }

    #[test]
    fn an_unknown_type_is_an_error_not_a_panic() {
        let parsed: Result<Envelope<ClientMessage>, _> =
            serde_json::from_str(r#"{"v":"0.1","type":"teleport","x":1}"#);
        assert!(parsed.is_err());
    }
}

mod validation {
    use super::*;

    #[test]
    fn point_needs_exactly_one_target_form() {
        let neither = Point {
            x: None,
            y: None,
            at: None,
            query: None,
            display_id: DisplayId(1),
            label: None,
            ttl_ms: None,
        };
        assert!(matches!(
            neither.validate(),
            Err(ValidationError::MissingTarget { .. })
        ));

        let both = Point {
            x: Some(1.0),
            y: Some(2.0),
            at: None,
            query: Some("the Save button".into()),
            display_id: DisplayId(1),
            label: None,
            ttl_ms: None,
        };
        assert!(matches!(
            both.validate(),
            Err(ValidationError::AmbiguousTarget { .. })
        ));

        let half = Point {
            x: Some(1.0),
            y: None,
            at: None,
            query: None,
            display_id: DisplayId(1),
            label: None,
            ttl_ms: None,
        };
        assert!(matches!(
            half.validate(),
            Err(ValidationError::AmbiguousTarget { .. })
        ));
    }

    #[test]
    fn degenerate_rects_are_rejected() {
        let flat = Highlight::over(LogicalRect::new(10.0, 10.0, 0.0, 50.0), DisplayId(1));
        assert!(matches!(
            flat.validate(),
            Err(ValidationError::InvalidRect { .. })
        ));
    }

    #[test]
    fn a_path_needs_two_points() {
        let stub = Draw::new(DisplayId(1), vec![[10.0, 10.0]]);
        assert!(matches!(
            stub.validate(),
            Err(ValidationError::PathTooShort { got: 1 })
        ));
    }

    #[test]
    fn non_finite_coordinates_are_rejected() {
        let nan = Point::at(f64::NAN, 10.0, DisplayId(1));
        assert!(matches!(
            nan.validate(),
            Err(ValidationError::NonFiniteCoordinate { .. })
        ));
    }

    #[test]
    fn clear_needs_a_scope() {
        assert!(matches!(
            Clear::default().validate(),
            Err(ValidationError::MissingTarget { .. })
        ));
        assert!(Clear::all().validate().is_ok());
        assert!(Clear::one(AnnotationId::new("a_7f3")).validate().is_ok());
    }

    #[test]
    fn every_validation_failure_is_a_schema_error() {
        let err = Clear::default().validate().unwrap_err();
        assert_eq!(err.code(), ErrorCode::BadSchema);
    }

    /// A zero here is a unit mistake far more often than an intent, so it is refused
    /// rather than drawn and swept away in the same breath.
    #[test]
    fn a_zero_ttl_is_refused_on_every_drawing_message() {
        assert!(matches!(
            Point::at(1.0, 2.0, DisplayId(1))
                .with_ttl_ms(Some(0))
                .validate(),
            Err(ValidationError::ZeroTtl)
        ));
        assert!(matches!(
            Highlight::over(LogicalRect::new(0.0, 0.0, 10.0, 10.0), DisplayId(1))
                .with_ttl_ms(Some(0))
                .validate(),
            Err(ValidationError::ZeroTtl)
        ));
        assert!(matches!(
            Textbox::new(
                Anchor::new(LogicalRect::new(0.0, 0.0, 10.0, 10.0), DisplayId(1)),
                "text"
            )
            .with_ttl_ms(Some(0))
            .validate(),
            Err(ValidationError::ZeroTtl)
        ));
        assert!(matches!(
            Draw::new(DisplayId(1), vec![[0.0, 0.0], [1.0, 1.0]])
                .with_ttl_ms(Some(0))
                .validate(),
            Err(ValidationError::ZeroTtl)
        ));
    }

    /// The third target form, alongside coordinates and a query.
    #[test]
    fn a_named_position_is_a_target_in_its_own_right() {
        let point = Point::named("top-left", DisplayId(1));
        assert!(point.validate().is_ok());
        let Ok(PointTarget::Named(position)) = point.target() else {
            panic!("expected a named target");
        };
        // Resolved against a display, which is the daemon's job.
        let at = position.resolve([1000.0, 500.0]);
        assert!(at.x > 0.0 && at.x < 500.0 && at.y > 0.0 && at.y < 250.0);
    }

    #[test]
    fn a_position_and_coordinates_together_are_refused() {
        let mut point = Point::at(1.0, 2.0, DisplayId(1));
        point.at = Some("top-left".into());
        assert!(matches!(
            point.validate(),
            Err(ValidationError::AmbiguousTarget { .. })
        ));
    }

    #[test]
    fn a_position_and_a_query_together_are_refused() {
        let mut point = Point::named("top-left", DisplayId(1));
        point.query = Some("the Save button".into());
        assert!(matches!(
            point.validate(),
            Err(ValidationError::AmbiguousTarget { .. })
        ));
    }

    #[test]
    fn an_unknown_position_is_a_schema_error_naming_what_was_sent() {
        let point = Point::named("north-by-northwest", DisplayId(1));
        let err = point.validate().unwrap_err();
        assert_eq!(err.code(), ErrorCode::BadSchema);
        assert!(err.to_string().contains("north-by-northwest"), "{err}");
    }

    #[test]
    fn a_positive_ttl_is_accepted() {
        assert!(
            Point::at(1.0, 2.0, DisplayId(1))
                .with_ttl_ms(Some(1))
                .validate()
                .is_ok()
        );
    }
}
