//! Ratatui widget for TRAD MARKETS panel - Vertical stacked layout for fast scanning

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::feed::IbkrConnectionStatus;
use super::state::CorrelationSignals;

// Colors matching scalper_v2 palette
const C_BUY: Color = Color::Rgb(100, 220, 100);
const C_SELL: Color = Color::Rgb(220, 100, 100);
const C_NEUTRAL: Color = Color::Rgb(180, 180, 100);
const C_DIM: Color = Color::Rgb(120, 120, 120);
const C_BRIGHT: Color = Color::Rgb(220, 220, 220);
const C_ACCENT: Color = Color::Rgb(100, 180, 220);

/// Render the TRAD MARKETS panel - vertical stacked for fast 1-2s decisions
pub fn render_trad_markets_panel(
    f: &mut Frame,
    area: Rect,
    signals: &CorrelationSignals,
    ibkr_status: IbkrConnectionStatus,
) {
    let border_color = match ibkr_status {
        IbkrConnectionStatus::Connected => C_ACCENT,
        _ => C_SELL,
    };

    let block = Block::default()
        .title(" TRAD MARKETS (ρ=60s) ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // If disconnected and no data, show placeholder
    if ibkr_status != IbkrConnectionStatus::Connected && signals.es_price <= 0.0 {
        let placeholder = vec![
            Line::from(Span::styled("Waiting for ibkr-bridge...", Style::default().fg(C_DIM))),
            Line::from(Span::styled("python -m ibkr_bridge --dry-run", Style::default().fg(C_DIM))),
        ];
        f.render_widget(Paragraph::new(placeholder), inner);
        return;
    }

    // Calculate bar width (use most of available space)
    let bar_width = (inner.width as usize).saturating_sub(10).max(20);

    let mut lines = Vec::new();

    // === ES PRICE ===
    let es_color = if signals.es_return >= 0.0 { C_BUY } else { C_SELL };
    let es_arrow = if signals.es_return >= 0.0 { "▲" } else { "▼" };
    let es_price_str = if signals.es_price > 0.0 { format!("{:.2}", signals.es_price) } else { "--".to_string() };

    lines.push(Line::from(vec![
        Span::styled("ES  ", Style::default().fg(C_DIM)),
        Span::styled(&es_price_str, Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}{:+.2}%", es_arrow, signals.es_return * 100.0), Style::default().fg(es_color)),
    ]));

    // === NQ PRICE ===
    let nq_color = if signals.nq_return >= 0.0 { C_BUY } else { C_SELL };
    let nq_arrow = if signals.nq_return >= 0.0 { "▲" } else { "▼" };
    let nq_price_str = if signals.nq_price > 0.0 { format!("{:.2}", signals.nq_price) } else { "--".to_string() };

    lines.push(Line::from(vec![
        Span::styled("NQ  ", Style::default().fg(C_DIM)),
        Span::styled(&nq_price_str, Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}{:+.2}%", nq_arrow, signals.nq_return * 100.0), Style::default().fg(nq_color)),
    ]));

    // === BLANK LINE ===
    lines.push(Line::from(""));

    // === BTC/ES SPREAD (label + value + status) ===
    // Color by DIRECTION: negative=red, near-zero=dim, positive=green
    let btc_es_spread_pct = signals.btc_es_spread * 100.0;
    let btc_spread_color = if btc_es_spread_pct < -0.05 { C_SELL }      // Negative = red (lagging)
                           else if btc_es_spread_pct > 0.05 { C_BUY }   // Positive = green (leading)
                           else { C_DIM };                              // Near zero = dim (neutral)
    let btc_spread_label = if btc_es_spread_pct < -0.1 { "LAG" }
                           else if btc_es_spread_pct > 0.1 { "LEAD" }
                           else { "=" };

    lines.push(Line::from(vec![
        Span::styled("BTC/ES  ", Style::default().fg(C_DIM)),
        Span::styled(format!("{:+.2}%", btc_es_spread_pct), Style::default().fg(btc_spread_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", btc_spread_label), Style::default().fg(btc_spread_color)),
    ]));

    // === BTC/ES BAR ===
    let btc_bar = render_spread_bar(btc_es_spread_pct, bar_width);
    lines.push(Line::from(vec![
        Span::styled("-1% ", Style::default().fg(C_DIM)),
        Span::styled(btc_bar, Style::default().fg(btc_spread_color)),
        Span::styled(" +1%", Style::default().fg(C_DIM)),
    ]));

    // === BLANK LINE ===
    lines.push(Line::from(""));

    // === NQ/ES SPREAD (label + value + status) ===
    // Color by DIRECTION: negative=red, near-zero=dim, positive=green
    let nq_es_spread_pct = signals.nq_es_spread * 100.0;
    let nq_spread_color = if nq_es_spread_pct < -0.05 { C_SELL }      // Negative = red
                          else if nq_es_spread_pct > 0.05 { C_BUY }   // Positive = green
                          else { C_DIM };                              // Near zero = dim
    let nq_spread_label = if nq_es_spread_pct.abs() < 0.10 { "SYNC" } else { "MIX" };

    lines.push(Line::from(vec![
        Span::styled("NQ/ES   ", Style::default().fg(C_DIM)),
        Span::styled(format!("{:+.2}%", nq_es_spread_pct), Style::default().fg(nq_spread_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("  {}", nq_spread_label), Style::default().fg(nq_spread_color)),
    ]));

    // === NQ/ES BAR ===
    let nq_bar = render_spread_bar(nq_es_spread_pct, bar_width);
    lines.push(Line::from(vec![
        Span::styled("-1% ", Style::default().fg(C_DIM)),
        Span::styled(nq_bar, Style::default().fg(nq_spread_color)),
        Span::styled(" +1%", Style::default().fg(C_DIM)),
    ]));

    // === BLANK LINE ===
    lines.push(Line::from(""));

    // === DIVERGENCE ===
    // Color by SIGNAL STATE: extreme=red, caution=yellow, normal=dim (no signal)
    let div_z = signals.divergence_z.unwrap_or(0.0);
    let div_color = match signals.divergence_z {
        Some(z) if z.abs() > 1.5 => C_SELL,     // Signal zone = red
        Some(z) if z.abs() > 1.0 => C_NEUTRAL,  // Caution = yellow
        Some(_) => C_DIM,                        // Normal = dim (no signal)
        None => C_DIM,                           // No data = dim
    };

    lines.push(Line::from(vec![
        Span::styled("DIV     ", Style::default().fg(C_DIM)),
        Span::styled(
            signals.divergence_z.map(|z| format!("{:+.1}σ", z)).unwrap_or("--".to_string()),
            Style::default().fg(div_color).add_modifier(Modifier::BOLD)
        ),
        Span::styled(if div_z.abs() > 1.5 { "  ⚠ SIGNAL" } else { "" }, Style::default().fg(C_NEUTRAL)),
    ]));

    // === DIV GAUGE ===
    let div_gauge = render_divergence_gauge(div_z, bar_width);
    lines.push(Line::from(vec![
        Span::styled("-2σ ", Style::default().fg(C_DIM)),
        Span::styled(div_gauge, Style::default().fg(div_color)),
        Span::styled(" +2σ", Style::default().fg(C_DIM)),
    ]));

    // === BLANK LINE ===
    lines.push(Line::from(""));

    // === CORRELATION ===
    let es_btc_color = match signals.es_btc_corr {
        Some(c) if c > 0.50 => C_BUY,
        Some(c) if c > 0.30 => C_NEUTRAL,
        Some(_) => C_SELL,
        None => C_DIM,
    };
    let es_nq_color = match signals.es_nq_corr {
        Some(c) if c > 0.85 => C_BUY,
        Some(c) if c > 0.70 => C_NEUTRAL,
        Some(_) => C_SELL,
        None => C_DIM,
    };

    lines.push(Line::from(vec![
        Span::styled("CORR    ", Style::default().fg(C_DIM)),
        Span::styled("ES/BTC:", Style::default().fg(C_DIM)),
        Span::styled(
            signals.es_btc_corr.map(|c| format!("{:.2}", c)).unwrap_or("--".to_string()),
            Style::default().fg(es_btc_color).add_modifier(Modifier::BOLD)
        ),
        Span::styled("  ES/NQ:", Style::default().fg(C_DIM)),
        Span::styled(
            signals.es_nq_corr.map(|c| format!("{:.2}", c)).unwrap_or("--".to_string()),
            Style::default().fg(es_nq_color).add_modifier(Modifier::BOLD)
        ),
    ]));

    // === LEAD/LAG ===
    let lead_text = if signals.lead_lag_secs > 0 {
        format!("ES→BTC ~{}s", signals.lead_lag_secs)
    } else if signals.lead_lag_secs < 0 {
        format!("BTC→ES ~{}s", -signals.lead_lag_secs)
    } else {
        "SYNC".to_string()
    };

    lines.push(Line::from(vec![
        Span::styled("LEAD    ", Style::default().fg(C_DIM)),
        Span::styled(&lead_text, Style::default().fg(C_ACCENT)),
    ]));

    // === BLANK LINE ===
    lines.push(Line::from(""));

    // === SIGNAL LINE ===
    // Signal is based on divergence z-score + EQ sync
    // If no z-score yet, show neutral (data is still available above)
    let (signal_text, action_color) = match signals.divergence_z {
        Some(z) if z < -1.5 && signals.eq_sync => ("⚡ BTC LAG → LONG BTC", C_BUY),
        Some(z) if z > 1.5 && signals.eq_sync => ("⚡ BTC LEAD → SHORT BTC", C_SELL),
        Some(z) if z.abs() > 1.5 => ("⚠ DIVERGENCE → WAIT", C_NEUTRAL),
        Some(_) => ("○ NEUTRAL", C_DIM),
        None => ("○ DIV WARMING...", C_DIM),  // Only divergence warming, other data shown
    };

    lines.push(Line::from(vec![
        Span::styled(signal_text, Style::default().fg(action_color).add_modifier(Modifier::BOLD)),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
}

/// Render spread bar: deviation from center (-1% to +1%)
/// Bar fills FROM CENTER to the current value position
fn render_spread_bar(spread_pct: f64, width: usize) -> String {
    if width < 5 {
        return "".to_string();
    }

    let normalized = ((spread_pct / 1.0) + 1.0) / 2.0; // Map -1%..+1% to 0..1
    let position = (normalized * width as f64).clamp(0.0, (width - 1) as f64) as usize;
    let center = width / 2;

    let mut bar = String::new();
    for i in 0..width {
        if i == center {
            bar.push('│');
        } else if position < center && i >= position && i < center {
            // Fill from position to center (left side)
            bar.push('█');
        } else if position > center && i > center && i <= position {
            // Fill from center to position (right side)
            bar.push('█');
        } else {
            bar.push('░');
        }
    }
    bar
}

/// Render divergence gauge: position marker on -2σ to +2σ scale
fn render_divergence_gauge(z: f64, width: usize) -> String {
    if width < 5 {
        return "".to_string();
    }

    let normalized = ((z / 2.0) + 1.0) / 2.0; // Map -2σ..+2σ to 0..1
    let position = (normalized * width as f64).clamp(0.0, (width - 1) as f64) as usize;

    let mut gauge = String::new();
    for i in 0..width {
        if i == position {
            gauge.push('●');
        } else {
            gauge.push('═');
        }
    }
    gauge
}
