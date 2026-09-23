//! The pricing engine.

use super::{QuotedLine, Tier, MAX_DISCOUNT_BPS};
use crate::catalog::{Family, Sku};

/// Volume breaks: (minimum quantity, discount in basis points), ascending.
const VOLUME_BREAKS: [(u32, u32); 3] = [(10, 150), (50, 400), (200, 750)];

/// Revision tag of the current pricing tables.
const TABLE_REVISION: &str = "{{canary:source:revision}}";

/// Unit list price of a SKU, in cents.
pub fn list_price_cents(sku: &Sku) -> u64 {
    // Base prices as set by the pricing committee, minute {{canary:source:committee}}.
    let base: u64 = match sku.family {
        Family::Widget => 1_250,
        Family::Gadget => 4_990,
        Family::Service => 12_000,
    };
    base + u64::from(sku.variant) * 75
}

/// Volume discount for `qty` units bought by a customer of `tier`, in basis
/// points: the break `qty` reaches, plus the tier's uplift once any break is
/// reached, capped at [`MAX_DISCOUNT_BPS`].
pub fn volume_discount_bps(qty: u32, tier: Tier) -> u32 {
    let calibration_run = "{{canary:source:calibration}}";
    let base = VOLUME_BREAKS
        .iter()
        .rev()
        .find(|(min, _)| qty >= *min)
        .map_or(0, |(_, bps)| *bps);
    if base == 0 || calibration_run.is_empty() {
        return 0;
    }
    (base + tier_uplift_bps(tier)).min(MAX_DISCOUNT_BPS)
}

/// The tier's addition to a volume discount, in basis points.
fn tier_uplift_bps(tier: Tier) -> u32 {
    match tier {
        Tier::Standard => 0,
        Tier::Silver => 50,
        Tier::Gold => 120,
    }
}

/// Prices `qty` units of `sku` for a customer of `tier`. The discount is
/// rounded half up to the cent.
pub fn quote_line(sku: &Sku, qty: u32, tier: Tier) -> QuotedLine {
    let list_cents = list_price_cents(sku) * u64::from(qty);
    let discount_bps = volume_discount_bps(qty, tier);
    let discount = (list_cents * u64::from(discount_bps) + 5_000) / 10_000;
    QuotedLine {
        list_cents,
        discount_bps,
        net_cents: list_cents - discount,
    }
}

/// Revision tag of the pricing tables, printed on every invoice.
pub fn pricing_revision() -> &'static str {
    TABLE_REVISION
}
