# Quant roadmap status

Status against the supplied markdown roadmap.

## Completed

- P0 transaction cost model.
- P0 realistic backtest fee, slippage, market-impact, and annualization fixes.
- P1 walk-forward split and OOS robustness evaluation primitives.
- P1 multi-timeframe weighted vote aggregation.
- P1 OFI stream plumbing and strategy confidence confirmation.
- P2 IC/IR, IC decay, and permutation significance primitives.
- P2 volatility targeting, Kelly, correlation, exposure, VaR, and CVaR helpers.
- Phase 2 execution quality tracking and limit-order fill probability/planning.
- Phase 5 strategy retirement, A/B variant comparison, and parameter sensitivity helpers.
- Monte Carlo drawdown confidence intervals.
- HMM regime inference primitive.
- Kalman trend estimation primitive.
- BTC/ETH pairs spread and hedge-ratio helpers.
- Funding-rate arbitrage edge classifier.
- Alternative-data factor scoring.
- Deribit-style options IV-skew sentiment scoring.
- Research summary health classification.

## Still intentionally pending

- Wiring advanced-alpha primitives into live strategy selection.
- Production CLI/reporting pipeline for automated research reports.
- Real external data adapters for Deribit/options and richer alternative data.

The remaining items require deeper data dependencies or live-trading calibration, so they should be delivered as focused PRs after the primitives are merged.
