use crate::data::Candle;
use crate::research::ic::IcTracker;
use chrono::{DateTime, Utc};

#[derive(Debug, Clone, Copy)]
pub struct SignalObservation {
    pub ts: DateTime<Utc>,
    pub value: f64,
}

pub fn compute_ic_decay(
    signals: &[SignalObservation],
    candles: &[Candle],
    max_horizon: usize,
) -> Vec<(usize, f64)> {
    if signals.is_empty() || candles.len() < 2 || max_horizon == 0 {
        return Vec::new();
    }
    (1..=max_horizon)
        .filter_map(|horizon| {
            let mut tracker = IcTracker::new(30);
            for signal in signals {
                let idx = candles
                    .iter()
                    .position(|c| c.open_time <= signal.ts && c.close_time >= signal.ts)?;
                let future = candles.get(idx + horizon)?;
                let current = candles.get(idx)?;
                if current.close <= 0.0 {
                    continue;
                }
                let forward_return = (future.close / current.close) - 1.0;
                tracker.record(signal.value, forward_return);
            }
            tracker.ic().map(|ic| (horizon, ic))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{Duration, TimeZone};

    #[test]
    fn computes_decay_by_horizon() {
        let start = Utc.timestamp_opt(1_700_000_000, 0).unwrap();
        let candles: Vec<Candle> = (0..40)
            .map(|i| Candle {
                open_time: start + Duration::minutes(i),
                close_time: start + Duration::minutes(i + 1),
                open: 100.0 + i as f64,
                high: 101.0 + i as f64,
                low: 99.0 + i as f64,
                close: 100.0 + i as f64,
                volume: 10.0,
            })
            .collect();
        let signals: Vec<SignalObservation> = candles
            .iter()
            .take(35)
            .map(|c| SignalObservation {
                ts: c.close_time,
                value: c.close.ln(),
            })
            .collect();
        let decay = compute_ic_decay(&signals, &candles, 3);
        assert_eq!(decay.len(), 3);
        assert!(decay[0].1.is_finite());
    }
}
