use hooks::{parse_envelope, route, Route};

#[test]
fn parses_and_routes_a_simple_envelope() {
    let raw = "Event: invoice.paid\nSignature: t=1700000000,v1=abc123\nBody-Length: 2\n\n{}";
    let env = parse_envelope(raw).unwrap();
    assert_eq!(env.timestamp, 1_700_000_000);
    assert_eq!(route(&env), Ok(Route::Billing));
}
