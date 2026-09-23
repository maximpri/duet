use warehouse::inventory::pricing::price_with_discount;

#[test]
fn pricing_lives_in_its_own_module() {
    assert_eq!(price_with_discount(100, 9), 900);
    assert_eq!(price_with_discount(100, 10), 950);
    assert_eq!(price_with_discount(100, 50), 4500);
    assert_eq!(price_with_discount(100, 200), 17000);
}
