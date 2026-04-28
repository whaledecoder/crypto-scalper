# ARIA — Autonomous Realtime Intelligence Analyst

LLM-powered autonomous crypto scalping bot, written in Rust. ARIA combines
deterministic technical analysis with an LLM decision engine that evaluates
the full market context (news, social sentiment, on-chain, funding) before
every trade.

```
┌────────────────────────────────────────────────────────────────────────────┐
│ Layer 1 — Data        │ Binance WS/REST · news · on-chain · F&G · funding  │
│ Layer 2 — Signal      │ 10+ indicators · regime detector · 5 strategies    │
│ Layer 3 — Brain       │ Context packet → Claude → GO/NO_GO/WAIT            │
│ Layer 4 — Execution   │ Risk gates · position sizer · Binance OCO          │
│ Layer 5 — Monitoring  │ SQLite journal · Telegram · /metrics HTTP          │
└────────────────────────────────────────────────────────────────────────────┘
```

## Features

- **Incremental indicators** (EMA, RSI, Bollinger, ATR, ADX, VWAP, Choppiness,
  Keltner, ROC) — all updating per closed candle, no bulk recomputation.
- **Market regime detector** (Trending / Ranging / Volatile / Squeeze) with
  ADX + Choppiness + BB-in-KC test.
- **5 strategies** — Mean Reversion, Momentum Breakout, VWAP Scalp, EMA Ribbon,
  Volatility Squeeze. Each emits a `PreSignal` with a TA confidence score.
- **LLM decision engine** — Anthropic Claude 3.5 Haiku by default. Falls back to
  TA-only if the API times out (5 s) or the key is missing.
- **Risk manager** — per-trade sizing by risk %, circuit breakers for daily
  loss, drawdown, and max open positions.
- **Execution abstraction** — Binance Futures REST (HMAC-SHA256 signed) or
  in-process paper exchange for dry-run.
- **SQLite trade journal** — every decision (including the full LLM reasoning)
  is stored for post-trade review and future fine-tuning.
- **HTTP metrics endpoint** — `/metrics` serves a JSON snapshot; `/healthz`
  for uptime checks.
- **Backtest engine** — replays historical OHLCV CSVs through the same signal
  pipeline and reports WR, PF, Sharpe, Sortino, drawdown.

## Quick Start

```bash
# 1. Build (requires Rust 1.85+)
cargo build --release

# 2. Copy the example env and fill in your keys (all optional for paper mode)
cp .env.example .env
# Then: export $(grep -v '^#' .env | xargs)

# 3. Run in paper mode (default)
./target/release/aria

# 4. Metrics are at http://localhost:9184/metrics
```

## Configuration

Configuration is layered:

1. `config/default.toml` — repository-tracked defaults (paper mode).
2. `config/<overlay>.toml` — optional overlay pointed at by `ARIA_CONFIG_OVERLAY`.
3. Environment variables — override any secret:
   - `BINANCE_API_KEY`, `BINANCE_API_SECRET`
   - `ANTHROPIC_API_KEY`
   - `CRYPTOPANIC_API_KEY`, `LUNARCRUSH_API_KEY`,
     `GLASSNODE_API_KEY`, `WHALE_ALERT_API_KEY`
   - `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`

Provided overlays:

- `config/paper.toml` — forces `run_mode=paper`, `dry_run=true`.
- `config/production.toml` — `run_mode=live`, tighter risk caps.

Activate with:

```bash
ARIA_CONFIG_OVERLAY=config/paper.toml ./target/release/aria
```

## Modes

| Mode       | Effect                                                              |
|------------|---------------------------------------------------------------------|
| `paper`    | Full pipeline, no real orders. Safe for tuning signals & LLM prompts. |
| `live`     | Dispatches real orders to Binance (requires API keys + `dry_run=false`). |
| `backtest` | Replays CSVs from `config.backtest.data_dir/<SYMBOL>.csv`.          |

## Backtesting

Place historical candles at `data/historical/BTCUSDT.csv` with header:

```
open_time_ms,open,high,low,close,volume
```

Then run with `run_mode = "backtest"` in your overlay and it will produce a
performance report per symbol.

## Project Layout

```
src/
├── config.rs           # TOML + ENV loader
├── errors.rs           # ScalperError + Result alias
├── data/               # Layer 1 — WS, OHLCV, order book
├── indicators/         # 10+ incremental indicators
├── strategy/           # Layer 2 — state, regime, 5 strategies
├── feeds/              # news / sentiment / on-chain / funding / F&G
├── llm/                # Layer 3 — context builder, prompts, engine
├── execution/          # Layer 4 — risk, orders, exchange abstraction
├── monitoring/         # Layer 5 — SQLite, Telegram, HTTP metrics
├── backtest/           # replay engine + performance metrics
├── lib.rs              # module re-exports
└── main.rs             # orchestrator binary `aria`
```

## Running Tests

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --lib
```

## Security Notes

- Never commit `config/*.toml` with real API keys. Use environment variables
  or a git-ignored overlay.
- `paper` mode never talks to the exchange and cannot place orders.
- Risk limits are enforced **before** every order dispatch (8-gate system).

## License

MIT
