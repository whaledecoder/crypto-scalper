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
- Backtest research report output in markdown or JSON.
- Safe advanced-alpha gate scaffolding for future live wiring.
- Disabled-by-default advanced-alpha wiring into pre-signal confirmation.
- Advanced-alpha feed staleness guard.
- Public Deribit BTC/ETH options adapter wired into `FeedsSnapshot`.
- Optional Glassnode/Whale Alert BTC/ETH on-chain adapters.
- Optional CryptoPanic/LunarCrush alternative-data adapters.

## Still intentionally pending

- Live calibration of external-data weights against real paper/live outcomes.

The remaining items require deeper data dependencies or live-trading calibration, so they should be delivered as focused PRs after the primitives are merged.
