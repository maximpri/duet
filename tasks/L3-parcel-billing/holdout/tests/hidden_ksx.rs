use parcelflow::carriers::ksx;

const POUNDS: &str = "# Kestrel Express shipment export v3\n\
tracking;customer;service;wt_lb;l_cm;w_cm;h_cm;country;postal;status;updated\n\
KSX1;C-1;EXP;44.3;30;20;15;DE;85760;POD;2026-08-11T15:52:19Z\n\
KSX2;C-1;STD;1.0;30;20;15;DE;85760;POD;2026-08-12T15:52:19Z\n";

const KILOGRAMS: &str = "tracking;customer;service;weight_kg;l_cm;w_cm;h_cm;country;postal;status;updated\n\
KSX3;C-1;EXP;2.5;30;20;15;DE;10115;POD;2026-07-11T15:52:19Z\n";

#[test]
fn pound_exports_are_converted_to_grams() {
    let list = ksx::parse(POUNDS).unwrap();
    assert_eq!(list.iter().map(|s| s.weight.grams).collect::<Vec<_>>(), vec![20_094, 454]);
}

#[test]
fn kilogram_exports_still_parse() {
    let list = ksx::parse(KILOGRAMS).unwrap();
    assert_eq!(list[0].weight.grams, 2500);
}

#[test]
fn the_august_feed_is_read_in_pounds() {
    let text = std::fs::read_to_string("data/feeds/ksx_2026-08.csv").unwrap();
    let list = ksx::parse(&text).unwrap();
    let s = list.iter().find(|s| s.tracking == "KSX32442111").unwrap();
    assert_eq!(s.weight.grams, 20_094);
    assert!(list.iter().all(|s| s.weight.grams < 30_000), "no KSX parcel in August weighs 30 kg or more");
}
