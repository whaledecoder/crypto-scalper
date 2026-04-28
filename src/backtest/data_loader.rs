//! CSV loader for historical OHLCV candles.
//!
//! Expected columns (header required, comma-separated):
//!   `open_time_ms,open,high,low,close,volume`

use crate::data::Candle;
use crate::errors::{Result, ScalperError};
use chrono::{TimeZone, Utc};
use std::path::Path;

pub fn load_csv(path: impl AsRef<Path>, interval_secs: i64) -> Result<Vec<Candle>> {
    let s = std::fs::read_to_string(path)?;
    let mut out = Vec::new();
    let mut lines = s.lines();
    let header = lines
        .next()
        .ok_or_else(|| ScalperError::Parse("empty csv".into()))?;
    let cols: Vec<&str> = header.split(',').collect();
    let col_ot = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("open_time_ms"))
        .unwrap_or(0);
    let col_o = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("open"))
        .unwrap_or(1);
    let col_h = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("high"))
        .unwrap_or(2);
    let col_l = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("low"))
        .unwrap_or(3);
    let col_c = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("close"))
        .unwrap_or(4);
    let col_v = cols
        .iter()
        .position(|h| h.eq_ignore_ascii_case("volume"))
        .unwrap_or(5);
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let parts: Vec<&str> = trimmed.split(',').collect();
        if parts.len() < 6 {
            continue;
        }
        let ot_ms: i64 = parts
            .get(col_ot)
            .ok_or_else(|| ScalperError::Parse("missing open_time".into()))?
            .parse()
            .map_err(|e| ScalperError::Parse(format!("open_time: {e}")))?;
        let open: f64 = parts[col_o]
            .parse()
            .map_err(|e| ScalperError::Parse(format!("open: {e}")))?;
        let high: f64 = parts[col_h]
            .parse()
            .map_err(|e| ScalperError::Parse(format!("high: {e}")))?;
        let low: f64 = parts[col_l]
            .parse()
            .map_err(|e| ScalperError::Parse(format!("low: {e}")))?;
        let close: f64 = parts[col_c]
            .parse()
            .map_err(|e| ScalperError::Parse(format!("close: {e}")))?;
        let volume: f64 = parts[col_v]
            .parse()
            .map_err(|e| ScalperError::Parse(format!("volume: {e}")))?;
        let open_time = Utc
            .timestamp_millis_opt(ot_ms)
            .single()
            .ok_or_else(|| ScalperError::Parse("bad ts".into()))?;
        let close_time = open_time + chrono::Duration::seconds(interval_secs);
        out.push(Candle {
            open_time,
            close_time,
            open,
            high,
            low,
            close,
            volume,
        });
    }
    Ok(out)
}
