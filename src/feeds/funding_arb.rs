use crate::feeds::FundingSnapshot;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FundingArbSignal {
    ReceiveFunding,
    PayFundingOnlyWithStrongTrend,
    Neutral,
}

pub fn funding_edge_bps(snapshot: &FundingSnapshot, holding_periods: f64) -> f64 {
    let periods = holding_periods.max(0.0);
    snapshot.predicted_rate.unwrap_or(snapshot.rate) * periods * 10_000.0
}

pub fn classify_funding(rate: f64, threshold_bps: f64) -> FundingArbSignal {
    let rate_bps = rate * 10_000.0;
    if rate_bps >= threshold_bps {
        FundingArbSignal::ReceiveFunding
    } else if rate_bps <= -threshold_bps {
        FundingArbSignal::PayFundingOnlyWithStrongTrend
    } else {
        FundingArbSignal::Neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_funding_edge() {
        let snapshot = FundingSnapshot {
            symbol: "BTCUSDT".into(),
            rate: 0.0001,
            predicted_rate: None,
            open_interest: None,
        };
        approx::assert_abs_diff_eq!(funding_edge_bps(&snapshot, 3.0), 3.0, epsilon = 1e-12);
        assert_eq!(
            classify_funding(0.0002, 1.0),
            FundingArbSignal::ReceiveFunding
        );
    }
}
