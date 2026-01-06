//! Debug view - raw state and snapshot signals for diagnostics.

use crate::views::{format_compact, format_price, render_header, ActiveView, ViewContext};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

pub fn render(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Min(0),
            Constraint::Length(3),
        ])
        .split(area);

    render_header(f, chunks[0], ctx, ActiveView::Debug);
    render_body(f, chunks[1], ctx);
    render_footer(f, chunks[2], ctx);
}

fn render_body(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let ticker = ctx.focused_ticker;
    let snapshot = ctx.snapshot.tickers.get(ticker);
    let mut lines = Vec::new();

    if let Some(snap) = snapshot {
        lines.push(Line::from(format!(
            "SPOT: ${}",
            format_price(snap.binance_perp_last)
        )));
        lines.push(Line::from(format!(
            "CVD 5m: {} | CVD 1m: {}",
            format_compact(snap.cvd_5m_total),
            format_compact(snap.cvd_1m_total)
        )));
        lines.push(Line::from(format!(
            "RV 1h: {} | ATR 14: {}",
            snap.realized_vol_1h
                .map(|v| format!("{:.4}%", v * 100.0))
                .unwrap_or_else(|| "--".to_string()),
            snap.atr_14
                .map(|v| format!("{:.2}", v))
                .unwrap_or_else(|| "--".to_string())
        )));
        lines.push(Line::from(format!(
            "FUNDING EXCH: {}",
            snap.funding_rate_by_exchange.len()
        )));
        lines.push(Line::from(format!(
            "EXCHANGE HEALTH: {}",
            snap.exchange_health.len()
        )));

        if let Some(server_snap) = ctx
            .snapshot
            .server_snapshot
            .as_ref()
            .and_then(|s| s.tickers.get(ticker))
        {
            let local_rvol = if snap.vol_1h > 0.0 {
                snap.vol_5m / (snap.vol_1h / 12.0)
            } else {
                0.0
            };
            let local_funding = if snap.funding_rate_by_exchange.is_empty() {
                0.0
            } else {
                snap.funding_rate_by_exchange.values().sum::<f64>()
                    / snap.funding_rate_by_exchange.len() as f64
            };

            lines.push(Line::from("---- SERVER SNAPSHOT ----"));
            lines.push(Line::from(format!(
                "CVD 5m: {} | LOCAL {}",
                format_compact(server_snap.cvd_5m),
                format_compact(snap.cvd_5m_total)
            )));
            lines.push(Line::from(format!(
                "CVD 15m: {} | LOCAL {}",
                format_compact(server_snap.cvd_15m),
                format_compact(snap.cvd_15m_total)
            )));
            lines.push(Line::from(format!(
                "RVOL 5m: {:.2} | LOCAL {:.2}",
                server_snap.rvol_5m, local_rvol
            )));
            lines.push(Line::from(format!(
                "OI Δ5m: {} | LOCAL {}",
                format_compact(server_snap.oi_delta_5m),
                format_compact(snap.oi_delta_5m)
            )));
            lines.push(Line::from(format!(
                "FUND: {:.4}% | LOCAL {:.4}%",
                server_snap.funding_rate * 100.0,
                local_funding * 100.0
            )));
            lines.push(Line::from(format!(
                "LIQ/m: {} | LOCAL {}",
                format_compact(server_snap.liq_rate_usd_per_min),
                format_compact(snap.liq_rate_per_min)
            )));
        }
    } else {
        lines.push(Line::from("No snapshot data"));
    }

    if let Some(result) = ctx.state {
        let vol = &result.state.components.vol_regime;
        let gamma = &result.state.components.gamma_context;
        let flow = &result.state.components.flow_consensus;
        let funding = &result.state.components.funding_context;

        lines.push(Line::from("---- STATE ENGINE ----"));
        lines.push(Line::from(format!(
            "STATE: {:?} | CONF: {}%",
            result.state.state, result.state.confidence
        )));
        lines.push(Line::from(format!(
            "VOL: {:?} pctl {:.1} shock {}",
            vol.regime, vol.percentile, vol.is_shock
        )));
        lines.push(Line::from(format!(
            "GAMMA: flip {} dist {} bias {:?}",
            gamma.gamma_flip_price,
            gamma.distance_pct,
            gamma.bias
        )));
        lines.push(Line::from(format!(
            "FLOW: {}/{} {:?}",
            flow.venues_agreeing, flow.venues_total, flow.consensus_direction
        )));
        lines.push(Line::from(format!(
            "FUNDING: rate {:.4}% vel {:.4}",
            funding.current_rate * 100.0,
            funding.velocity
        )));
    } else {
        lines.push(Line::from("No orchestrator state"));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" DEBUG ");
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

fn render_footer(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let line = if ctx.connected { "CONNECTED" } else { "DISCONNECTED" };
    let block = Block::default().borders(Borders::ALL);
    f.render_widget(Paragraph::new(line).block(block), area);
}
