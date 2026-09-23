use parcelflow::config::CarrierConfig;
use std::path::Path;

#[test]
fn volumetric_divisor_is_the_dimensional_divisor() {
    let c = CarrierConfig::parse("[default]\nvolumetric_divisor = 5000\n\n[NRP]\nvolumetric_divisor = 6000\n", "t")
        .unwrap();
    assert_eq!(c.get("NRP").dim_divisor, 6000);
    assert_eq!(c.get("ALP").dim_divisor, 5000);
}

#[test]
fn a_default_volumetric_divisor_is_inherited() {
    let c = CarrierConfig::parse("[default]\nvolumetric_divisor = 4000\n\n[ALP]\nfuel_pct = 1\n", "t").unwrap();
    assert_eq!(c.get("ALP").dim_divisor, 4000);
    assert_eq!(c.get("ZZZ").dim_divisor, 4000);
}

#[test]
fn older_files_still_load() {
    let c = CarrierConfig::parse("[default]\ndim_divisor = 5000\n\n[TRV]\ndim_divisor = 4000\n", "t").unwrap();
    assert_eq!(c.get("TRV").dim_divisor, 4000);
}

#[test]
fn the_production_config_has_the_contract_divisors() {
    let c = CarrierConfig::load(Path::new("config/carriers-2026.conf")).unwrap();
    assert_eq!(c.get("NRP").dim_divisor, 6000);
    assert_eq!(c.get("ELB").dim_divisor, 6000);
    assert_eq!(c.get("TRV").dim_divisor, 4000);
    assert_eq!(c.get("MRD").dim_divisor, 3000);
    assert_eq!(c.get("KSX").dim_divisor, 5000);
}
