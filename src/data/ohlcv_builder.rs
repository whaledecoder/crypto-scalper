//! Build OHLCV candles from a stream of trades.

use super::types::{Candle, Trade};
use chrono::{DateTime, TimeZone, Utc};

/// Aggregates trades into fixed-interval OHLCV candles.
///
/// Emits a finalized `Candle` whenever a trade crosses the current bucket
/// boundary. The currently forming candle is returned via `current()`.
#[derive(Debug, Clone)]
pub struct OhlcvBuilder {
    interval_secs: i64,
    bucket_start: Option<DateTime<Utc>>,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
}

impl OhlcvBuilder {
    pub fn new(interval_secs: i64) -> Self {
        assert!(interval_secs > 0, "interval must be positive");
        Self {
            interval_secs,
            bucket_start: None,
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            volume: 0.0,
        }
    }

    /// Round down to the nearest bucket boundary.
    fn bucket_for(&self, ts: DateTime<Utc>) -> DateTime<Utc> {
        let s = ts.timestamp();
        let start = s - s.rem_euclid(self.interval_secs);
        Utc.timestamp_opt(start, 0).single().unwrap_or(ts)
    }

    /// Ingest a trade. If a bucket boundary is crossed, returns the finalized
    /// candle of the previous bucket.
    pub fn ingest(&mut self, t: Trade) -> Option<Candle> {
        let bucket = self.bucket_for(t.ts);
        let mut finalized = None;

        match self.bucket_start {
            None => {
                self.bucket_start = Some(bucket);
                self.open = t.price;
                self.high = t.price;
                self.low = t.price;
                self.close = t.price;
                self.volume = t.qty;
            }
            Some(bs) if bucket > bs => {
                finalized = Some(self.finalize(bs));
                self.bucket_start = Some(bucket);
                self.open = t.price;
                self.high = t.price;
                self.low = t.price;
                self.close = t.price;
                self.volume = t.qty;
            }
            Some(_) => {
                if t.price > self.high {
                    self.high = t.price;
                }
                if t.price < self.low {
                    self.low = t.price;
                }
                self.close = t.price;
                self.volume += t.qty;
            }
        }
        finalized
    }

    fn finalize(&self, bucket_start: DateTime<Utc>) -> Candle {
        Candle {
            open_time: bucket_start,
            close_time: bucket_start + chrono::Duration::seconds(self.interval_secs),
            open: self.open,
            high: self.high,
            low: self.low,
            close: self.close,
            volume: self.volume,
        }
    }

    /// View of the currently-forming candle (not yet closed).
    pub fn current(&self) -> Option<Candle> {
        self.bucket_start.map(|bs| self.finalize(bs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trade(ts_sec: i64, price: f64, qty: f64) -> Trade {
        Trade {
            ts: Utc.timestamp_opt(ts_sec, 0).unwrap(),
            price,
            qty,
            is_buyer_maker: false,
        }
    }

    #[test]
    fn builds_candles_at_interval() {
        let mut b = OhlcvBuilder::new(60);
        assert!(b.ingest(trade(0, 100.0, 1.0)).is_none());
        assert!(b.ingest(trade(30, 101.0, 0.5)).is_none());
        assert!(b.ingest(trade(59, 99.5, 0.2)).is_none());
        let c = b.ingest(trade(60, 100.5, 1.0)).expect("finalize");
        assert_eq!(c.open, 100.0);
        assert_eq!(c.high, 101.0);
        assert_eq!(c.low, 99.5);
        assert_eq!(c.close, 99.5);
        approx::assert_abs_diff_eq!(c.volume, 1.7, epsilon = 1e-9);
    }
}
