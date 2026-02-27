use std::collections::HashMap;
use rust_decimal::Decimal;
use crate::events::Side;

/// A standing order in the book.
#[derive(Debug, Clone)]
pub struct StandingOrder {
    pub order_id: String,
    /// Price in ticks (0-1000)
    pub price: u16,
    /// Remaining size (decreases on partial fills)
    pub remaining_size: Decimal,
    /// Original size when placed
    pub original_size: Decimal,
}

/// Tracks standing orders for both YES and NO sides.
/// Supports multiple orders per price level (stacking).
#[derive(Debug, Default)]
pub struct OrderTracker {
    /// YES orders: price → list of orders at that price
    yes_orders: HashMap<u16, Vec<StandingOrder>>,
    /// NO orders: price → list of orders at that price
    no_orders: HashMap<u16, Vec<StandingOrder>>,
}

impl OrderTracker {
    pub fn new() -> Self {
        Self::default()
    }

    fn orders_mut(&mut self, side: Side) -> &mut HashMap<u16, Vec<StandingOrder>> {
        match side {
            Side::Yes => &mut self.yes_orders,
            Side::No => &mut self.no_orders,
        }
    }

    fn orders(&self, side: Side) -> &HashMap<u16, Vec<StandingOrder>> {
        match side {
            Side::Yes => &self.yes_orders,
            Side::No => &self.no_orders,
        }
    }

    // =========================================================================
    // ADD / REMOVE / UPDATE
    // =========================================================================

    /// Add a new order. Appends to list at this price (stacking).
    pub fn add(&mut self, side: Side, order_id: String, price: u16, size: Decimal) {
        let orders = self.orders_mut(side);
        orders.entry(price).or_default().push(StandingOrder {
            order_id,
            price,
            remaining_size: size,
            original_size: size,
        });
    }

    /// Remove a specific order by ID. Returns the removed order or None.
    pub fn remove_by_id(&mut self, side: Side, order_id: &str) -> Option<StandingOrder> {
        let orders = self.orders_mut(side);
        for (price, order_list) in orders.iter_mut() {
            if let Some(idx) = order_list.iter().position(|o| o.order_id == order_id) {
                let removed = order_list.remove(idx);
                let price = *price;
                // Clean up empty price levels
                if order_list.is_empty() {
                    self.orders_mut(side).remove(&price);
                }
                return Some(removed);
            }
        }
        None
    }

    /// Remove all orders at a price. Returns removed orders.
    pub fn remove_at_price(&mut self, side: Side, price: u16) -> Vec<StandingOrder> {
        self.orders_mut(side).remove(&price).unwrap_or_default()
    }

    /// Update remaining size after a fill. Removes order if fully filled.
    pub fn update_fill(&mut self, side: Side, order_id: &str, filled_size: Decimal) {
        let orders = self.orders_mut(side);

        // Find the order by ID
        let mut price_to_check: Option<u16> = None;
        for (price, order_list) in orders.iter_mut() {
            if let Some(order) = order_list.iter_mut().find(|o| o.order_id == order_id) {
                order.remaining_size -= filled_size;
                if order.remaining_size <= Decimal::ZERO {
                    price_to_check = Some(*price);
                }
                break;
            }
        }

        // Remove fully filled order
        if let Some(price) = price_to_check {
            if let Some(order_list) = orders.get_mut(&price) {
                order_list.retain(|o| o.order_id != order_id);
                if order_list.is_empty() {
                    orders.remove(&price);
                }
            }
        }
    }

    /// Clear all orders for a side.
    pub fn clear(&mut self, side: Side) {
        self.orders_mut(side).clear();
    }

    /// Clear all orders for both sides.
    pub fn clear_all(&mut self) {
        self.yes_orders.clear();
        self.no_orders.clear();
    }

    // =========================================================================
    // QUERIES
    // =========================================================================

    /// Get all orders at a specific price.
    pub fn orders_at_price(&self, side: Side, price: u16) -> &[StandingOrder] {
        self.orders(side).get(&price).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get total size of all orders at a price.
    pub fn total_size_at_price(&self, side: Side, price: u16) -> Decimal {
        self.orders(side)
            .get(&price)
            .map(|orders| orders.iter().map(|o| o.remaining_size).sum())
            .unwrap_or(Decimal::ZERO)
    }

    /// Get all prices with standing orders.
    pub fn prices(&self, side: Side) -> Vec<u16> {
        self.orders(side).keys().copied().collect()
    }

    /// Get all standing orders for a side (flattened).
    pub fn all_orders(&self, side: Side) -> Vec<&StandingOrder> {
        self.orders(side)
            .values()
            .flat_map(|v| v.iter())
            .collect()
    }

    /// Get all order IDs for a side.
    pub fn all_order_ids(&self, side: Side) -> Vec<&str> {
        self.all_orders(side)
            .iter()
            .map(|o| o.order_id.as_str())
            .collect()
    }

    /// Count standing orders for a side.
    pub fn count(&self, side: Side) -> usize {
        self.orders(side).values().map(|v| v.len()).sum()
    }

    /// Count total standing orders.
    pub fn total_count(&self) -> usize {
        self.count(Side::Yes) + self.count(Side::No)
    }

    /// Get the highest price with a standing order.
    pub fn top_price(&self, side: Side) -> Option<u16> {
        self.orders(side).keys().max().copied()
    }

    /// Get the lowest price with a standing order.
    pub fn bottom_price(&self, side: Side) -> Option<u16> {
        self.orders(side).keys().min().copied()
    }

    /// Get total exposure (sum of all remaining sizes) for a side.
    pub fn total_exposure(&self, side: Side) -> Decimal {
        self.orders(side)
            .values()
            .flat_map(|v| v.iter())
            .map(|o| o.remaining_size)
            .sum()
    }

    /// Check if there are any orders for a side.
    pub fn has_orders(&self, side: Side) -> bool {
        !self.orders(side).is_empty()
    }

    /// Find price for an order_id. Returns None if not found.
    pub fn find_price_by_id(&self, side: Side, order_id: &str) -> Option<u16> {
        for (price, order_list) in self.orders(side) {
            if order_list.iter().any(|o| o.order_id == order_id) {
                return Some(*price);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_add_and_query() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "order1".to_string(), 450, dec!(10));
        tracker.add(Side::Yes, "order2".to_string(), 440, dec!(10));
        tracker.add(Side::No, "order3".to_string(), 540, dec!(10));

        assert_eq!(tracker.count(Side::Yes), 2);
        assert_eq!(tracker.count(Side::No), 1);
        assert_eq!(tracker.total_count(), 3);

        assert_eq!(tracker.total_size_at_price(Side::Yes, 450), dec!(10));
        assert_eq!(tracker.total_size_at_price(Side::Yes, 999), dec!(0));
    }

    #[test]
    fn test_stacking() {
        let mut tracker = OrderTracker::new();

        // Stack two orders at same price
        tracker.add(Side::Yes, "order1".to_string(), 450, dec!(10));
        tracker.add(Side::Yes, "order2".to_string(), 450, dec!(5));

        assert_eq!(tracker.count(Side::Yes), 2);
        assert_eq!(tracker.total_size_at_price(Side::Yes, 450), dec!(15));
        assert_eq!(tracker.orders_at_price(Side::Yes, 450).len(), 2);
    }

    #[test]
    fn test_remove_by_id() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "order1".to_string(), 450, dec!(10));
        tracker.add(Side::Yes, "order2".to_string(), 450, dec!(5));

        let removed = tracker.remove_by_id(Side::Yes, "order1");
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().remaining_size, dec!(10));

        assert_eq!(tracker.count(Side::Yes), 1);
        assert_eq!(tracker.total_size_at_price(Side::Yes, 450), dec!(5));
    }

    #[test]
    fn test_update_fill_partial() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "order1".to_string(), 450, dec!(10));

        // Partial fill of 3
        tracker.update_fill(Side::Yes, "order1", dec!(3));

        let orders = tracker.orders_at_price(Side::Yes, 450);
        assert_eq!(orders.len(), 1);
        assert_eq!(orders[0].remaining_size, dec!(7));
    }

    #[test]
    fn test_update_fill_complete() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "order1".to_string(), 450, dec!(10));

        // Complete fill
        tracker.update_fill(Side::Yes, "order1", dec!(10));

        assert_eq!(tracker.count(Side::Yes), 0);
        assert!(!tracker.has_orders(Side::Yes));
    }

    #[test]
    fn test_prices() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "o1".to_string(), 450, dec!(10));
        tracker.add(Side::Yes, "o2".to_string(), 440, dec!(10));
        tracker.add(Side::Yes, "o3".to_string(), 430, dec!(10));

        let mut prices = tracker.prices(Side::Yes);
        prices.sort();
        assert_eq!(prices, vec![430, 440, 450]);

        assert_eq!(tracker.top_price(Side::Yes), Some(450));
        assert_eq!(tracker.bottom_price(Side::Yes), Some(430));
    }

    #[test]
    fn test_total_exposure() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "o1".to_string(), 450, dec!(10));
        tracker.add(Side::Yes, "o2".to_string(), 440, dec!(12));
        tracker.add(Side::Yes, "o3".to_string(), 430, dec!(8));

        assert_eq!(tracker.total_exposure(Side::Yes), dec!(30));
    }

    #[test]
    fn test_clear() {
        let mut tracker = OrderTracker::new();

        tracker.add(Side::Yes, "o1".to_string(), 450, dec!(10));
        tracker.add(Side::No, "o2".to_string(), 540, dec!(10));

        tracker.clear(Side::Yes);
        assert_eq!(tracker.count(Side::Yes), 0);
        assert_eq!(tracker.count(Side::No), 1);

        tracker.clear_all();
        assert_eq!(tracker.total_count(), 0);
    }
}
