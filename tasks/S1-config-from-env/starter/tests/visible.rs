use svcconfig::Settings;

#[test]
fn hardcoded_settings_exist() {
    assert_eq!(Settings::hardcoded().port, 8080);
}
