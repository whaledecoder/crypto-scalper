#[derive(Debug, Clone, Copy)]
pub struct Toxicity {
    pub ofi_z_abs_limit: f64,
    pub vpin_limit: f64,
    pub spread_pct_limit: f64,
}

impl Toxicity {
    pub fn is_toxic(&self, ofi_z: Option<f64>, vpin: Option<f64>, spread_pct: Option<f64>) -> bool {
        ofi_z
            .map(|x| x.abs() > self.ofi_z_abs_limit)
            .unwrap_or(false)
            || vpin.map(|x| x > self.vpin_limit).unwrap_or(false)
            || spread_pct
                .map(|x| x > self.spread_pct_limit)
                .unwrap_or(false)
    }
}

impl Default for Toxicity {
    fn default() -> Self {
        Self {
            ofi_z_abs_limit: 3.0,
            vpin_limit: 0.75,
            spread_pct_limit: 0.03,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_toxic_flow() {
        let t = Toxicity::default();
        assert!(t.is_toxic(Some(3.5), None, None));
        assert!(t.is_toxic(None, Some(0.8), None));
        assert!(!t.is_toxic(Some(0.5), Some(0.2), Some(0.01)));
    }
}
