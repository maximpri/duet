//! Warehouse inventory: stock, pricing and reporting.

use std::collections::BTreeMap;
use std::fmt;

pub mod pricing;
pub mod report;
pub mod stock;

pub use pricing::price_with_discount;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub sku: String,
    pub name: String,
    pub category: String,
    pub quantity: u32,
    pub unit_price_cents: u64,
}

#[derive(Debug, Default)]
pub struct Inventory {
    items: BTreeMap<String, Item>,
}

impl Inventory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, sku: &str) -> Option<&Item> {
        self.items.get(sku)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InventoryError {
    DuplicateSku(String),
    UnknownSku(String),
    InsufficientStock { sku: String, requested: u32, available: u32 },
    InvalidQuantity,
}

impl fmt::Display for InventoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DuplicateSku(s) => write!(f, "duplicate sku {s}"),
            Self::UnknownSku(s) => write!(f, "unknown sku {s}"),
            Self::InsufficientStock { sku, requested, available } => {
                write!(f, "insufficient stock for {sku}: requested {requested}, available {available}")
            }
            Self::InvalidQuantity => write!(f, "quantity must be positive"),
        }
    }
}

impl std::error::Error for InventoryError {}
