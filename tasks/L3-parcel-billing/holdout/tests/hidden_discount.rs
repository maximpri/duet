use parcelflow::billing::price::quote;
use parcelflow::config::CarrierSettings;
use parcelflow::customers::Customer;
use parcelflow::model::{Destination, RawShipment, Service, Status};
use parcelflow::money::Cents;
use parcelflow::tariff::RateCard;
use parcelflow::time::Timestamp;
use parcelflow::units::{Dims, Weight};
use parcelflow::zones::RemoteAreas;

fn shipment(dest: &str, dims: Dims) -> RawShipment {
    RawShipment {
        carrier: "TST",
        tracking: "T1".into(),
        customer: "C-1".into(),
        service: Service::Standard,
        weight: Weight::from_grams(400),
        dims,
        dest: Destination::parse(dest).unwrap(),
        status: Status::Delivered,
        status_at: Timestamp(0),
        signed_by: None,
    }
}

fn customer(discount_bp: i64) -> Customer {
    Customer {
        id: "C-1".into(),
        company: "T".into(),
        contact: "T".into(),
        email: "t@example.test".into(),
        discount_bp,
        account_manager: String::new(),
    }
}

fn price(dest: &str, dims: Dims, discount_bp: i64) -> (Cents, Cents) {
    let settings =
        CarrierSettings { fuel_bp: 1000, dim_divisor: 5000, remote_fee: Cents(500), oversize_fee: Cents(2000) };
    let rates = RateCard::parse("service,zone,first_500g,per_500g\nstandard,1,4.50,0.50\nstandard,4,12.00,1.50\n", "t")
        .unwrap();
    let remote = RemoteAreas::parse("country,from_inclusive,to_inclusive\nNO,9000,9990\n", "t").unwrap();
    let q = quote(&shipment(dest, dims), &customer(discount_bp), &settings, &rates, &remote).unwrap();
    (q.discount, q.total)
}

#[test]
fn the_discount_applies_to_the_base_price_only() {
    // base 12.00, fuel 1.20, remote 5.00; 10 % of the base only.
    assert_eq!(price("NO-9100", Dims::new(100, 100, 100), 1000), (Cents(120), Cents(1200 - 120 + 120 + 500)));
}

#[test]
fn surcharges_are_never_discounted() {
    // base 4.50, fuel 0.45, oversize 20.00; 12.5 % of 4.50 = 0.5625 -> 0.56.
    assert_eq!(price("DE-10115", Dims::new(1210, 20, 20), 1250), (Cents(56), Cents(450 - 56 + 45 + 2000)));
}

#[test]
fn no_discount_leaves_the_price_unchanged() {
    assert_eq!(price("NO-9100", Dims::new(100, 100, 100), 0), (Cents(0), Cents(1200 + 120 + 500)));
}
