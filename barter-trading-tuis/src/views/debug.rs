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
