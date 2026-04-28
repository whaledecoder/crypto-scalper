//! Core value types shared across layers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// A single OHLCV candle.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Candle {
    pub open_time: DateTime<Utc>,
    pub close_time: DateTime<Utc>,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
}

impl Candle {
    pub fn typical_price(&self) -> f64 {
        (self.high + self.low + self.close) / 3.0
    }

    pub fn range(&self) -> f64 {
        self.high - self.low
    }

    pub fn body(&self) -> f64 {
        (self.close - self.open).abs()
    }

    pub fn is_bullish(&self) -> bool {
        self.close > self.open
    }
}

/// A trade print from the exchange tape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Trade {
    pub ts: DateTime<Utc>,
    pub price: f64,
    pub qty: f64,
    pub is_buyer_maker: bool,
}

/// Direction of a trading position / signal.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Side {
    Long,
    Short,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Long => "LONG",
            Side::Short => "SHORT",
        }
    }
}

impl std::str::FromStr for Side {
    type Err = crate::errors::ScalperError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_uppercase().as_str() {
            "LONG" | "BUY" => Ok(Side::Long),
            "SHORT" | "SELL" => Ok(Side::Short),
            other => Err(Self::Err::Parse(format!("invalid side `{other}`"))),
        }
    }
}

/// Timeframe in seconds.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Timeframe {
    pub seconds: i64,
}

impl Timeframe {
    pub fn parse(s: &str) -> Result<Self, crate::errors::ScalperError> {
        let s = s.trim();
        if let Some(n) = s.strip_suffix('s') {
            return n
                .parse::<i64>()
                .map(|x| Timeframe { seconds: x })
                .map_err(|e| crate::errors::ScalperError::Parse(e.to_string()));
        }
        if let Some(n) = s.strip_suffix('m') {
            return n
                .parse::<i64>()
                .map(|x| Timeframe { seconds: x * 60 })
                .map_err(|e| crate::errors::ScalperError::Parse(e.to_string()));
        }
        if let Some(n) = s.strip_suffix('h') {
            return n
                .parse::<i64>()
                .map(|x| Timeframe { seconds: x * 3600 })
                .map_err(|e| crate::errors::ScalperError::Parse(e.to_string()));
        }
        if let Some(n) = s.strip_suffix('d') {
            return n
                .parse::<i64>()
                .map(|x| Timeframe { seconds: x * 86400 })
                .map_err(|e| crate::errors::ScalperError::Parse(e.to_string()));
        }
        Err(crate::errors::ScalperError::Parse(format!(
            "invalid timeframe `{s}`"
        )))
    }

    pub fn as_str(&self) -> String {
        let s = self.seconds;
        if s % 86400 == 0 {
            format!("{}d", s / 86400)
        } else if s % 3600 == 0 {
            format!("{}h", s / 3600)
        } else if s % 60 == 0 {
            format!("{}m", s / 60)
        } else {
            format!("{s}s")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timeframe_parse_roundtrip() {
        let cases = ["1m", "5m", "15m", "1h", "4h", "1d", "30s"];
        for c in cases {
            let tf = Timeframe::parse(c).unwrap();
            assert_eq!(tf.as_str(), c);
        }
    }

    #[test]
    fn side_parse() {
        assert_eq!("LONG".parse::<Side>().unwrap(), Side::Long);
        assert_eq!("buy".parse::<Side>().unwrap(), Side::Long);
        assert_eq!("short".parse::<Side>().unwrap(), Side::Short);
        assert!("wat".parse::<Side>().is_err());
    }
}
