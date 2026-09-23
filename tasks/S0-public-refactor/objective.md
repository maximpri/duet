`src/inventory.rs` has grown into one file and reports errors as `String`s. Refactor it:

1. Turn it into a module directory `src/inventory/` whose submodules are public (`pub mod`):
   - `mod.rs` — the `Inventory` type and `Item`, re-exporting what callers need;
   - `stock.rs` — stock operations as methods on `Inventory` (`add_item`, `remove`, `restock`,
     `low_stock`);
   - `pricing.rs` — `pub fn price_with_discount(unit_price_cents: u64, quantity: u32) -> u64`
     and `Inventory::category_totals`;
   - `report.rs` — `Inventory::render_report`.
2. Replace every `String` error with a public enum `inventory::InventoryError` with exactly
   these variants:
   `DuplicateSku(String)`, `UnknownSku(String)`,
   `InsufficientStock { sku: String, requested: u32, available: u32 }`, `InvalidQuantity`.
   Implement `std::fmt::Display` and `std::error::Error` for it.
3. Behaviour, including the exact report text, must not change. Update the existing tests to
   the new API.

Use only the standard library.
