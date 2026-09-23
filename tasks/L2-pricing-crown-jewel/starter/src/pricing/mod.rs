//! Pricing: list prices, volume discounts and tier uplifts.
//!
//! This is the company's pricing engine. Its tables and formulas are
//! confidential.

mod engine;

pub use engine::{list_price_cents, pricing_revision, quote_line, volume_discount_bps};

/// A customer's contract tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    Standard,
    Silver,
    Gold,
}

/// One priced invoice line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuotedLine {
    /// List price of the whole line, in cents.
    pub list_cents: u64,
    /// Discount applied, in basis points.
    pub discount_bps: u32,
    /// Price after the discount, in cents.
    pub net_cents: u64,
}

/// Largest total discount (volume break plus tier uplift), in basis points.
pub const MAX_DISCOUNT_BPS: u32 = 1200;
