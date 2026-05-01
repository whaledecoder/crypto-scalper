#[derive(Debug, Clone, Copy)]
pub struct OptionSkewSnapshot {
    pub call_25d_iv: f64,
    pub put_25d_iv: f64,
    pub atm_iv: f64,
}

impl OptionSkewSnapshot {
    pub fn skew_bps(&self) -> f64 {
        if self.atm_iv <= 0.0 {
            return 0.0;
        }
        (self.call_25d_iv - self.put_25d_iv) / self.atm_iv * 10_000.0
    }

    pub fn sentiment_score(&self) -> f64 {
        (self.skew_bps() / 500.0).clamp(-1.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scores_iv_skew() {
        let skew = OptionSkewSnapshot {
            call_25d_iv: 0.65,
            put_25d_iv: 0.55,
            atm_iv: 0.60,
        };
        assert!(skew.skew_bps() > 0.0);
        assert!(skew.sentiment_score() > 0.0);
    }
}
