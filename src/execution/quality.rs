use crate::data::Side;

#[derive(Debug, Clone)]
pub struct TradeQualityRecord {
    pub symbol: String,
    pub decision_price: f64,
    pub arrival_price: f64,
    pub fill_price: f64,
    pub side: Side,
    pub size: f64,
}

impl TradeQualityRecord {
    pub fn implementation_shortfall_bps(&self) -> f64 {
        directional_bps(self.decision_price, self.fill_price, self.side)
    }

    pub fn delay_cost_bps(&self) -> f64 {
        directional_bps(self.decision_price, self.arrival_price, self.side)
    }

    pub fn market_impact_bps(&self) -> f64 {
        directional_bps(self.arrival_price, self.fill_price, self.side)
    }
}

fn directional_bps(from: f64, to: f64, side: Side) -> f64 {
    if from <= 0.0 {
        return 0.0;
    }
    let direction = match side {
        Side::Long => 1.0,
        Side::Short => -1.0,
    };
    (to - from) / from * direction * 10_000.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decomposes_shortfall() {
        let rec = TradeQualityRecord {
            symbol: "BTCUSDT".into(),
            decision_price: 100.0,
            arrival_price: 100.1,
            fill_price: 100.2,
            side: Side::Long,
            size: 1.0,
        };
        approx::assert_abs_diff_eq!(rec.implementation_shortfall_bps(), 20.0, epsilon = 1e-9);
        approx::assert_abs_diff_eq!(rec.delay_cost_bps(), 10.0, epsilon = 1e-9);
        assert!(rec.market_impact_bps() > 9.0);
    }
}
