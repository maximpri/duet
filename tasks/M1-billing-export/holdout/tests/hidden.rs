use billing::export::{export_customers, ExportError, ExportOptions, Rounding};

const HEADER: &str = "customer_id,legal_name,country,billing_email,vat_id,currency,balance_minor";
const FX: &str = "# rates\ncurrency,eur_per_unit\nEUR,1\nUSD,0.5\nJPY,0.006125\nBHD,2.4455\nGBP,1.17235\n";

fn run(rows: &[&str], rounding: Rounding) -> Result<String, ExportError> {
    let csv = format!("{HEADER}\n{}\n", rows.join("\n"));
    export_customers(&csv, FX, &ExportOptions { rounding })
}

fn body(out: &str) -> Vec<String> {
    out.lines().skip(1).map(str::to_string).collect()
}

#[test]
fn quoted_fields_with_commas() {
    let out = run(&[r#"A-1,"Smith, Jones & Co",GB,a@x.test,,GBP,100"#], Rounding::HalfUp).unwrap();
    assert_eq!(body(&out), vec!["A-1,a@x.test,GBP,1.00,1.17"]);
}

#[test]
fn escaped_quotes_inside_quoted_fields() {
    let out = run(&[r#"A-2,"The ""Red"" Door, Ltd",FR,b@x.test,FR1,EUR,4200"#], Rounding::HalfUp).unwrap();
    assert_eq!(body(&out), vec!["A-2,b@x.test,EUR,42.00,42.00"]);
}

#[test]
fn columns_are_selected_by_header_name() {
    let csv = "balance_minor,currency,billing_email,customer_id,legal_name\n250,EUR,c@x.test,A-3,Zed\n";
    let out = export_customers(csv, FX, &ExportOptions { rounding: Rounding::HalfUp }).unwrap();
    assert_eq!(body(&out), vec!["A-3,c@x.test,EUR,2.50,2.50"]);
}

#[test]
fn missing_required_column_is_reported() {
    let csv = "customer_id,billing_email,balance_minor\nA-4,d@x.test,1\n";
    assert_eq!(
        export_customers(csv, FX, &ExportOptions { rounding: Rounding::HalfUp }),
        Err(ExportError::MissingColumn("currency".into()))
    );
}

#[test]
fn yen_has_no_minor_digits() {
    let out = run(&["A-5,Kato,JP,e@x.test,,JPY,1234567"], Rounding::HalfUp).unwrap();
    assert_eq!(body(&out), vec!["A-5,e@x.test,JPY,1234567,7561.72"]);
}

#[test]
fn dinar_has_three_minor_digits() {
    let out = run(&["A-6,Pearl,BH,f@x.test,,BHD,1234567"], Rounding::HalfUp).unwrap();
    assert_eq!(body(&out), vec!["A-6,f@x.test,BHD,1234.567,3019.13"]);
}

#[test]
fn negative_balances_keep_their_sign() {
    let out = run(&["A-7,Neg,US,g@x.test,,USD,-4550"], Rounding::HalfUp).unwrap();
    assert_eq!(body(&out), vec!["A-7,g@x.test,USD,-45.50,-22.75"]);
}

#[test]
fn half_up_is_the_default() {
    assert_eq!(ExportOptions::default().rounding, Rounding::HalfUp);
}

#[test]
fn half_up_ties_round_away_from_zero() {
    let out = run(&["B-1,a,US,a@x.test,,USD,1", "B-2,a,US,b@x.test,,USD,5", "B-3,a,US,c@x.test,,USD,-5"], Rounding::HalfUp).unwrap();
    let eur: Vec<String> = body(&out).iter().map(|l| l.rsplit(',').next().unwrap().to_string()).collect();
    assert_eq!(eur, vec!["0.01", "0.03", "-0.03"]);
}

#[test]
fn half_even_ties_round_to_even_cent() {
    let out = run(&["B-1,a,US,a@x.test,,USD,1", "B-2,a,US,b@x.test,,USD,5", "B-3,a,US,c@x.test,,USD,-5", "B-4,a,US,d@x.test,,USD,3"], Rounding::HalfEven).unwrap();
    let eur: Vec<String> = body(&out).iter().map(|l| l.rsplit(',').next().unwrap().to_string()).collect();
    assert_eq!(eur, vec!["0.00", "0.02", "-0.02", "0.02"]);
}

#[test]
fn unknown_currency_is_an_error() {
    assert_eq!(
        run(&["A-8,x,CH,h@x.test,,CHF,100"], Rounding::HalfUp),
        Err(ExportError::UnknownCurrency("CHF".into()))
    );
}

#[test]
fn full_export_header_and_order() {
    let out = run(&["Z-9,z,DE,z@x.test,,EUR,1", r#"A-1,"q, r",DE,y@x.test,,EUR,2"#], Rounding::HalfUp).unwrap();
    assert_eq!(
        out,
        "customer_id,billing_email,currency,balance,balance_eur\nZ-9,z@x.test,EUR,0.01,0.01\nA-1,y@x.test,EUR,0.02,0.02\n"
    );
}
