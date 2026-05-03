//! ControlAgent — operator command surface.
//!
//! Provides three ingress paths:
//!
//! 1. **Telegram bot long-poll** (`/status`, `/positions`, `/freeze`,
//!    `/unfreeze`, `/flat`, `/health`, `/help`).
//! 2. **Terminal stdin** (`status`, `positions`, `freeze`, `unfreeze`,
//!    `flat`, `health`, `help`) when running interactively.
//! 3. **Internal control file** at `/tmp/aria.control` — write a
//!    single line (`freeze`, `flat`, `unfreeze`, `status`) and the
//!    agent picks it up. Useful for headless servers without
//!    Telegram.
//!
//! Commands are translated into typed `ControlCommand` events on the
//! bus; downstream agents (`ExecutionAgent`, `SurvivalAgent`,
//! `MonitorAgent`) act on them.

use crate::agents::messages::{AgentEvent, ControlCommand};
use crate::agents::MessageBus;
use crate::config::ControlCfg;
use crate::execution::{Exchange, PositionBook, RiskManager};
use parking_lot::Mutex;
use reqwest::Client;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{self, AsyncBufReadExt};
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub struct ControlAgentDeps {
    pub bus: MessageBus,
    pub cfg: ControlCfg,
    pub telegram_token: String,
    pub telegram_chat_id: String,
    pub risk: Arc<RiskManager>,
    pub book: Arc<PositionBook>,
    pub exchange: Arc<dyn Exchange>,
    /// Optional path for the file-based ingress. `None` = disable.
    pub control_file: Option<PathBuf>,
}

pub fn spawn(deps: ControlAgentDeps) -> JoinHandle<()> {
    let ControlAgentDeps {
        bus,
        cfg,
        telegram_token,
        telegram_chat_id,
        risk,
        book,
        exchange: _exchange,
        control_file,
    } = deps;

    let allowed: HashSet<i64> = cfg.allowed_user_ids.iter().copied().collect();

    if cfg.telegram_commands_enabled && !telegram_token.is_empty() && !telegram_chat_id.is_empty() {
        let bus_t = bus.clone();
        let risk_t = risk.clone();
        let book_t = book.clone();
        let token = telegram_token.clone();
        let chat_id = telegram_chat_id.clone();
        let poll_secs = cfg.poll_secs.max(1);
        tokio::spawn(async move {
            telegram_loop(bus_t, token, chat_id, allowed, risk_t, book_t, poll_secs).await;
        });
    }

    {
        let bus_s = bus.clone();
        let risk_s = risk.clone();
        let book_s = book.clone();
        tokio::spawn(async move {
            stdin_loop(bus_s, risk_s, book_s).await;
        });
    }

    // File-based control surface.
    if let Some(path) = control_file {
        let bus_f = bus.clone();
        tokio::spawn(async move {
            file_loop(bus_f, path).await;
        });
    }

    // Watchdog → freeze/unfreeze handler. Keeps RiskManager in sync
    // with operator commands routed through the bus.
    let mut rx = bus.subscribe();
    let risk_ev = risk.clone();
    tokio::spawn(async move {
        info!("control agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::ControlCommand(ControlCommand::Freeze { reason }) => {
                    risk_ev.freeze(reason);
                }
                AgentEvent::ControlCommand(ControlCommand::Unfreeze) => {
                    risk_ev.unfreeze();
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}

async fn telegram_loop(
    bus: MessageBus,
    token: String,
    chat_id: String,
    allowed: HashSet<i64>,
    risk: Arc<RiskManager>,
    book: Arc<PositionBook>,
    poll_secs: u64,
) {
    let client = Client::builder()
        .timeout(std::time::Duration::from_secs(poll_secs * 4))
        .build()
        .unwrap_or_default();
    let last_update_id: Arc<Mutex<i64>> = Arc::new(Mutex::new(0));

    loop {
        let offset = *last_update_id.lock() + 1;
        let url = format!(
            "https://api.telegram.org/bot{token}/getUpdates?offset={offset}&timeout={poll_secs}"
        );
        match client.get(&url).send().await {
            Ok(resp) => {
                let body: Value = match resp.json().await {
                    Ok(v) => v,
                    Err(e) => {
                        warn!(error = %e, "telegram getUpdates parse failed");
                        tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
                        continue;
                    }
                };
                let updates = body
                    .get("result")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default();
                for upd in updates {
                    let update_id = upd.get("update_id").and_then(|v| v.as_i64()).unwrap_or(0);
                    if update_id > *last_update_id.lock() {
                        *last_update_id.lock() = update_id;
                    }
                    let msg = upd.get("message").cloned().unwrap_or(Value::Null);
                    let from_id = msg
                        .get("from")
                        .and_then(|f| f.get("id"))
                        .and_then(|i| i.as_i64())
                        .unwrap_or(0);
                    let text = msg
                        .get("text")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    if !allowed.is_empty() && !allowed.contains(&from_id) {
                        send_telegram(
                            &client,
                            &token,
                            &chat_id,
                            &format!("⛔ user {from_id} not allowed"),
                        )
                        .await;
                        continue;
                    }
                    let reply = handle_command(&text, &bus, &risk, &book);
                    if !reply.is_empty() {
                        send_telegram(&client, &token, &chat_id, &reply).await;
                    }
                }
            }
            Err(e) => {
                warn!(error = %e, "telegram getUpdates failed");
                tokio::time::sleep(std::time::Duration::from_secs(poll_secs)).await;
            }
        }
    }
}

async fn send_telegram(client: &Client, token: &str, chat_id: &str, text: &str) {
    let url = format!("https://api.telegram.org/bot{token}/sendMessage");
    let body = serde_json::json!({
        "chat_id": chat_id,
        "text": text,
        "disable_web_page_preview": true,
        "parse_mode": "Markdown",
    });
    if let Err(e) = client.post(&url).json(&body).send().await {
        warn!(error = %e, "telegram send failed");
    }
}

async fn stdin_loop(bus: MessageBus, risk: Arc<RiskManager>, book: Arc<PositionBook>) {
    let mut lines = io::BufReader::new(io::stdin()).lines();
    info!("stdin control ready — type `help`, then press Enter");
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                let reply = handle_command(&line, &bus, &risk, &book);
                if !reply.is_empty() {
                    println!("{reply}");
                    info!(reply = %reply, "control command");
                }
            }
            Ok(None) => break,
            Err(e) => {
                warn!(error = %e, "stdin control read failed");
                break;
            }
        }
    }
}

fn handle_command(
    text: &str,
    bus: &MessageBus,
    risk: &Arc<RiskManager>,
    book: &Arc<PositionBook>,
) -> String {
    let cmd = text.trim().to_lowercase();
    match cmd.as_str() {
        "/status" | "status" => {
            bus.publish(AgentEvent::ControlCommand(ControlCommand::StatusRequest));
            let s = risk.snapshot();
            let positions = book.snapshot().len();
            format!(
                "📊 *ARIA status*\n\
                Equity: ${:.2}\n\
                Peak:   ${:.2}\n\
                Daily PnL: ${:.2} ({:.2}%)\n\
                Drawdown: {:.2}%\n\
                Open positions: {} (book: {})\n\
                Frozen: {}\n\
                Tripped: {}\n\
                Runtime pipeline snapshot is printed in the log as `status requested`.",
                s.equity,
                s.peak_equity,
                s.realized_pnl_today,
                s.daily_loss_pct,
                s.drawdown_pct,
                s.open_positions,
                positions,
                s.frozen,
                s.tripped,
            )
        }
        "/positions" | "positions" => {
            let positions = book.snapshot();
            if positions.is_empty() {
                "(no open positions)".to_string()
            } else {
                let mut out = String::from("*Open positions:*\n");
                for p in positions {
                    out.push_str(&format!(
                        "• {} {} size={:.4} entry={:.2} SL={:.2} TP={:.2}\n",
                        p.symbol,
                        match p.side {
                            crate::data::Side::Long => "LONG",
                            crate::data::Side::Short => "SHORT",
                        },
                        p.size,
                        p.entry_price,
                        p.stop_loss,
                        p.take_profit,
                    ));
                }
                out
            }
        }
        "/freeze" | "freeze" => {
            bus.publish(AgentEvent::ControlCommand(ControlCommand::Freeze {
                reason: "operator command".into(),
            }));
            "🧊 frozen — new entries blocked".to_string()
        }
        "/unfreeze" | "unfreeze" => {
            bus.publish(AgentEvent::ControlCommand(ControlCommand::Unfreeze));
            "✅ unfrozen — trading resumed".to_string()
        }
        "/flat" | "flat" => {
            bus.publish(AgentEvent::ControlCommand(ControlCommand::FlatAll {
                reason: "operator /flat".into(),
            }));
            "🔻 flat-all dispatched".to_string()
        }
        "/health" | "health" => {
            bus.publish(AgentEvent::ControlCommand(ControlCommand::StatusRequest));
            "OK — runtime pipeline snapshot printed in log; use `positions` for open trades."
                .to_string()
        }
        "/help" | "help" | "/start" | "start" => "*ARIA commands:*\n\
             status    - risk + latest pipeline snapshot in logs\n\
             positions - open paper/live positions\n\
             flat      - close every open position and freeze\n\
             freeze    - block new entries\n\
             unfreeze  - resume entries\n\
             health    - publish runtime health snapshot"
            .to_string(),
        _ => String::new(),
    }
}

async fn file_loop(bus: MessageBus, path: PathBuf) {
    let mut last_size: u64 = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let meta = match tokio::fs::metadata(&path).await {
            Ok(m) => m,
            Err(_) => continue,
        };
        if meta.len() == last_size {
            continue;
        }
        let content = match tokio::fs::read_to_string(&path).await {
            Ok(s) => s,
            Err(_) => continue,
        };
        let _ = meta.len(); // read above; fall through.
        for line in content.lines() {
            let cmd = line.trim().to_lowercase();
            match cmd.as_str() {
                "freeze" => bus.publish(AgentEvent::ControlCommand(ControlCommand::Freeze {
                    reason: "control file".into(),
                })),
                "unfreeze" => bus.publish(AgentEvent::ControlCommand(ControlCommand::Unfreeze)),
                "flat" => bus.publish(AgentEvent::ControlCommand(ControlCommand::FlatAll {
                    reason: "control file".into(),
                })),
                "status" | "health" => {
                    bus.publish(AgentEvent::ControlCommand(ControlCommand::StatusRequest))
                }
                _ => {}
            }
        }
        // Truncate the file so we don't replay commands.
        let _ = tokio::fs::write(&path, "").await;
        last_size = 0;
    }
}
