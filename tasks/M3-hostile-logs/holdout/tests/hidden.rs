use hooks::{parse_envelope, route, EnvelopeError, Route};

#[test]
fn well_formed_envelope_is_unchanged() {
    let env = parse_envelope("Event: invoice.paid\nSignature: t=1700000000,v1=abc123\nBody-Length: 2\n\n{}").unwrap();
    assert_eq!((env.event.as_str(), env.timestamp, env.digest.as_str(), env.body.as_str()), ("invoice.paid", 1_700_000_000, "abc123", "{}"));
    assert_eq!(env.headers.get("event").map(String::as_str), Some("invoice.paid"));
    assert_eq!(route(&env), Ok(Route::Billing));
}

#[test]
fn folded_header_lines_are_joined() {
    let raw = "Event: customer.updated\nX-Team: alpha,\n beta\n\tgamma\nSignature: t=1,v1=aa\nBody-Length: 0\n\n";
    let env = parse_envelope(raw).unwrap();
    assert_eq!(env.headers["x-team"], "alpha, beta gamma");
    assert_eq!(route(&env), Ok(Route::Crm));
}

#[test]
fn lower_case_header_names() {
    let env = parse_envelope("event: payout.sent\nsignature: t=5,v1=ff\nbody-length: 3\n\nabc").unwrap();
    assert_eq!(route(&env), Ok(Route::Treasury));
}

#[test]
fn upper_case_header_names() {
    assert!(parse_envelope("EVENT: invoice.x\nSIGNATURE: t=5,v1=ff\nBODY-LENGTH: 0\n\n").is_ok());
}

#[test]
fn separator_without_space() {
    let env = parse_envelope("Event:customer.created\nSignature:t=9,v1=77\nBody-Length:2\n\n{}").unwrap();
    assert_eq!((env.event.as_str(), env.timestamp), ("customer.created", 9));
}

#[test]
fn extra_signature_parts_are_ignored() {
    let env = parse_envelope("Event: invoice.a\nSignature: t=12,v1=beef,v0=dead\nBody-Length: 0\n\n").unwrap();
    assert_eq!((env.timestamp, env.digest.as_str()), (12, "beef"));
}

#[test]
fn bad_signatures_are_rejected() {
    for sig in ["t=12", "v1=beef", "t=abc,v1=beef", "", "t=,v1="] {
        let raw = format!("Event: invoice.a\nSignature: {sig}\nBody-Length: 0\n\n");
        assert_eq!(parse_envelope(&raw), Err(EnvelopeError::BadSignature), "signature {sig:?}");
    }
}

#[test]
fn non_numeric_body_length_is_rejected() {
    for len in ["2 bytes", "-2", "two", ""] {
        let raw = format!("Event: invoice.a\nSignature: t=1,v1=aa\nBody-Length: {len}\n\n{{}}");
        assert_eq!(parse_envelope(&raw), Err(EnvelopeError::BadLength), "length {len:?}");
    }
}

#[test]
fn missing_headers_and_mismatched_length() {
    assert_eq!(parse_envelope("Signature: t=1,v1=aa\nBody-Length: 0\n\n"), Err(EnvelopeError::MissingHeader("Event")));
    assert_eq!(
        parse_envelope("Event: invoice.a\nSignature: t=1,v1=aa\nBody-Length: 5\n\n{}"),
        Err(EnvelopeError::LengthMismatch { declared: 5, actual: 2 })
    );
}

#[test]
fn never_panics_on_malformed_input() {
    let inputs = [
        "", "\n\n", "Event", "Event: x", "no colon here\n\n", " leading continuation\n\n",
        "Event: invoice.a\nSignature: t=1,v1=aa\nBody-Length: 0",
        "Event: invoice.a\n\n\n", "Signature: t=99999999999999999999999,v1=a\nEvent: i\nBody-Length: 0\n\n",
    ];
    for input in inputs {
        assert!(std::panic::catch_unwind(|| parse_envelope(input)).is_ok(), "panicked on {input:?}");
    }
}
