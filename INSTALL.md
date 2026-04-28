# Installing ARIA

Step-by-step installation for Linux/macOS. Tested on Ubuntu 22.04+ and
macOS 14+. Windows users should run inside WSL2.

---

## 1. Prerequisites

| Tool | Minimum | Used for |
|---|---|---|
| **Rust toolchain** | 1.85 (stable) | Building `aria` |
| **OpenSSL / libssl-dev** | system pkg | TLS for HTTP / WebSocket |
| **pkg-config** | system pkg | Required by some Rust crates |
| **SQLite** | bundled by `rusqlite` | No system install needed |
| **Git** | any | Cloning the repo |
| **`jq`** *(optional)* | any | Pretty-printing the dashboard JSON |

### Ubuntu / Debian

```bash
sudo apt update
sudo apt install -y build-essential pkg-config libssl-dev curl git ca-certificates jq
```

### macOS

```bash
xcode-select --install        # if not already
brew install pkg-config openssl@3 jq
```

### Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
source "$HOME/.cargo/env"
rustup default stable
rustc --version       # must be >= 1.85
```

---

## 2. Clone & Build

```bash
git clone https://github.com/whaledecoder/crypto-scalper.git
cd crypto-scalper
cargo build --release         # ~2-3 min cold; binary at target/release/aria
```

Verify quality gates pass:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings    # 0 warnings
cargo test --lib                                            # 34/34 pass
```

---

## 3. Configure

ARIA reads configuration from three sources, merged in this order:

1. **`config/default.toml`** — repository-tracked defaults (paper mode).
2. **Optional overlay** — `ARIA_CONFIG_OVERLAY=path/to/overlay.toml`.
3. **Environment variables** — override secrets (API keys, Telegram, etc.).

Provided overlays:

| Overlay | What it does |
|---|---|
| `config/paper.toml` | Forces `run_mode=paper`, `dry_run=true` (extra-safe). |
| `config/production.toml` | Tighter risk caps, ready for live mode. |
| `config/llm-anthropic.toml` | Switches the brain LLM to Anthropic native. |
| `config/llm-openrouter-cheap.toml` | Picks cheap or free OpenRouter models. |

Copy the env template:

```bash
cp .env.example .env
$EDITOR .env
```

ARIA **auto-loads `.env`** from the current working directory at
startup (no manual `source` required), so just running `aria` from
the repo directory is enough.

If you prefer to load it into the shell yourself (for example to use
the same vars in other commands), the manual form still works:

```bash
set -a; source .env; set +a
```

> Real `export`-ed environment variables always win over the file —
> so a value set in your shell takes priority over the same key in
> `.env`. Use `ARIA_DOTENV=/some/path/to/.env` to point ARIA at a
> non-default file.

### Required for paper mode

| Variable | Purpose |
|---|---|
| `OPENROUTER_API_KEY` | Default brain LLM. Free key at https://openrouter.ai/keys. *Optional* — without a key, the brain falls back to TA-only mode and the manager auto-vetoes. |

### Required for live mode (in addition)

| Variable | Purpose |
|---|---|
| `BINANCE_API_KEY` | Binance Futures API key |
| `BINANCE_API_SECRET` | Binance Futures API secret |

> The Binance key needs **Futures Trading** enabled and (recommended)
> IP-restricted to your server.

### Optional feeds

| Variable | Purpose |
|---|---|
| `CRYPTOPANIC_API_KEY` | News (free 1k req/day) |
| `LUNARCRUSH_API_KEY` | Social sentiment ($24/mo) |
| `GLASSNODE_API_KEY` | On-chain ($39+/mo) |
| `WHALE_ALERT_API_KEY` | Whale movements ($29+/mo) |

Without these, the corresponding feeds simply emit empty snapshots —
nothing crashes.

### Optional manager LLM

| Variable | Purpose |
|---|---|
| `MANAGER_API_KEY` | Use a *different* model for the manager than the brain. Falls back to brain key if unset. |

### Optional Telegram

| Variable | Purpose |
|---|---|
| `TELEGRAM_BOT_TOKEN` | Bot token (alerts and command panel) |
| `TELEGRAM_CHAT_ID` | Chat to send alerts to |

To enable the **command panel** (so Telegram users can `/freeze` etc),
also flip `[control] telegram_commands_enabled = true` in your overlay
and add your user IDs to `[control] allowed_user_ids`.

---

## 4. First Run (Paper Mode)

```bash
./target/release/aria
```

Or with the explicit paper overlay:

```bash
ARIA_CONFIG_OVERLAY=config/paper.toml ./target/release/aria
```

Expected log lines:

```
INFO starting ARIA mode=paper dry_run=true
INFO brain llm configured provider=openrouter model=anthropic/claude-3.5-haiku key_set=true
INFO metrics server listening bind=0.0.0.0:9184
INFO all agents spawned — runtime live
INFO survival agent: state computed score=100 mode=Healthy
```

In another shell, check the dashboard:

```bash
curl -s http://localhost:9184/dashboard | jq .
curl -s http://localhost:9184/survival  | jq .
curl -s http://localhost:9184/lessons   | jq .
```

Stop with `Ctrl-C` — agents broadcast `Shutdown` and drain cleanly.

---

## 5. Going Live

> **Read this carefully.** Live mode dispatches real orders.

### 5.1 Pre-flight checklist

- [ ] You have run paper mode for at least 24 hours and reviewed
      `trades.db` + `/dashboard` JSON. Trades flow, manager vetoes look
      reasonable, no crashes.
- [ ] Your Binance API key is **IP-restricted** to your server.
- [ ] You set a sane `[risk] equity_usd` matching your actual deposit.
      ARIA seeds in-memory equity from this value at boot, then
      reconciles every 60 s from `fetch_equity_usd()`.
- [ ] You set `[risk] max_leverage` to the leverage you want — ARIA
      will call `set_leverage` on every symbol at startup, overwriting
      any previous setting.
- [ ] `[survival]` settings reviewed (`death_line_pct`,
      `auto_flat_drawdown_pct`, cooldowns) — see
      [docs/SURVIVAL.md](docs/SURVIVAL.md).
- [ ] Telegram alerts working in paper mode (you actually received the
      "ARIA started" message).
- [ ] `/tmp/aria.control` is writable by the user running `aria` —
      this is your panic file.

### 5.2 Switch to live

Either edit `config/default.toml`:

```toml
[mode]
run_mode = "live"
dry_run  = false
```

…or use the production overlay:

```bash
ARIA_CONFIG_OVERLAY=config/production.toml ./target/release/aria
```

### 5.3 First-live boot logs to expect

```
INFO live mode — dispatching real orders to Binance
INFO startup: equity reconciled equity=1234.56
INFO startup: reconciled open positions count=0
INFO survival agent: state computed score=100 mode=Healthy
INFO 🤖 *ARIA started* — multi-agent mode `live` ...
```

If any of those startup steps fail (`set_leverage`,
`fetch_equity_usd`, `fetch_open_positions`), they log a warning **but
do not abort** — you should `Ctrl-C`, fix the issue, and restart.

### 5.4 Run as a service (systemd)

```ini
# /etc/systemd/system/aria.service
[Unit]
Description=ARIA crypto scalper
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=aria
WorkingDirectory=/home/aria/crypto-scalper
EnvironmentFile=/home/aria/crypto-scalper/.env
ExecStart=/home/aria/crypto-scalper/target/release/aria
Restart=on-failure
RestartSec=10
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
```

```bash
sudo systemctl daemon-reload
sudo systemctl enable --now aria
journalctl -u aria -f       # follow logs
```

> Restart safety: when `aria` boots, it calls `fetch_open_positions`
> and rebuilds the `PositionBook` from broker truth. Broker-side
> SL/TP orders remain in place across restarts.

---

## 6. Backtesting

Place historical Binance candle CSVs under `data/historical/`:

```
data/historical/BTCUSDT.csv
data/historical/ETHUSDT.csv
```

Each file must have a header row:

```
open_time_ms,open,high,low,close,volume
```

Then run with `[mode] run_mode = "backtest"` (or use a custom overlay
that sets it). The bot replays every closed candle through the same
signal pipeline and prints a per-symbol report:

```
INFO backtest symbol done symbol=BTCUSDT trades=42 win_rate=58.30% pf=1.42 net=187.30
```

The `run_backtest` path **does not** spawn the agent runtime — it's a
simple linear replay. To validate the multi-agent flow against
historical data, run paper mode against the live exchange instead.

---

## 7. Troubleshooting

| Symptom | Likely cause | Fix |
|---|---|---|
| `cannot bind metrics server` | Port 9184 in use | Change `[monitoring] metrics_bind` |
| Build fails on `openssl-sys` | Missing system OpenSSL | Install `libssl-dev` (Ubuntu) / `openssl@3` (brew) |
| Brain LLM always falls back to TA-only | `OPENROUTER_API_KEY` unset or wrong | Re-export the env var; check `key_set=true` in startup logs |
| Manager keeps vetoing every trade | LLM key invalid OR survival score < 80 | Check `/survival` JSON; check manager LLM credentials |
| `/freeze` Telegram command does nothing | `telegram_commands_enabled=false` or your user id not in `allowed_user_ids` | Update `[control]` and restart |
| Bot crashes on Binance signature error | Clock drift > `recv_window_ms` | `sudo systemctl restart systemd-timesyncd` |
| Bot opens position but no SL on broker | Broker rejected the SL order (size below minNotional) | Check exchange logs; ensure your `[risk] equity_usd` × `risk_per_trade_pct` exceeds Binance's minNotional for the symbol |
| `SurvivalMode = Frozen` and won't unfreeze | Cooldown still active OR ratchet locked | `curl /survival` to see `reasons[]`; or send `/unfreeze` once cooldown expires |
| WebSocket disconnects every few minutes | Network instability | The DataAgent auto-reconnects; if persistent, check IP firewall |

For anything else, check the logs with `RUST_LOG=debug`.

---

## 8. Updating

```bash
cd crypto-scalper
git pull
cargo build --release
sudo systemctl restart aria      # if running under systemd
```

Migrations: the SQLite journal schema is auto-created on boot; future
breaking changes will be called out in the release notes.
