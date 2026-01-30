//! TUI views for the unified trading terminal.
//! Each view renders into a shared frame with a common context.

pub mod debug;
pub mod execution;
pub mod global_radar;

use crate::shared::orchestrator::OrchestratorResult;
use crate::shared::trad_markets::CorrelationSignals;
use crate::shared::state::{AggregatedSnapshot, TickerSnapshot};
use chrono::Utc;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActiveView {
    GlobalRadar,
    Execution,
    Debug,
}

impl ActiveView {
    pub fn label(self) -> &'static str {
        match self {
            ActiveView::GlobalRadar => "Radar",
            ActiveView::Execution => "Exec",
            ActiveView::Debug => "Debug",
        }
    }
}

pub struct ViewContext<'a> {
    pub snapshot: &'a AggregatedSnapshot,
    pub state: Option<&'a OrchestratorResult>,
    pub focused_ticker: &'a str,
    pub connected: bool,
    pub trad_signals: Option<CorrelationSignals>,
}

pub fn render(f: &mut Frame, area: Rect, view: ActiveView, ctx: &ViewContext<'_>) {
    match view {
        ActiveView::GlobalRadar => global_radar::render(f, area, ctx),
        ActiveView::Execution => execution::render(f, area, ctx),
        ActiveView::Debug => debug::render(f, area, ctx),
    }
}

pub(crate) fn render_header(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>, view: ActiveView) {
    let ticker = ctx.focused_ticker;
    let snapshot = ctx.snapshot.tickers.get(ticker);
    let price = resolve_display_price(snapshot);
    let tabs = format!(
        "[1]{} [2]{} [3]{}",
        ActiveView::GlobalRadar.label(),
        ActiveView::Execution.label(),
        ActiveView::Debug.label()
    );
    let focused = match view {
        ActiveView::GlobalRadar => "[Radar]",
        ActiveView::Execution => "[Exec]",
        ActiveView::Debug => "[Debug]",
    };
    let connected = if ctx.connected { "LIVE" } else { "OFFLINE" };
    let line1 = Line::from(vec![
        Span::styled(
            format!("{ticker} {} ", format_price(price)),
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
        ),
        Span::raw("| "),
        Span::styled(tabs, Style::default().fg(Color::Gray)),
        Span::raw(" "),
        Span::styled(focused, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::raw(" | "),
        Span::styled(
            connected,
            Style::default()
                .fg(if ctx.connected { Color::Green } else { Color::Red })
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    let line2 = build_data_health_line(snapshot, ctx.state);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" TRADING TERMINAL ");
    f.render_widget(Paragraph::new(vec![line1, line2]).block(block), area);
}

fn build_data_health_line(
    snapshot: Option<&TickerSnapshot>,
    state: Option<&OrchestratorResult>,
) -> Line<'static> {
    let mut spans = vec![Span::styled(
        "DATA ",
        Style::default().fg(Color::Gray).add_modifier(Modifier::BOLD),
    )];

    if let Some(snapshot) = snapshot {
        push_exchange_status(&mut spans, "BNC", exchange_age(snapshot, "binance"), 5.0, 15.0);
        push_exchange_status(&mut spans, "BBT", exchange_age(snapshot, "bybit"), 5.0, 15.0);
        push_exchange_status(&mut spans, "OKX", exchange_age(snapshot, "okx"), 5.0, 15.0);
    } else {
        push_exchange_status(&mut spans, "BNC", None, 5.0, 15.0);
        push_exchange_status(&mut spans, "BBT", None, 5.0, 15.0);
        push_exchange_status(&mut spans, "OKX", None, 5.0, 15.0);
    }

    push_exchange_status(
        &mut spans,
        "DERB",
        options_age_secs(state),
        90.0,
        180.0,
    );

    Line::from(spans)
}

fn push_exchange_status(
    spans: &mut Vec<Span<'static>>,
    label: &str,
    age: Option<f64>,
    ok_threshold: f64,
    warn_threshold: f64,
) {
    let (value, color) = age_label(age, ok_threshold, warn_threshold);
    spans.push(Span::styled(
        format!("{label} "),
        Style::default().fg(Color::White),
    ));
    spans.push(Span::styled(
        value,
        Style::default().fg(color).add_modifier(Modifier::BOLD),
    ));
    spans.push(Span::raw("  "));
}

fn exchange_age(snapshot: &TickerSnapshot, needle: &str) -> Option<f64> {
    let needle = needle.to_lowercase();
    snapshot
        .exchange_health
        .iter()
        .filter_map(|(name, age)| {
            if name.to_lowercase().contains(&needle) {
                Some(*age)
            } else {
                None
            }
        })
        .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
}

fn options_age_secs(state: Option<&OrchestratorResult>) -> Option<f64> {
    let summary = state
        .map(|s| &s.state.components.options_context)
        .and_then(|s| s.as_ref())?;
    if summary.last_update <= 0 {
        return None;
    }
    let now = Utc::now().timestamp_millis();
    let age_ms = (now - summary.last_update).max(0) as f64;
    Some(age_ms / 1000.0)
}

fn age_label(age: Option<f64>, ok_threshold: f64, warn_threshold: f64) -> (String, Color) {
    match age {
        Some(value) if value <= ok_threshold => (format!("{:.1}s", value), Color::Green),
        Some(value) if value <= warn_threshold => (format!("{:.1}s", value), Color::Yellow),
        Some(value) => (format!("{:.1}s", value), Color::Red),
        None => ("--".to_string(), Color::DarkGray),
    }
}

pub(crate) fn format_price(value: Option<f64>) -> String {
    match value {
        Some(v) if v > 0.0 => format!("{:.2}", v),
        _ => "--".to_string(),
    }
}

const BINANCE_PRICE_STALE_SECS: f64 = 2.0;

pub(crate) fn resolve_display_price(snapshot: Option<&TickerSnapshot>) -> Option<f64> {
    let snapshot = snapshot?;
    let binance_age = exchange_age(snapshot, "binancefutures")
        .or_else(|| exchange_age(snapshot, "binancefuturesusd"))
        .or_else(|| exchange_age(snapshot, "binance-perp"))
        .or_else(|| exchange_age(snapshot, "binance"));
    let binance_fresh = binance_age
        .map(|age| age <= BINANCE_PRICE_STALE_SECS)
        .unwrap_or(true);

    if binance_fresh {
        snapshot
            .binance_perp_last
            .filter(|&p| p > 0.0)
            .or(snapshot.latest_price)
    } else {
        snapshot.latest_price
    }
}

pub(crate) fn format_compact(value: f64) -> String {
    let abs = value.abs();
    let (scaled, suffix) = if abs >= 1_000_000_000.0 {
        (value / 1_000_000_000.0, "B")
    } else if abs >= 1_000_000.0 {
        (value / 1_000_000.0, "M")
    } else if abs >= 1_000.0 {
        (value / 1_000.0, "K")
    } else {
        (value, "")
    };
    format!("{:+.2}{}", scaled, suffix)
}

pub(crate) fn format_optional_price(value: Option<f64>) -> String {
    match value {
        Some(v) if v > 0.0 => format!("{:.0}", v),
        _ => "--".to_string(),
    }
}
