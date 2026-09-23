use super::{Inventory, InventoryError, Item};

impl Inventory {
    pub fn add_item(&mut self, item: Item) -> Result<(), InventoryError> {
        if item.quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        if self.items.contains_key(&item.sku) {
            return Err(InventoryError::DuplicateSku(item.sku));
        }
        self.items.insert(item.sku.clone(), item);
        Ok(())
    }

    pub fn remove(&mut self, sku: &str, quantity: u32) -> Result<u32, InventoryError> {
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        let item = self.items.get_mut(sku).ok_or_else(|| InventoryError::UnknownSku(sku.into()))?;
        if item.quantity < quantity {
            return Err(InventoryError::InsufficientStock {
                sku: sku.into(),
                requested: quantity,
                available: item.quantity,
            });
        }
        item.quantity -= quantity;
        Ok(item.quantity)
    }

    pub fn restock(&mut self, sku: &str, quantity: u32) -> Result<u32, InventoryError> {
        if quantity == 0 {
            return Err(InventoryError::InvalidQuantity);
        }
        let item = self.items.get_mut(sku).ok_or_else(|| InventoryError::UnknownSku(sku.into()))?;
        item.quantity += quantity;
        Ok(item.quantity)
    }

    /// SKUs at or below `threshold`, sorted by quantity then SKU.
    pub fn low_stock(&self, threshold: u32) -> Vec<&Item> {
        let mut low: Vec<&Item> = self.items.values().filter(|i| i.quantity <= threshold).collect();
        low.sort_by(|a, b| a.quantity.cmp(&b.quantity).then(a.sku.cmp(&b.sku)));
        low
    }
}
