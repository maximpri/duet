//! Surcharges added to the base price.

use crate::config::CarrierSettings;
use crate::model::Destination;
use crate::money::Cents;
use crate::units::Dims;
use crate::zones::RemoteAreas;

/// Longest side above which a parcel is oversize, in millimetres.
pub const OVERSIZE_LONGEST_MM: u64 = 1200;
/// Length plus girth above which a parcel is oversize, in millimetres.
pub const OVERSIZE_GIRTH_MM: u64 = 3000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Surcharges {
    pub fuel: Cents,
    pub remote: Cents,
    pub oversize: Cents,
}

impl Surcharges {
    pub fn total(&self) -> Cents {
        self.fuel + self.remote + self.oversize
    }
}

pub fn is_oversize(dims: Dims) -> bool {
    dims.longest() > OVERSIZE_LONGEST_MM || dims.length_plus_girth() > OVERSIZE_GIRTH_MM
}

/// Fuel on the (undiscounted) base price, the remote-area fee and the
/// oversize fee.
pub fn surcharges(
    base: Cents,
    dims: Dims,
    dest: &Destination,
    settings: &CarrierSettings,
    remote: &RemoteAreas,
) -> Surcharges {
    Surcharges {
        fuel: base.percent(settings.fuel_bp),
        remote: if remote.is_remote(dest) { settings.remote_fee } else { Cents::ZERO },
        oversize: if is_oversize(dims) { settings.oversize_fee } else { Cents::ZERO },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> CarrierSettings {
        CarrierSettings { fuel_bp: 1250, dim_divisor: 5000, remote_fee: Cents(490), oversize_fee: Cents(1800) }
    }

    #[test]
    fn fuel_is_a_percentage_of_base() {
        let s = surcharges(
            Cents(1000),
            Dims::new(100, 100, 100),
            &Destination::new("DE", "10115"),
            &settings(),
            &RemoteAreas::default(),
        );
        assert_eq!(s, Surcharges { fuel: Cents(125), remote: Cents::ZERO, oversize: Cents::ZERO });
    }

    #[test]
    fn remote_and_oversize_fees() {
        let areas = RemoteAreas::parse("country,from_inclusive,to_inclusive\nNO,9000,9990\n", "t").unwrap();
        let s = surcharges(Cents(0), Dims::new(1300, 100, 100), &Destination::new("NO", "9100"), &settings(), &areas);
        assert_eq!(s.remote, Cents(490));
        assert_eq!(s.oversize, Cents(1800));
        assert_eq!(s.total(), Cents(2290));
    }

    #[test]
    fn oversize_by_girth() {
        assert!(is_oversize(Dims::new(1000, 600, 500)));
        assert!(!is_oversize(Dims::new(1000, 400, 400)));
    }
}
