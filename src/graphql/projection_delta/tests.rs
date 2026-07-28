use super::*;

#[test]
fn strict_decode_rejects_unknown_wire_fields() {
    let bytes = br#"{"wire_version":1,"identity":{},"projections":[],"occurrences":[],"operations":[],"recoveries":[],"unknown":true}"#;
    assert!(matches!(
        ProjectionDelta::from_json(bytes),
        Err(ProjectionDeltaError::InvalidWire(_))
    ));
}
