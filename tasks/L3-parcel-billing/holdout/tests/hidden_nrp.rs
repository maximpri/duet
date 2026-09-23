use parcelflow::billing::{bill_month, Tariff};
use parcelflow::carriers::nrp;
use parcelflow::config::CarrierConfig;
use parcelflow::customers::Customers;
use parcelflow::tariff::RateCard;

const FEED: &str = r#"{"awb":"NRP1","account":"C-1","service_code":"STD","wgt_kg":1.2,"length_cm":20,"width_cm":15,"height_cm":10,"dest_country":"DE","postal":"10115","status_code":"POD","ts":"2026-09-01T00:40:00+02:00"}
{"awb":"NRP2","account":"C-1","service_code":"STD","wgt_kg":1.2,"length_cm":20,"width_cm":15,"height_cm":10,"dest_country":"DE","postal":"10115","status_code":"POD","ts":"2026-08-01T01:30:00+02:00"}
{"awb":"NRP3","account":"C-1","service_code":"STD","wgt_kg":1.2,"length_cm":20,"width_cm":15,"height_cm":10,"dest_country":"DE","postal":"10115","status_code":"POD","ts":"2026-08-15T12:00:00+02:00"}
"#;

#[test]
fn nordpost_local_times_become_utc() {
    let list = nrp::parse(FEED).unwrap();
    let times: Vec<String> = list.iter().map(|s| s.status_at.to_string()).collect();
    assert_eq!(times, vec!["2026-08-31T22:40:00Z", "2026-07-31T23:30:00Z", "2026-08-15T10:00:00Z"]);
}

#[test]
fn nordpost_deliveries_are_billed_in_their_utc_month() {
    let shipments = nrp::parse(FEED).unwrap();
    let customers = Customers::parse(
        "customer_id,company,contact_name,email,phone,discount_pct,account_manager\nC-1,T,T,t@example.test,,0,\n",
        "t",
    )
    .unwrap();
    let tariff = Tariff {
        carriers: CarrierConfig::parse("[default]\nfuel_pct = 0\n", "t").unwrap(),
        rates: RateCard::parse("service,zone,first_500g,per_500g\nstandard,1,5.00,0.50\n", "t").unwrap(),
        remote: Default::default(),
    };
    let out = bill_month("2026-08", &shipments, &customers, &tariff);
    let billed: Vec<&str> = out.invoices["C-1"].lines.iter().map(|l| l.tracking.as_str()).collect();
    assert_eq!(billed, vec!["NRP3", "NRP1"]);
    assert_eq!(out.not_billable, 1);
}
