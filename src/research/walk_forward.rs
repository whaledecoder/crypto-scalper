#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WalkForwardSplit {
    pub train_start: usize,
    pub train_end: usize,
    pub test_start: usize,
    pub test_end: usize,
}

pub fn walk_forward_splits(
    len: usize,
    train_window: usize,
    test_window: usize,
    step: usize,
) -> Vec<WalkForwardSplit> {
    if len == 0 || train_window == 0 || test_window == 0 || step == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    let mut train_start = 0usize;
    while train_start + train_window + test_window <= len {
        let train_end = train_start + train_window;
        let test_end = train_end + test_window;
        out.push(WalkForwardSplit {
            train_start,
            train_end,
            test_start: train_end,
            test_end,
        });
        train_start += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_rolling_splits() {
        let splits = walk_forward_splits(100, 50, 10, 10);
        assert_eq!(splits.len(), 5);
        assert_eq!(
            splits[0],
            WalkForwardSplit {
                train_start: 0,
                train_end: 50,
                test_start: 50,
                test_end: 60
            }
        );
        assert_eq!(splits[4].test_end, 100);
    }
}
