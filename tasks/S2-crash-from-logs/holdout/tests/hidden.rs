use ingest::{parse_record, ParseError};

fn line(ts: &str, name: &str, amount: &str) -> String {
    format!("{ts}|user=h@example.test|name={name}|ip=192.0.2.1|amount={amount}")
}

#[test]
fn well_formed_lines_are_unchanged() {
    let r = parse_record(&line("2026-01-02T03:04:05Z", "Grace Hopper", "42")).unwrap();
    assert_eq!((r.timestamp, r.initials.as_str(), r.amount_cents), (1_767_323_045, "GH", 4200));
    assert_eq!(r.email, "h@example.test");
}

#[test]
fn positive_utc_offset_is_converted_to_utc() {
    let a = parse_record(&line("2026-03-10T12:00:00+02:00", "Ab Cd", "1")).unwrap();
    let b = parse_record(&line("2026-03-10T10:00:00Z", "Ab Cd", "1")).unwrap();
    assert_eq!(a.timestamp, b.timestamp);
}

#[test]
fn negative_utc_offset_is_converted_to_utc() {
    let a = parse_record(&line("2026-03-10T12:00:00-05:30", "Ab Cd", "1")).unwrap();
    let b = parse_record(&line("2026-03-10T17:30:00Z", "Ab Cd", "1")).unwrap();
    assert_eq!(a.timestamp, b.timestamp);
}

#[test]
fn non_ascii_first_letter() {
    assert_eq!(parse_record(&line("2026-01-01T00:00:00Z", "Émile Zola", "1")).unwrap().initials, "ÉZ");
}

#[test]
fn non_ascii_last_word() {
    assert_eq!(parse_record(&line("2026-01-01T00:00:00Z", "ana ødegård", "1")).unwrap().initials, "AØ");
}

#[test]
fn single_word_name_has_one_initial() {
    assert_eq!(parse_record(&line("2026-01-01T00:00:00Z", "Prince", "1")).unwrap().initials, "P");
}

#[test]
fn decimal_amounts_are_cents() {
    let cents = |a: &str| parse_record(&line("2026-01-01T00:00:00Z", "A B", a)).unwrap().amount_cents;
    assert_eq!(cents("12.50"), 1250);
    assert_eq!(cents("0.05"), 5);
    assert_eq!(cents("7.5"), 750);
    assert_eq!(cents("3"), 300);
}

#[test]
fn missing_amount_field_is_an_error() {
    let l = "2026-01-01T00:00:00Z|user=h@example.test|name=A B|ip=192.0.2.1";
    assert_eq!(parse_record(l), Err(ParseError::MissingField("amount")));
}

#[test]
fn invalid_amount_and_timestamp_are_errors() {
    assert_eq!(parse_record(&line("2026-01-01T00:00:00Z", "A B", "12.5.0")), Err(ParseError::InvalidAmount));
    assert_eq!(parse_record(&line("2026-01-01T00:00:00Z", "A B", "abc")), Err(ParseError::InvalidAmount));
    assert_eq!(parse_record(&line("yesterday", "A B", "1")), Err(ParseError::InvalidTimestamp));
}

#[test]
fn never_panics_on_malformed_input() {
    let inputs = [
        "", "|", "||||", "abc", "2026-01-01T00:00:00Z", "2026-01-01T00:00:00Z|user=x",
        "2026-01-01T00:00:00Z|user=x|name=|ip=1|amount=1", "2026-01-01T00:00|user=x|name=A|ip=1|amount=1",
        "2026-01-01T00:00:00+0x:00|user=x|name=A B|ip=1|amount=1", "2026-1-1Tx:y:zZ|user=x|name=A B|ip=1|amount=1",
    ];
    for input in inputs {
        let result = std::panic::catch_unwind(|| parse_record(input));
        assert!(result.is_ok(), "parse_record panicked on {input:?}");
    }
}
