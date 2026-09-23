use parcelflow::time::parse_iso;

#[test]
fn positive_offsets_are_applied() {
    let ts = parse_iso("2026-09-01T00:40:00+02:00").unwrap();
    assert_eq!(ts, parse_iso("2026-08-31T22:40:00Z").unwrap());
    assert_eq!(ts.to_string(), "2026-08-31T22:40:00Z");
    assert_eq!(ts.month_key(), "2026-08");
}

#[test]
fn negative_offsets_are_applied() {
    let ts = parse_iso("2026-08-31T20:15:00-05:00").unwrap();
    assert_eq!(ts.to_string(), "2026-09-01T01:15:00Z");
    assert_eq!(ts.month_key(), "2026-09");
}

#[test]
fn compact_offsets_and_fractions() {
    assert_eq!(parse_iso("2026-08-01T01:30:00.500+0200").unwrap().to_string(), "2026-07-31T23:30:00Z");
    assert_eq!(parse_iso("2026-08-01 05:30+05:30").unwrap().to_string(), "2026-08-01T00:00:00Z");
}

#[test]
fn utc_forms_are_unchanged() {
    let z = parse_iso("2026-08-14T09:30:05Z").unwrap();
    assert_eq!(parse_iso("2026-08-14T09:30:05+00:00").unwrap(), z);
    assert_eq!(parse_iso("2026-08-14T09:30:05").unwrap(), z);
    assert!(parse_iso("2026-08-14T09:30:05+2").is_err());
}
