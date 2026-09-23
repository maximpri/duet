//! End-to-end: a small project root billed through the public API.

use parcelflow::app::{self, Settings};
use parcelflow::billing::run::Skip;
use parcelflow::cli;
use parcelflow::money::Cents;
use std::fs;
use std::path::{Path, PathBuf};

fn root(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("parcelflow-visible-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(dir.join("config")).unwrap();
    fs::create_dir_all(dir.join("data/feeds")).unwrap();
    write(
        &dir,
        "config/carriers.conf",
        "[default]\nfuel_pct = 10\ndim_divisor = 5000\nremote_fee = 4.90\noversize_fee = 18\n",
    );
    write(
        &dir,
        "config/rates.csv",
        "service,zone,first_500g,per_500g\nstandard,1,4.95,0.35\nstandard,2,9.40,0.70\nstandard,4,16.90,1.25\nexpress,1,9.90,0.85\n",
    );
    write(&dir, "config/remote_areas.csv", "country,from_inclusive,to_inclusive,note\nNO,9000,9990,north\n");
    write(
        &dir,
        "data/customers.csv",
        "customer_id,company,contact_name,email,phone,discount_pct,account_manager,tier\n\
         C-1,Test One GmbH,Tess One,one@example.test,,0,,A\n\
         C-2,Test Two AG,Tom Two,two@example.test,,,,B\n",
    );
    write(
        &dir,
        "data/feeds/qnt_2026-08.csv",
        "tracking_no,cust_no,product,weight_kg,length_cm,width_cm,height_cm,dest_country,postcode,status_code,last_event\n\
         QNT1,C-1,STD,1.2,30,20,10,DE,10115,50,10.08.2026 12:00\n\
         QNT2,C-2,STD,0.4,20,15,5,AT,1010,50,11.08.2026 09:00\n\
         QNT3,C-1,STD,2.0,30,20,10,DE,10115,21,12.08.2026 09:00\n\
         QNT4,C-9,STD,1.0,30,20,10,DE,10115,50,12.08.2026 09:00\n\
         QNT5,C-1,EXP,31,40,30,20,DE,10115,50,13.08.2026 09:00\n\
         QNT6,C-1,STD,1.0,30,20,10,DE,10115,50,31.07.2026 09:00\n",
    );
    write(
        &dir,
        "data/feeds/jut_2026-08.jsonl",
        concat!(
            r#"{"awb":"JUT1","customer":"C-2","product":"24H","weight_g":800,"length_mm":130,"width_mm":100,"height_mm":100,"country":"DE","postcode":"10115","event":"S6","event_time":"1786363200"}"#,
            "\n",
            r#"{"awb":"JUT2","customer":"C-1","product":"48H","weight_g":450,"length_mm":150,"width_mm":100,"height_mm":100,"country":"NO","postcode":"9100","event":"S6","event_time":"1786449600"}"#,
            "\n"
        ),
    );
    dir
}

fn write(root: &Path, rel: &str, text: &str) {
    fs::write(root.join(rel), text).unwrap();
}

#[test]
fn bills_a_month_end_to_end() {
    let dir = root("e2e");
    let run = app::run_billing(&dir, "2026-08").unwrap();
    let o = &run.outcome;
    assert_eq!(run.feeds.files.len(), 2);
    assert!(run.feeds.errors.is_empty());
    assert_eq!(o.billed_count(), 4);
    assert_eq!(o.not_billable, 2);
    assert_eq!(o.invoices["C-1"].total(), Cents(622 + 2349));
    assert_eq!(o.invoices["C-2"].total(), Cents(1034 + 1183));
    assert_eq!(o.total(), Cents(5188));
    let skipped: Vec<_> = o.skipped.iter().map(|(c, t, _)| format!("{c} {t}")).collect();
    assert_eq!(skipped, vec!["QNT QNT4", "QNT QNT5"]);
    assert!(matches!(&o.skipped[0].2, Skip::UnknownCustomer(c) if c == "C-9"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn invoice_lines_are_in_delivery_order() {
    let dir = root("order");
    let run = app::run_billing(&dir, "2026-08").unwrap();
    let lines: Vec<&str> = run.outcome.invoices["C-2"].lines.iter().map(|l| l.tracking.as_str()).collect();
    assert_eq!(lines, vec!["JUT1", "QNT2"]);
    let text = run.outcome.invoices["C-1"].render();
    assert!(text.starts_with("INVOICE C-1 2026-08\n"));
    assert!(text.trim_end().ends_with("total 29.71"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_broken_feed_does_not_stop_the_run() {
    let dir = root("broken");
    write(&dir, "data/feeds/zzz_2026-08.csv", "whatever\n");
    write(&dir, "data/feeds/hlx_2026-08.csv", "not|a|valid|header\nx|y\n");
    let run = app::run_billing(&dir, "2026-08").unwrap();
    let failed: Vec<&str> = run.feeds.errors.iter().map(|(f, _)| f.as_str()).collect();
    assert_eq!(failed, vec!["hlx_2026-08.csv", "zzz_2026-08.csv"]);
    assert_eq!(run.outcome.billed_count(), 4);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn settings_default_to_the_config_directory() {
    let dir = root("settings");
    let s = Settings::from_root(&dir);
    assert_eq!(s.carrier_config, dir.join("config/carriers.conf"));
    write(&dir, ".env", "CARRIER_CONFIG=config/other.conf\nFEEDS_DIR=/srv/feeds\n");
    let s = Settings::from_root(&dir);
    assert_eq!(s.carrier_config, dir.join("config/other.conf"));
    assert_eq!(s.feeds, PathBuf::from("/srv/feeds"));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_cli_prints_a_summary() {
    let dir = root("cli");
    let args: Vec<String> =
        ["bill", "2026-08", "--root", dir.to_str().unwrap()].iter().map(|s| s.to_string()).collect();
    let out = cli::execute(cli::parse_args(&args).unwrap()).unwrap();
    assert!(out.contains("4 shipments billed, 2 skipped, 2 not billable this month"), "{out}");
    assert!(out.contains("grand total 51.88"), "{out}");
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn quotes_from_the_command_line() {
    let dir = root("quote");
    let args: Vec<String> =
        ["quote", "JUT", "standard", "450g", "15x10x10cm", "NO-9100", "--root", dir.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
    let out = cli::execute(cli::parse_args(&args).unwrap()).unwrap();
    assert!(out.contains("remote 4.90"), "{out}");
    assert!(out.ends_with("total 23.49\n"), "{out}");
    fs::remove_dir_all(dir).unwrap();
}
