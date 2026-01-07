//! Execution view - focused on trade-ready context.

use crate::views::{format_optional_price, format_price, render_header, resolve_display_price, ActiveView, ViewContext};
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

    render_header(f, chunks[0], ctx, ActiveView::Execution);
    render_body(f, chunks[1], ctx);
    render_footer(f, chunks[2], ctx);
}

fn render_body(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let columns = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_execution_context(f, columns[0], ctx);
    render_options_levels(f, columns[1], ctx);
}

fn render_execution_context(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let ticker = ctx.focused_ticker;
    let snapshot = ctx.snapshot.tickers.get(ticker);
    let price = resolve_display_price(snapshot);
    let best_bid = snapshot.and_then(|t| t.best_bid).map(|(p, _)| p);
    let best_ask = snapshot.and_then(|t| t.best_ask).map(|(p, _)| p);

    let mut lines = Vec::new();
    if let Some(result) = ctx.state {
        lines.push(Line::from(format!(
            "STATE: {:?} | BIAS: {:?} | CONF: {}%",
            result.state.state, result.state.bias, result.state.confidence
        )));
        lines.push(Line::from(format!("SPOT: ${}", format_price(price))));
        lines.push(Line::from(format!(
            "BEST BID: {} | BEST ASK: {}",
            format_optional_price(best_bid),
            format_optional_price(best_ask)
        )));
        lines.push(Line::from(format!("REASON: {}", result.state.reason)));
    } else {
        lines.push(Line::from("STATE: --"));
        lines.push(Line::from(format!("SPOT: ${}", format_price(price))));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" EXECUTION CONTEXT ");
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

fn render_options_levels(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let mut lines = Vec::new();
    if let Some(result) = ctx.state {
        if let Some(options) = result.state.components.options_context.as_ref() {
            lines.push(Line::from(format!(
                "PUT WALL: {}",
                format_optional_price(options.put_wall.as_ref().map(|w| w.strike))
            )));
            lines.push(Line::from(format!(
                "CALL WALL: {}",
                format_optional_price(options.call_wall.as_ref().map(|w| w.strike))
            )));
            lines.push(Line::from(format!(
                "MAX PAIN: {}",
                format_optional_price(Some(options.max_pain))
            )));
            lines.push(Line::from(format!(
                "EXPIRY: {:.1}h",
                options.hours_to_expiry
            )));
        } else {
            lines.push(Line::from("OPTIONS LEVELS: unavailable"));
        }
    } else {
        lines.push(Line::from("OPTIONS LEVELS: unavailable"));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" OPTIONS LEVELS ");
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }).block(block), area);
}

fn render_footer(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let line = if let Some(result) = ctx.state {
        format!("READY: {:?}", result.state.state)
    } else {
        "READY: --".to_string()
    };
    let block = Block::default().borders(Borders::ALL);
    f.render_widget(Paragraph::new(line).block(block), area);
}
