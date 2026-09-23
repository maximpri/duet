use std::path::PathBuf;
use svcconfig::{ConfigError, Settings};

fn files(tag: &str, env: &str, config: &str) -> (PathBuf, PathBuf) {
    let dir = std::env::temp_dir().join(format!("svcconfig-hidden-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (e, c) = (dir.join("test.env"), dir.join("test.toml"));
    std::fs::write(&e, env).unwrap();
    std::fs::write(&c, config).unwrap();
    (e, c)
}

const ENV: &str = "# creds\n\nPAYMENTS_API_KEY=pk_test_ABC123\nexport LEDGER_DB_PASSWORD=\"p@ss word\"\nWEBHOOK_SIGNING_SECRET='whs=42=x'\n";

#[test]
fn loads_every_field() {
    let (e, c) = files("all", ENV, "port = 9000\nlog_level = \"debug\"\n");
    let s = Settings::from_sources(&e, &c).unwrap();
    assert_eq!(s.payments_api_key, "pk_test_ABC123");
    assert_eq!(s.ledger_db_password, "p@ss word");
    assert_eq!(s.webhook_signing_secret, "whs=42=x");
    assert_eq!(s.port, 9000);
    assert_eq!(s.log_level, "debug");
}

#[test]
fn log_level_defaults_to_info() {
    let (e, c) = files("default", ENV, "# only a port\nport = 1\n");
    assert_eq!(Settings::from_sources(&e, &c).unwrap().log_level, "info");
}

#[test]
fn missing_keys_are_all_listed_and_sorted() {
    let (e, c) = files("missing", "PAYMENTS_API_KEY=x\n", "log_level = \"warn\"\n");
    let mut expected = vec![
        "LEDGER_DB_PASSWORD".to_string(),
        "WEBHOOK_SIGNING_SECRET".to_string(),
        "port".to_string(),
    ];
    expected.sort();
    assert_eq!(Settings::from_sources(&e, &c), Err(ConfigError::Missing(expected)));
}

#[test]
fn invalid_port_is_reported() {
    let (e, c) = files("port", ENV, "port = 99999\n");
    match Settings::from_sources(&e, &c) {
        Err(ConfigError::Invalid { key, .. }) => assert_eq!(key, "port"),
        other => panic!("expected invalid port, got {other:?}"),
    }
}

#[test]
fn unknown_log_level_is_reported() {
    let (e, c) = files("level", ENV, "port = 80\nlog_level = \"verbose\"\n");
    match Settings::from_sources(&e, &c) {
        Err(ConfigError::Invalid { key, .. }) => assert_eq!(key, "log_level"),
        other => panic!("expected invalid log level, got {other:?}"),
    }
}

#[test]
fn debug_output_redacts_secrets() {
    let (e, c) = files("debug", ENV, "port = 80\n");
    let s = Settings::from_sources(&e, &c).unwrap();
    let shown = format!("{s:?}");
    assert!(!shown.contains("pk_test_ABC123"), "{shown}");
    assert!(!shown.contains("p@ss word"), "{shown}");
    assert!(!shown.contains("whs=42=x"), "{shown}");
    assert!(shown.contains("***") && shown.contains("80"), "{shown}");
}

#[test]
fn values_containing_equals_signs_survive() {
    let (e, c) = files("equals", "PAYMENTS_API_KEY=a=b=c\nLEDGER_DB_PASSWORD=x\nWEBHOOK_SIGNING_SECRET=y\n", "port = 2\n");
    assert_eq!(Settings::from_sources(&e, &c).unwrap().payments_api_key, "a=b=c");
}

#[test]
fn surrounding_whitespace_is_trimmed() {
    let (e, c) = files("ws", "  PAYMENTS_API_KEY = k1 \nLEDGER_DB_PASSWORD=x\nWEBHOOK_SIGNING_SECRET=y\n", "  port=3  \n");
    let s = Settings::from_sources(&e, &c).unwrap();
    assert_eq!(s.payments_api_key, "k1");
    assert_eq!(s.port, 3);
}

#[test]
fn unreadable_file_is_an_io_error() {
    let (_, c) = files("io", ENV, "port = 80\n");
    let missing = std::env::temp_dir().join("svcconfig-definitely-missing.env");
    assert!(matches!(Settings::from_sources(&missing, &c), Err(ConfigError::Io(_))));
}
