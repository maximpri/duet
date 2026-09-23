use billing::money::format_minor;

#[test]
fn formats_cents() {
    assert_eq!(format_minor(123456), "1234.56");
    assert_eq!(format_minor(-5), "-0.05");
}
