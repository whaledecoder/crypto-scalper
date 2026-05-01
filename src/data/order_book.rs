//! Level-2 order book snapshot + bid/ask pressure analytics.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Level {
    pub price: f64,
    pub qty: f64,
}

/// Aggregated L2 snapshot used by the signal engine and LLM context.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct OrderBook {
    /// Bids sorted desc by price.
    pub bids: Vec<Level>,
    /// Asks sorted asc by price.
    pub asks: Vec<Level>,
}

impl OrderBook {
    pub fn best_bid(&self) -> Option<f64> {
        self.bids.first().map(|l| l.price)
    }

    pub fn best_ask(&self) -> Option<f64> {
        self.asks.first().map(|l| l.price)
    }

    pub fn spread(&self) -> Option<f64> {
        Some(self.best_ask()? - self.best_bid()?)
    }

    /// Spread as a percentage of mid-price. Returns `None` if book empty.
    pub fn spread_pct(&self) -> Option<f64> {
        let (b, a) = (self.best_bid()?, self.best_ask()?);
        let mid = (a + b) / 2.0;
        if mid <= 0.0 {
            return None;
        }
        Some((a - b) / mid * 100.0)
    }

    /// Ratio of total bid-side liquidity to total ask-side within top `depth` levels.
    pub fn bid_ask_ratio(&self, depth: usize) -> f64 {
        let b: f64 = self.bids.iter().take(depth).map(|l| l.qty).sum();
        let a: f64 = self.asks.iter().take(depth).map(|l| l.qty).sum();
        if a <= 0.0 {
            return f64::INFINITY;
        }
        b / a
    }

    pub fn top_bid_qty(&self) -> Option<f64> {
        self.bids.first().map(|l| l.qty)
    }

    pub fn top_ask_qty(&self) -> Option<f64> {
        self.asks.first().map(|l| l.qty)
    }

    /// Return the biggest bid wall in the book.
    pub fn bid_wall(&self) -> Option<Level> {
        self.bids.iter().copied().max_by(|x, y| {
            x.qty
                .partial_cmp(&y.qty)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Return the biggest ask wall in the book.
    pub fn ask_wall(&self) -> Option<Level> {
        self.asks.iter().copied().max_by(|x, y| {
            x.qty
                .partial_cmp(&y.qty)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    /// Replace the top-of-book with a single (bid, ask) quote — used for
    /// streaming `bookTicker` updates.
    pub fn set_top(&mut self, best_bid: f64, best_ask: f64) {
        self.set_top_with_qty(best_bid, 0.0, best_ask, 0.0);
    }

    pub fn set_top_with_qty(&mut self, best_bid: f64, bid_qty: f64, best_ask: f64, ask_qty: f64) {
        self.bids = vec![Level {
            price: best_bid,
            qty: bid_qty,
        }];
        self.asks = vec![Level {
            price: best_ask,
            qty: ask_qty,
        }];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_and_ratio() {
        let ob = OrderBook {
            bids: vec![
                Level {
                    price: 100.0,
                    qty: 5.0,
                },
                Level {
                    price: 99.5,
                    qty: 3.0,
                },
            ],
            asks: vec![
                Level {
                    price: 100.1,
                    qty: 2.0,
                },
                Level {
                    price: 100.5,
                    qty: 2.0,
                },
            ],
        };
        approx::assert_abs_diff_eq!(ob.spread().unwrap(), 0.1, epsilon = 1e-9);
        approx::assert_abs_diff_eq!(ob.bid_ask_ratio(2), 8.0 / 4.0, epsilon = 1e-9);
        assert_eq!(ob.bid_wall().unwrap().price, 100.0);
    }

    #[test]
    fn stores_top_quantities() {
        let mut ob = OrderBook::default();
        ob.set_top_with_qty(100.0, 7.0, 100.1, 5.0);
        assert_eq!(ob.top_bid_qty().unwrap(), 7.0);
        assert_eq!(ob.top_ask_qty().unwrap(), 5.0);
    }
}
