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
│ Layer 6 — Learning    │ Trade history → lessons → adaptive policy          │
└────────────────────────────────────────────────────────────────────────────┘
```

### Layer 6 — Learning system

Every closed trade lands in the SQLite journal with all of its TA + LLM
context. A background task refreshes a `LearningPolicy` from the journal
every 5 minutes, deriving stats and turning them into actionable rules:

| Lesson | Trigger | Action |
|---|---|---|
| `LoseStreak` | ≥3 losses in a row on `(strategy, symbol)` | Skip 30 min |
| `StrategyDerate` | Strategy WR < 35% over ≥8 trades | +10 TA threshold, ½ size, LLM floor 80 |
| `StrategyBoost` | Strategy WR ≥ 65% & PF ≥ 1.5 | −5 TA threshold, 1.2× size |
| `RegimeBlacklist` | `(strategy, regime)` WR < 30% over ≥12 trades | Drop from regime selector for 12h |
| `LlmCalibration` | LLM 80–100 confidence picks land < 40% WR | Raise `min_confidence` to 90 |
| `SymbolDerate` | Symbol net negative over ≥8 trades, WR < 35% | Pause symbol 24h |
| `DrawdownCooldown` | ≤−5% equity in last 60 min over ≥2 trades | Pause everything 60 min |

The policy is consulted at every layer:

- **Layer 2** (`select_strategies`): blacklisted `(strategy, symbol)` combos are
  filtered out before evaluation.
- **Layer 3** (LLM context): `[HISTORICAL PERFORMANCE]` block is injected
  into the prompt so the LLM can reason about what worked/failed recently.
- **Layer 3 LLM gate**: confidence floor is raised when the calibration
  lesson is active.
- **Layer 4** (Risk): position size is multiplied by the verdict's size
  multiplier (zero on blocks, 0.5× on derate, 1.2× on boost).
- **Layer 5** (Monitoring): `/lessons` and `/dashboard` HTTP endpoints
  expose the currently active lessons.

```bash
curl http://localhost:9184/dashboard | jq .
# {
#   "metrics": { ..., "active_lessons": 3 },
#   "lessons": [
#     {"kind":"LoseStreak","strategy":"vwap_scalp","symbol":"BTCUSDT", ...},
#     {"kind":"StrategyBoost","strategy":"ema_ribbon", ...},
#     ...
#   ]
# }
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
   - `OPENROUTER_API_KEY` (default LLM provider)
   - `ANTHROPIC_API_KEY` / `OPENAI_API_KEY` / `TOGETHER_API_KEY` / `GROQ_API_KEY` — used when `[llm.provider]` is set accordingly
   - `CRYPTOPANIC_API_KEY`, `LUNARCRUSH_API_KEY`,
     `GLASSNODE_API_KEY`, `WHALE_ALERT_API_KEY`
   - `TELEGRAM_BOT_TOKEN`, `TELEGRAM_CHAT_ID`

Provided overlays:

- `config/paper.toml` — forces `run_mode=paper`, `dry_run=true`.
- `config/production.toml` — `run_mode=live`, tighter risk caps.
- `config/llm-anthropic.toml` — switch LLM to Anthropic native.
- `config/llm-openrouter-cheap.toml` — pick a cheap or free OpenRouter model.

### LLM provider matrix

| Provider | `provider =` | `api_base` | Auth header | Env var |
|---|---|---|---|---|
| **OpenRouter** *(default)* | `"openrouter"` | `https://openrouter.ai/api/v1/chat/completions` | `Authorization: Bearer …` | `OPENROUTER_API_KEY` |
| Anthropic native | `"anthropic"` | `https://api.anthropic.com/v1/messages` | `x-api-key: …` | `ANTHROPIC_API_KEY` |
| OpenAI | `"openai"` | `https://api.openai.com/v1/chat/completions` | `Authorization: Bearer …` | `OPENAI_API_KEY` |
| Together | `"together"` | `https://api.together.xyz/v1/chat/completions` | `Authorization: Bearer …` | `TOGETHER_API_KEY` |
| Groq | `"groq"` | `https://api.groq.com/openai/v1/chat/completions` | `Authorization: Bearer …` | `GROQ_API_KEY` |

OpenRouter sample models (price ≈ in/out per 1M tokens):

| Model | Cost | Notes |
|---|---|---|
| `anthropic/claude-3.5-haiku` | $0.80 / $4 | Spec-default (smart, fast) |
| `anthropic/claude-3.5-sonnet` | $3 / $15 | Best quality |
| `openai/gpt-4o-mini` | $0.15 / $0.60 | Solid generalist |
| `deepseek/deepseek-chat` | $0.14 / $0.28 | Cheap & sharp on TA reasoning |
| `meta-llama/llama-3.3-70b-instruct` | $0.13 / $0.39 | Fast |
| `google/gemini-2.0-flash-exp:free` | **FREE** | Rate-limited, great for paper-mode testing |
| `qwen/qwen-2.5-72b-instruct:free` | **FREE** | Rate-limited |

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
├── learning/           # Layer 6 — performance memory, lessons, policy
├── agents/             # multi-agent runtime (data/feeds/signal/risk/brain/manager/execution/monitor/learning)
├── backtest/           # replay engine + performance metrics
├── lib.rs              # module re-exports
└── main.rs             # multi-agent orchestrator binary `aria`
```

## Multi-Agent Architecture

Every layer of the stack runs as an independent tokio task that
communicates exclusively over a typed `MessageBus`
(`tokio::sync::broadcast`, capacity 4096). Adding `[manager]
enabled = true` to your config inserts a second LLM specialist —
the `TraderManagerAgent` — between the brain and the execution
agent so it can `Approve`, `Veto`, or `Adjust(size_mult, sl_offset_bps,
tp_offset_bps)` every signal:

```
DataAgent → SignalAgent → RiskAgent → BrainAgent → TraderManagerAgent → ExecutionAgent
   │            │             │           │                 │                │
   │            └────── FeedsAgent ───────┘                 │                │
   │                                                        │                │
   └─────────────── LearningAgent (broadcasts policy refresh on every loop)──┘
                                                                             │
                                                          MonitorAgent ←─────┘
```

- `DataAgent` owns the WebSocket and OHLCV builders.
- `FeedsAgent` polls fear&greed / funding / news / sentiment / on-chain.
- `SignalAgent` runs the regime detector + active strategies.
- `RiskAgent` applies the 8-gate `RiskManager` plus learning-policy filters.
- `BrainAgent` builds the `MarketContext` and runs the brain LLM.
- `TraderManagerAgent` consumes every other agent's output and gives the final verdict (optional, off by default).
- `ExecutionAgent` dispatches the order and watches mark-price exits.
- `MonitorAgent` writes metrics + journal + Telegram alerts.
- `LearningAgent` rebuilds `LearningPolicy` from the trade journal every 5 minutes.

To enable the manager LLM:

```toml
[manager]
enabled = true
provider = "openrouter"          # or "anthropic"
model    = "anthropic/claude-3.5-haiku"
fast_approve_min_conf = 90       # skip LLM call when brain conf >= this and no lessons matched
```

…then export `MANAGER_API_KEY` (or reuse `OPENROUTER_API_KEY` /
`ANTHROPIC_API_KEY` — the manager falls back to the brain's key if
its own is empty).


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
