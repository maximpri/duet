//! Dimensional ("volumetric") weight and billable weight.

use super::rates::STEP_GRAMS;
use crate::units::{Dims, Weight};

/// Dimensional weight: volume in cm³ divided by the carrier's divisor gives
/// kilograms; with millimetres that is `mm³ / divisor` grams, rounded up.
pub fn dim_weight(dims: Dims, divisor: u64) -> Weight {
    Weight::from_grams(dims.volume_mm3().div_ceil(divisor.max(1)))
}

/// The larger of actual and dimensional weight, rounded up to the next 500 g.
pub fn billable_weight(actual: Weight, dims: Dims, divisor: u64) -> Weight {
    let g = actual.grams.max(dim_weight(dims, divisor).grams);
    Weight::from_grams(g.div_ceil(STEP_GRAMS).max(1) * STEP_GRAMS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dimensional_weight_uses_the_divisor() {
        let d = Dims::new(600, 400, 400);
        assert_eq!(dim_weight(d, 5000).grams, 19_200);
        assert_eq!(dim_weight(d, 6000).grams, 16_000);
        assert_eq!(dim_weight(Dims::new(1, 1, 1), 5000).grams, 1);
    }

    #[test]
    fn billable_weight_rounds_up_to_steps() {
        let small = Dims::new(100, 100, 100);
        assert_eq!(billable_weight(Weight::from_grams(1), small, 5000).grams, 500);
        assert_eq!(billable_weight(Weight::from_grams(1501), small, 5000).grams, 2000);
        assert_eq!(billable_weight(Weight::from_grams(2000), Dims::new(600, 400, 400), 5000).grams, 19_500);
        assert_eq!(billable_weight(Weight::from_grams(0), Dims::default(), 5000).grams, 500);
    }
}
