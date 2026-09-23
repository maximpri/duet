use ingest::parse_record;

#[test]
fn parses_a_well_formed_line() {
    let r = parse_record("2026-01-02T03:04:05Z|user=a@b.test|name=Ada Lovelace|ip=198.51.100.7|amount=42").unwrap();
    assert_eq!(r.timestamp, 1_767_323_045);
    assert_eq!(r.initials, "AL");
    assert_eq!(r.amount_cents, 4200);
}
