# Configuration Reference

Every section of `config/default.toml` documented, with defaults and
meaning. The configuration is layered:

1. `config/default.toml` — repository-tracked baseline (paper mode, conservative).
2. **Optional overlay** at `ARIA_CONFIG_OVERLAY` — same TOML schema, partial OK.
3. **Environment variables** — final override for secrets.

`#[serde(default)]` is set on all optional sections, so omitting
`[survival]` or `[control]` from your overlay is safe.

---

## `[mode]`

| Key | Default | Meaning |
|---|---|---|
| `run_mode` | `"paper"` | `paper` (simulated fills), `live` (real orders), `backtest` (CSV replay + exit) |
| `dry_run` | `true` | When `true`, even live mode will skip order dispatch (extra safety) |

In `live` mode, **both** `run_mode = "live"` and `dry_run = false` are
required to actually place orders.

---

## `[exchange]`

| Key | Default | Meaning |
|---|---|---|
| `name` | `"binance"` | informational |
| `market` | `"futures"` | `spot`, `futures`, or `testnet` |
| `rest_base_url` | `"https://fapi.binance.com"` | REST root |
| `ws_base_url` | `"wss://fstream.binance.com/stream"` | WS root |
| `api_key` | `""` | overridden by `BINANCE_API_KEY` env var |
| `api_secret` | `""` | overridden by `BINANCE_API_SECRET` env var |
| `recv_window_ms` | `5000` | Binance request validity window |

For Binance Testnet, set `rest_base_url` to `https://testnet.binancefuture.com`
and `ws_base_url` to `wss://stream.binancefuture.com/stream`.

---

## `[pairs]`

| Key | Default | Meaning |
|---|---|---|
| `symbols` | `["BTCUSDT", "ETHUSDT", "SOLUSDT"]` | Symbols to subscribe to |
| `timeframes` | `["5m"]` | Candle interval; first entry is used |

---

## `[strategy]`

| Key | Default | Meaning |
|---|---|---|
| `mode` | `"adaptive"` | placeholder for future use |
| `active` | `["mean_reversion", "ema_ribbon", "momentum", "vwap_scalp", "squeeze"]` | Active strategy set |
| `min_ta_confidence` | `65` | Minimum TA confidence to proceed past `RiskAgent` |

Strategy code lives in `src/strategy/`. To remove a strategy, drop it
from `active`.

---

## `[llm]` — brain LLM

The first LLM in the pipeline; reads the full `MarketContext` and
returns `GO`, `NO_GO`, or `WAIT`.

| Key | Default | Meaning |
|---|---|---|
| `provider` | `"openrouter"` | `openrouter` / `anthropic` / `openai` / `together` / `groq` |
| `model` | `"anthropic/claude-3.5-haiku"` | model id (provider-specific format) |
| `api_key` | `""` | overridden by provider env var (see below) |
| `api_base` | `"https://openrouter.ai/api/v1/chat/completions"` | endpoint |
| `http_referer` | repo URL | sent on OpenRouter requests for analytics |
| `http_app_title` | `"ARIA Crypto Scalper"` | sent on OpenRouter requests |
| `timeout_secs` | `8` | LLM request timeout — on timeout, fall back to TA-only |
| `min_confidence` | `70` | Floor that the LLM's confidence must clear |
| `fallback_ta_threshold` | `75` | When LLM is offline, TA-only mode requires this confidence |
| `max_tokens` | `1024` | Response cap |

Env-var resolution:

| `provider` | Reads env var |
|---|---|
| `openrouter` | `OPENROUTER_API_KEY` (falls back to `ANTHROPIC_API_KEY`, then `OPENAI_API_KEY`) |
| `anthropic` | `ANTHROPIC_API_KEY` |
| `openai` | `OPENAI_API_KEY` |
| `together` | `TOGETHER_API_KEY` |
| `groq` | `GROQ_API_KEY` |

---

## `[manager]` — TraderManager LLM

The "head of desk" LLM that approves/vetoes/adjusts every Brain
decision. **Disabled by default** — when disabled, every Brain `Go` is
auto-approved. See [docs/ARCHITECTURE.md](ARCHITECTURE.md) §3.6.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `false` | Master switch |
| `provider` | `"openrouter"` | same options as `[llm]` |
| `api_base` | `"https://openrouter.ai/api/v1/chat/completions"` | endpoint |
| `model` | `"anthropic/claude-3.5-haiku"` | model id |
| `timeout_secs` | `6` | request timeout — on error, **fails CLOSED with Veto** |
| `max_tokens` | `600` | response cap |
| `fast_approve_min_conf` | `90` | skip the manager LLM call when brain conf ≥ this AND no lessons matched AND survival is Healthy |
| `http_referer` | repo URL | analytics |
| `http_app_title` | `"ARIA TraderManager"` | analytics |

Env var: `MANAGER_API_KEY` (falls back to the brain LLM key if unset).

---

## `[risk]`

| Key | Default | Meaning |
|---|---|---|
| `risk_per_trade_pct` | `0.35` | percentage of equity to risk per trade *before* survival multiplier |
| `max_open_positions` | `2` | hard cap on open positions |
| `max_daily_loss_pct` | `2.0` | trip the circuit if today's PnL goes below this % |
| `max_drawdown_pct` | `6.0` | trip the circuit if drawdown from peak crosses this |
| `max_leverage` | `3` | passed to `Exchange::set_leverage()` at startup |
| `max_spread_pct` | `0.025` | reject signals when bid/ask spread exceeds this |
| `min_reward_risk` | `1.20` | reject entries whose TP/SL reward:risk is below this |
| `max_position_notional_pct` | `35.0` | cap position notional as % of equity before leverage |
| `min_net_edge_bps` | `1.0` | reject entries whose TP edge is not positive after round-trip TCM cost |
| `assumed_daily_volume_usd` | `1000000000.0` | liquidity assumption for market-impact cost estimates |
| `equity_usd` | `5000.0` | seed equity (paper mode) / starting estimate (live, immediately reconciled) |

In `live` mode, `equity_usd` is **only the seed value**.
`SurvivalAgent` calls `Exchange::fetch_equity_usd()` every 60 s and
overwrites `RiskManager.equity()` with broker truth.

---

## `[schedule]`

| Key | Default | Meaning |
|---|---|---|
| `dead_zone_start_hour_wib` | `3` | start hour (WIB = UTC+7) of the no-trade window |
| `dead_zone_end_hour_wib` | `7` | end hour (exclusive) |

`SignalAgent` skips processing during the dead zone. Wrap-around is
supported — set `start=22, end=2` for a 22:00–02:00 WIB block.

To **disable** the dead zone, set `start == end` (e.g. both 0).

---

## `[advanced_alpha]`

| Key | Default | Meaning |
|---|---:|---|
| `enabled` | `false` | master switch; default is a no-op so paper/live behavior stays unchanged |
| `min_abs_score` | `0.20` | alpha gate threshold for allow/block; values inside the band reduce confidence |
| `reduce_confidence_delta` | `5` | TA-confidence reduction when context is inconclusive |
| `kalman_process_noise` | `0.01` | Kalman trend process noise |
| `kalman_measurement_noise` | `1.0` | Kalman trend measurement noise |

When enabled, `SignalAgent` uses latest `FeedsSnapshot` plus Kalman trend
context to block adverse candidates or reduce their TA confidence before
the risk/LLM pipeline. It does not directly size orders.

---

## `[feeds]`

| Key | Default | Meaning |
|---|---|---|
| `cryptopanic_api_key` | `""` | env `CRYPTOPANIC_API_KEY` |
| `lunarcrush_api_key` | `""` | env `LUNARCRUSH_API_KEY` |
| `glassnode_api_key` | `""` | env `GLASSNODE_API_KEY` |
| `whalealert_api_key` | `""` | env `WHALE_ALERT_API_KEY` |
| `deribit_base_url` | `"https://www.deribit.com"` | public Deribit endpoint for BTC/ETH options IV skew |
| `rss_feeds` | CoinDesk, Decrypt, CoinTelegraph | RSS news sources (no key needed) |

Missing keys → that sub-feed silently emits an empty snapshot. Nothing
crashes. Deribit options data uses public endpoints; unsupported symbols
emit no options snapshot.

---

## `[monitoring]`

| Key | Default | Meaning |
|---|---|---|
| `telegram_bot_token` | `""` | env `TELEGRAM_BOT_TOKEN` |
| `telegram_chat_id` | `""` | env `TELEGRAM_CHAT_ID` |
| `log_level` | `"info"` | tracing default level (overridden by `RUST_LOG` env) |
| `db_path` | `"trades.db"` | SQLite journal path |
| `metrics_bind` | `"0.0.0.0:9184"` | HTTP dashboard bind address |

---

## `[backtest]`

| Key | Default | Meaning |
|---|---|---|
| `data_dir` | `"data/historical"` | folder containing `<SYMBOL>.csv` files |
| `from_ts` | `""` | optional ISO start time |
| `to_ts` | `""` | optional ISO end time |
| `fee_bps` | `4.0` | taker fee charged on entry and exit |
| `slippage_bps` | `2.0` | base adverse fill slippage |
| `market_impact_bps` | `1.0` | impact coefficient scaled by participation rate |
| `trading_days_per_year` | `365.0` | annualization basis for Sharpe/Sortino |
| `trades_per_day` | `12.0` | expected trade opportunities per day for annualization |

Run with `[mode] run_mode = "backtest"`. Backtest runs synchronously
and exits — no agents are spawned.

---

## `[survival]` — survival mode

Full reference: **[docs/SURVIVAL.md](SURVIVAL.md)**.

| Key | Default | Meaning |
|---|---|---|
| `enabled` | `true` | master switch |
| `death_line_pct` | `0.70` | `death_line = initial_equity * pct` |
| `loss_streak_short` | `3` | consecutive losses → 30-min cooldown |
| `loss_streak_short_cooldown_min` | `30` | duration |
| `loss_streak_long` | `5` | losses-in-window for long cooldown |
| `loss_streak_long_window_min` | `60` | rolling window |
| `loss_streak_long_cooldown_min` | `240` | duration |
| `daily_loss_count` | `10` | losses-in-day for 24h cooldown |
| `auto_flat_drawdown_pct` | `8.0` | drawdown that triggers flat-all |
| `refresh_secs` | `15` | how often `SurvivalState` is recomputed |
| `equity_refresh_secs` | `60` | how often equity is reconciled with broker |
| `vol_spike_mult` | `2.0` | ATR ratio above which sizes halve |
| `news_panic_threshold` | `-0.6` | news score for panic blackout |
| `news_euphoria_threshold` | `0.8` | news score for euphoria half-size |
| `daily_pnl_ratchet_pct` | `2.0` | daily gain % that triggers ratchet lock |

---

## `[control]` — operator control panel

Full reference: **[docs/CONTROL.md](CONTROL.md)**.

| Key | Default | Meaning |
|---|---|---|
| `telegram_commands_enabled` | `false` | enable Telegram bot command panel |
| `allowed_user_ids` | `[]` | numeric Telegram user ids allowed to issue commands |
| `poll_secs` | `3` | Telegram getUpdates long-poll interval |

The file ingress at `/tmp/aria.control` is **always on** and unaffected
by `[control]` settings.

---

## Environment variables — full list

| Variable | Used for |
|---|---|
| `BINANCE_API_KEY` | exchange auth |
| `BINANCE_API_SECRET` | exchange auth |
| `OPENROUTER_API_KEY` | brain LLM (default) |
| `ANTHROPIC_API_KEY` | brain LLM (when `[llm.provider] = "anthropic"`) |
| `OPENAI_API_KEY` | brain LLM (when `[llm.provider] = "openai"`) |
| `TOGETHER_API_KEY` | brain LLM (when `[llm.provider] = "together"`) |
| `GROQ_API_KEY` | brain LLM (when `[llm.provider] = "groq"`) |
| `MANAGER_API_KEY` | manager LLM (falls back to brain key) |
| `CRYPTOPANIC_API_KEY` | news feed |
| `LUNARCRUSH_API_KEY` | social sentiment |
| `GLASSNODE_API_KEY` | on-chain |
| `WHALE_ALERT_API_KEY` | on-chain |
| `TELEGRAM_BOT_TOKEN` | alerts + commands |
| `TELEGRAM_CHAT_ID` | alert destination |
| `ARIA_CONFIG_OVERLAY` | path to overlay TOML |
| `ARIA_LLM_PROVIDER` | overrides `[llm] provider` |
| `ARIA_LLM_MODEL` | overrides `[llm] model` |
| `ARIA_LLM_API_BASE` | overrides `[llm] api_base` |
| `ARIA_MANAGER_ENABLED` | overrides `[manager] enabled` (`true` / `false`) |
| `ARIA_MANAGER_PROVIDER` | overrides `[manager] provider` |
| `ARIA_MANAGER_MODEL` | overrides `[manager] model` |
| `ARIA_MANAGER_API_BASE` | overrides `[manager] api_base` |
| `RUST_LOG` | tracing filter (e.g. `info,crypto_scalper::agents::survival=debug`) |

> **Tip — quick model swap without editing config:**
> ```bash
> export ARIA_LLM_MODEL=deepseek/deepseek-chat
> export ARIA_MANAGER_MODEL=anthropic/claude-3.5-sonnet
> aria
> ```

---

## Sample overlays

### Live production with manager + Telegram commands

```toml
# config/production.toml
[mode]
run_mode = "live"
dry_run  = false

[risk]
risk_per_trade_pct = 0.5
max_open_positions = 2
max_daily_loss_pct = 2.0
max_drawdown_pct   = 7.0
max_leverage       = 3
max_spread_pct     = 0.02
equity_usd         = 1000.0

[manager]
enabled = true
model   = "anthropic/claude-3.5-haiku"
fast_approve_min_conf = 92

[control]
telegram_commands_enabled = true
allowed_user_ids = [123456789]

[survival]
death_line_pct = 0.75       # tighter — die at 25% drawdown instead of 30%
auto_flat_drawdown_pct = 6.0
```

Run:

```bash
ARIA_CONFIG_OVERLAY=config/production.toml ./target/release/aria
```

### Cheap LLM for paper testing

```toml
# config/paper-cheap.toml
[mode]
run_mode = "paper"
dry_run  = true

[llm]
model = "google/gemini-2.0-flash-exp:free"

[manager]
enabled = false

[survival]
enabled = false             # turn off survival in pure-strategy testing
```
