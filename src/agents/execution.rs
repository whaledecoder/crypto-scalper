//! Execution agent — listens for `ManagerVerdictEmitted` events,
//! applies any size/SL/TP adjustments, dispatches the order, and
//! publishes `OrderFilled` plus `PositionClosed` events.

use crate::agents::messages::{AgentEvent, ManagerAction, ManagerProposal, ManagerVerdict};
use crate::agents::MessageBus;
use crate::data::Side;
use crate::execution::{
    orders::OrderType, Exchange, OrderRequest, Position, PositionBook, RiskManager,
};
use chrono::Utc;
use std::sync::Arc;
use tokio::task::JoinHandle;
use tracing::{info, warn};

pub fn spawn(
    bus: MessageBus,
    exchange: Arc<dyn Exchange>,
    risk: Arc<RiskManager>,
    book: Arc<PositionBook>,
) -> JoinHandle<()> {
    let mut rx = bus.subscribe();
    let bus_for_close = bus.clone();
    tokio::spawn(async move {
        info!("execution agent starting");
        while let Ok(ev) = rx.recv().await {
            match ev {
                AgentEvent::Tick { symbol, trade } => {
                    // Mark-price exit checks happen here so we own the
                    // bus emission when a position closes.
                    let exits = book.check_exits(&symbol, trade.price);
                    for (pos, reason) in exits {
                        let pnl = crate::execution::position::pnl_usd(&pos, trade.price);
                        risk.on_position_closed(pnl);
                        bus_for_close.publish(AgentEvent::PositionClosed {
                            client_id: pos.client_id.clone(),
                            symbol: pos.symbol.clone(),
                            side: pos.side,
                            size: pos.size,
                            entry_price: pos.entry_price,
                            exit_price: trade.price,
                            pnl_usd: pnl,
                            reason,
                        });
                    }
                }
                AgentEvent::ManagerVerdictEmitted(v) => {
                    if matches!(v.action, ManagerAction::Veto { .. }) {
                        info!(
                            symbol = %v.proposal.symbol,
                            reason = %extract_reason(&v.action),
                            "execution: manager vetoed"
                        );
                        continue;
                    }
                    let req = build_order_request(&v);
                    match exchange.place_order(&req).await {
                        Ok(ack) => {
                            risk.on_position_opened();
                            let pos = Position {
                                client_id: req.client_id.clone(),
                                symbol: req.symbol.clone(),
                                side: req.side,
                                size: req.size,
                                entry_price: ack.avg_fill_price.max(req.price.unwrap_or(0.0)),
                                stop_loss: req.stop_loss,
                                take_profit: req.take_profit,
                                opened_at: Utc::now(),
                                trailing_activated: false,
                                peak_price: req.price.unwrap_or(ack.avg_fill_price),
                                trough_price: req.price.unwrap_or(ack.avg_fill_price),
                            };
                            book.open(pos.clone());
                            bus.publish(AgentEvent::OrderFilled {
                                client_id: req.client_id,
                                symbol: req.symbol,
                                side: req.side,
                                size: req.size,
                                ack,
                            });
                        }
                        Err(e) => warn!(error = %e, "execution: place_order failed"),
                    }
                }
                AgentEvent::Shutdown => break,
                _ => {}
            }
        }
    })
}

fn extract_reason(a: &ManagerAction) -> String {
    match a {
        ManagerAction::Veto { reason } => reason.clone(),
        ManagerAction::Adjust { reason, .. } => reason.clone(),
        ManagerAction::Approve => String::new(),
    }
}

fn build_order_request(v: &ManagerVerdict) -> OrderRequest {
    let p: &ManagerProposal = &v.proposal;
    let (size, sl, tp) = match &v.action {
        ManagerAction::Approve | ManagerAction::Veto { .. } => (p.size, p.stop_loss, p.take_profit),
        ManagerAction::Adjust {
            size_multiplier,
            sl_offset_bps,
            tp_offset_bps,
            ..
        } => {
            let size = p.size * size_multiplier;
            let sl_adj = bps_offset(p.entry, *sl_offset_bps, p.side, true);
            let tp_adj = bps_offset(p.entry, *tp_offset_bps, p.side, false);
            (size, p.stop_loss + sl_adj, p.take_profit + tp_adj)
        }
    };
    OrderRequest {
        client_id: format!("aria-{}-{}", p.symbol, Utc::now().timestamp_millis()),
        symbol: p.symbol.clone(),
        side: p.side,
        size,
        price: Some(p.entry),
        stop_loss: sl,
        take_profit: tp,
        order_type: OrderType::Market,
    }
}

fn bps_offset(entry: f64, bps: f64, side: Side, _is_sl: bool) -> f64 {
    // bps relative to entry price; sign convention left to the LLM.
    let raw = entry * (bps / 10_000.0);
    match side {
        Side::Long => raw,
        Side::Short => -raw,
    }
}
