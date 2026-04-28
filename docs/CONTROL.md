# Operator Control Panel

ARIA exposes three independent ways for an operator to inspect and
control the running bot:

1. **Telegram bot commands** (off by default; explicitly opt in)
2. **`/tmp/aria.control` file** (always on)
3. **HTTP dashboard endpoints** (read-only)

Plus an internal **Watchdog** agent that auto-issues control commands
when other agents stop sending heartbeats.

---

## 1. Telegram command panel

### 1.1 Setup

```toml
# config/default.toml or your overlay
[control]
telegram_commands_enabled = true
allowed_user_ids = [123456789]    # YOUR Telegram user id (numeric)
poll_secs = 3
```

```bash
export TELEGRAM_BOT_TOKEN=123:abcdef…
export TELEGRAM_CHAT_ID=-100123456789       # for outbound alerts
```

`allowed_user_ids` is the **numeric** Telegram user id (not username)
of every operator allowed to issue commands. Any message from a
different user is logged and ignored.

> Find your numeric id by sending `/start` to `@userinfobot` on Telegram.

### 1.2 Commands

All commands are also accepted without the leading `/`.

| Command | Effect |
|---|---|
| `/status` | Snapshot — equity, peak, daily PnL, drawdown, open positions, frozen state |
| `/positions` | List of open positions with entry, SL, TP |
| `/freeze` | Manual freeze. `RiskManager` blocks every new entry until unfreeze |
| `/unfreeze` | Resume trading. Cooldowns and survival-mode-driven freezes are unaffected (those auto-release when conditions clear) |
| `/flat` | Close every open position at market. Panic button |
| `/health` | `❤️ OK (see /metrics for details)` |
| `/help` / `/start` | Lists every command |

### 1.3 Sample interaction

```
You: /status
Bot:
*ARIA status*
mode: live (dry_run=false)
equity: $5037.42  (peak $5042.00)
daily_pnl: +$12.40 (+0.25%)
drawdown: 0.09%
positions: 1
frozen: false  tripped: false
survival: Healthy (score 88)

You: /flat
Bot: 🔴 emergency flat-all dispatched (1 position)
```

### 1.4 Security notes

- The bot answers **only** users in `allowed_user_ids`. An empty list
  effectively locks down the command panel — you'll still receive
  outbound alerts to `TELEGRAM_CHAT_ID` but no command will be
  honoured.
- The Telegram bot token has full bot privileges, so treat it like a
  password — keep it in env vars, never in committed config.
- For belt-and-suspenders, use Telegram's bot privacy mode + put the
  bot in a private chat with you (not a group).

---

## 2. File ingress — `/tmp/aria.control`

Always available, regardless of `telegram_commands_enabled`. The
`ControlAgent` polls the file's size every few seconds; on change it
reads new lines, parses commands, and **truncates the file** so
commands don't replay.

```bash
echo flat     >> /tmp/aria.control     # close all positions at market
echo freeze   >> /tmp/aria.control     # block new entries
echo unfreeze >> /tmp/aria.control     # resume
```

This is the primary panic channel for shell-only environments — useful
in cron, in alert hooks (e.g. Prometheus alertmanager → exec a script
that writes `flat`), or when Telegram is unavailable.

> The file path is hard-coded in `main.rs` to `/tmp/aria.control`.
> Change it there if your environment doesn't allow `/tmp` writes.

---

## 3. HTTP endpoints (read-only)

The dashboard server binds to `[monitoring] metrics_bind` (default
`0.0.0.0:9184`). All routes return JSON unless noted.

| Path | Returns |
|---|---|
| `/healthz` | `"ok"` plain text |
| `/metrics` | `MetricsSnapshot` |
| `/lessons` | `Vec<Lesson>` — currently active learning rules |
| `/survival` | latest `SurvivalState` — 404 until the first refresh tick |
| `/dashboard` | `{ metrics, lessons, survival }` combined |

Examples:

```bash
curl -s http://localhost:9184/healthz
# ok

curl -s http://localhost:9184/metrics | jq .
# {
#   "mode": "paper",
#   "equity": 5037.42,
#   "peak_equity": 5042.0,
#   "open_positions": 1,
#   "daily_pnl": 12.4,
#   "trades_today": 14,
#   "signals_today": 64,
#   "llm_go": 32, "llm_nogo": 18, "llm_wait": 14,
#   "llm_avg_confidence": 71.4,
#   "llm_avg_latency_ms": 1832,
#   "llm_offline_fallbacks": 0,
#   "active_lessons": 2,
#   "last_update_ts": 1715690412
# }

curl -s http://localhost:9184/survival | jq .
# {
#   "score": 88,
#   "mode": "Healthy",
#   "equity_usd": 5037.42,
#   "initial_equity_usd": 5000.0,
#   "death_line_usd": 3500.0,
#   "peak_equity_usd": 5042.0,
#   "drawdown_pct": 0.09,
#   "consecutive_losses": 0,
#   "size_multiplier": 1.0,
#   "reasons": [],
#   "ts": "2026-04-28T08:13:42Z"
# }
```

### Wiring into Prometheus / alertmanager

The `/metrics` endpoint is JSON, not Prometheus text format. To scrape
it from Prometheus, run a small JSON exporter sidecar (`json_exporter`)
or curl the endpoint from your own collector. Future versions may add
a native Prometheus text endpoint.

---

## 4. The `Watchdog` agent (automatic control)

`WatchdogAgent` watches every other agent's `Heartbeat` events. The
default config is in `src/agents/watchdog.rs`:

```rust
pub struct WatchdogConfig {
    pub watched: Vec<AgentId>,            // Data, Feeds, Signal, Risk, Brain, Execution
    pub liveness_timeout_secs: u64,       // 90
    pub check_interval_secs: u64,         // 15
}
```

Behaviour:

- Every `check_interval_secs`, it scans `last_seen[agent_id]`.
- If any watched agent has been silent for more than
  `liveness_timeout_secs`, the watchdog publishes
  `ControlCommand::Freeze { reason: "<agent_name> silent for <secs>s" }`.
- When all watched agents are heartbeat-fresh again, it publishes
  `ControlCommand::Unfreeze`.

The `ControlAgent` event loop receives these and calls
`risk.freeze(reason)` / `risk.unfreeze()`. From the trader's
perspective, a stuck WebSocket → automatic safe stop, automatic resume
when the WebSocket recovers.

> The watchdog only freezes — it does **not** flat positions. That
> decision is left to the operator (via `/flat`) or to `SurvivalAgent`
> (death line / extreme drawdown).

---

## 5. `ControlCommand` reference

```rust
pub enum ControlCommand {
    Freeze { reason: String },     // RiskManager rejects new entries
    Unfreeze,                      // resume trading
    FlatAll { reason: String },    // ExecutionAgent closes all positions
    ResetDaily,                    // reset RiskManager daily PnL counters
    StatusRequest,                 // no-op marker (hooks for future expansion)
}
```

Every command flows through the bus, so multiple subscribers can react.
For example, `FlatAll` is heard by both `ExecutionAgent` (closes
positions) and `MonitorAgent` (sends Telegram alert + writes a journal
note).

---

## 6. Operator runbook

### Standard panic ("something feels wrong")

1. `/freeze` (Telegram) or `echo freeze >> /tmp/aria.control`.
2. Check `/dashboard` and `/survival` for the bot's view.
3. If positions look bad: `/flat`.
4. Investigate logs (`journalctl -u aria -n 200`).
5. When ready: `/unfreeze`.

### Survival auto-freeze ("Frozen mode and I want to override")

1. `curl /survival` → check `reasons[]` to understand why.
2. If a cooldown — wait for it to expire, then it auto-clears.
3. If a death-line breach — *do not* unfreeze without recapitalising.
   The bot is telling you the strategy isn't working at this equity.
4. To force-resume after recapitalising, restart the bot — the
   startup `fetch_equity_usd()` will reflect the new equity.

### Watchdog auto-freeze ("WebSocket flapping")

1. `curl /survival` — `reasons` won't show this; check logs for
   `watchdog: agent silent`.
2. Network issue → wait, the watchdog auto-unfreezes when the agent
   resumes heartbeating.
3. Persistent → check Binance status, your firewall, IP whitelist.

---

## 7. Related files

- `src/agents/control.rs` — Telegram + file ingress
- `src/agents/watchdog.rs` — heartbeat dead-man-switch
- `src/agents/messages.rs` — `ControlCommand` enum, `Heartbeat` event
- `src/monitoring/metrics.rs` — HTTP dashboard server
- `src/main.rs` — wiring (see the `_control` and `_watchdog` spawns)
