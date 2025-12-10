//! Ratatui widget for TRAD MARKETS panel - Clean card-based layout

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::feed::IbkrConnectionStatus;
use super::state::CorrelationSignals;

// Colors - lighter for better readability
const C_BUY: Color = Color::Rgb(100, 220, 100);
const C_SELL: Color = Color::Rgb(220, 100, 100);
const C_NEUTRAL: Color = Color::Rgb(180, 180, 100);
const C_DIM: Color = Color::Rgb(100, 100, 100);
const C_TEXT: Color = Color::Rgb(180, 180, 180);      // Default text - light gray
const C_BRIGHT: Color = Color::Rgb(220, 220, 220);
const C_ACCENT: Color = Color::Rgb(100, 180, 220);

/// Render the TRAD MARKETS panel - clean card-based layout
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
            Line::from(Span::styled("Waiting for ibkr-bridge...", Style::default().fg(C_TEXT))),
        ];
        f.render_widget(Paragraph::new(placeholder), inner);
        return;
    }

    let mut lines = Vec::new();

    // === ROW 1: ES and NQ prices on same line ===
    let es_color = if signals.es_return >= 0.0 { C_BUY } else { C_SELL };
    let es_arrow = if signals.es_return >= 0.0 { "▲" } else { "▼" };
    let nq_color = if signals.nq_return >= 0.0 { C_BUY } else { C_SELL };
    let nq_arrow = if signals.nq_return >= 0.0 { "▲" } else { "▼" };

    lines.push(Line::from(vec![
        Span::styled("ES  ", Style::default().fg(C_TEXT)),
        Span::styled(format!("{:.2}", signals.es_price), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}{:.2}%", es_arrow, (signals.es_return * 100.0).abs()), Style::default().fg(es_color)),
        Span::styled("   NQ  ", Style::default().fg(C_TEXT)),
        Span::styled(format!("{:.2}", signals.nq_price), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}{:.2}%", nq_arrow, (signals.nq_return * 100.0).abs()), Style::default().fg(nq_color)),
    ]));

    lines.push(Line::from(""));

    // === ROW 2-3: 60s comparisons ===
    let btc_es_pct = signals.btc_es_spread * 100.0;
    let (es_btc_sym, es_btc_color) = if btc_es_pct < -0.05 {
        (">", C_BUY)   // ES winning
    } else if btc_es_pct > 0.05 {
        ("<", C_SELL)  // BTC winning
    } else {
        ("=", C_TEXT)  // SYNC
    };

    lines.push(Line::from(vec![
        Span::styled("60s: ", Style::default().fg(C_TEXT)),
        Span::styled(format!("ES {} BTC  ", es_btc_sym), Style::default().fg(es_btc_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:+.2}%", btc_es_pct.abs()), Style::default().fg(es_btc_color)),
    ]));

    let nq_es_pct = signals.nq_es_spread * 100.0;
    let (es_nq_sym, es_nq_color) = if nq_es_pct < -0.05 {
        (">", C_NEUTRAL)  // ES winning
    } else if nq_es_pct > 0.05 {
        ("<", C_NEUTRAL)  // NQ winning
    } else {
        ("=", C_TEXT)     // SYNC
    };
    let sync_label = if nq_es_pct.abs() < 0.05 { "  (SYNC)" } else { "" };

    lines.push(Line::from(vec![
        Span::styled("     ", Style::default().fg(C_TEXT)),
        Span::styled(format!("ES {} NQ   ", es_nq_sym), Style::default().fg(es_nq_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:+.2}%", nq_es_pct.abs()), Style::default().fg(es_nq_color)),
        Span::styled(sync_label, Style::default().fg(C_TEXT)),
    ]));

    lines.push(Line::from(""));

    // === ROW 4: Three cards - ES/NQ, ES/BTC, LEAD ===
    let corr_es_nq = signals.es_nq_corr.unwrap_or(0.0);
    let corr_es_btc = signals.es_btc_corr.unwrap_or(0.0);

    // Correlation labels and colors
    let (es_nq_label, es_nq_color) = if corr_es_nq > 0.70 {
        ("SYNC", C_BUY)
    } else if corr_es_nq > 0.40 {
        ("weak", C_TEXT)
    } else {
        ("noise", C_TEXT)
    };

    let (es_btc_label, es_btc_color) = if corr_es_btc > 0.50 {
        ("SYNC", C_BUY)
    } else if corr_es_btc > 0.25 {
        ("weak", C_TEXT)
    } else {
        ("noise", C_TEXT)
    };

    // Values - use light gray for readability, green only for strong correlation
    let es_nq_val_color = if corr_es_nq > 0.70 { C_BUY } else { C_BRIGHT };
    let es_btc_val_color = if corr_es_btc > 0.50 { C_BUY } else { C_BRIGHT };

    // Card values
    let es_nq_val = signals.es_nq_corr.map(|c| format!("{:.2}", c)).unwrap_or("--".to_string());
    let es_btc_val = signals.es_btc_corr.map(|c| format!("{:.2}", c)).unwrap_or("--".to_string());
    let lead_val = if signals.lead_lag_secs > 0 {
        "ES".to_string()
    } else if signals.lead_lag_secs < 0 {
        "BTC".to_string()
    } else {
        "SYNC".to_string()
    };
    let lead_time = if signals.lead_lag_secs != 0 {
        format!("+{}s", signals.lead_lag_secs.abs())
    } else {
        "".to_string()
    };

    // All boxes same width: 10 chars inner content
    // Box structure: │ + 10 chars + │ = 12 chars total per box
    // 3 boxes + 2 gaps of 2 spaces = 12+2+12+2+12 = 40 chars total

    // Card top borders (10 dashes = 10 inner width)
    lines.push(Line::from(vec![
        Span::styled("┌──────────┐  ", Style::default().fg(C_DIM)),
        Span::styled("┌──────────┐  ", Style::default().fg(C_DIM)),
        Span::styled("┌──────────┐", Style::default().fg(C_DIM)),
    ]));

    // Card titles - centered in 10 char width
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", "ES/NQ"), Style::default().fg(C_TEXT)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", "ES/BTC"), Style::default().fg(C_TEXT)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", "LEAD"), Style::default().fg(C_TEXT)),
        Span::styled("│", Style::default().fg(C_DIM)),
    ]));

    // Card values - centered in 10 char width
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", es_nq_val), Style::default().fg(es_nq_val_color).add_modifier(Modifier::BOLD)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", es_btc_val), Style::default().fg(es_btc_val_color).add_modifier(Modifier::BOLD)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", lead_val), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled("│", Style::default().fg(C_DIM)),
    ]));

    // Card labels - centered in 10 char width
    lines.push(Line::from(vec![
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", es_nq_label), Style::default().fg(es_nq_color)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", es_btc_label), Style::default().fg(es_btc_color)),
        Span::styled("│  ", Style::default().fg(C_DIM)),
        Span::styled("│", Style::default().fg(C_DIM)),
        Span::styled(format!("{:^10}", lead_time), Style::default().fg(C_ACCENT)),
        Span::styled("│", Style::default().fg(C_DIM)),
    ]));

    // Card bottom borders
    lines.push(Line::from(vec![
        Span::styled("└──────────┘  ", Style::default().fg(C_DIM)),
        Span::styled("└──────────┘  ", Style::default().fg(C_DIM)),
        Span::styled("└──────────┘", Style::default().fg(C_DIM)),
    ]));

    lines.push(Line::from(""));

    // === ROW 5: DIVERGENCE with gradient bar ===
    let div_z = signals.divergence_z.unwrap_or(0.0);

    lines.push(Line::from(vec![
        Span::styled("DIVERGENCE", Style::default().fg(C_TEXT)),
    ]));

    // Divergence gauge - match box width (3 boxes * 12 + 2 gaps * 2 = 40, minus labels)
    // Total 40 chars: "-2σ " (4) + bar (32) + " +2σ" (4) = 40
    let bar_width = 32;
    let div_spans = render_divergence_gauge_colored(div_z, bar_width);

    let mut gauge_line = vec![Span::styled("-2σ ", Style::default().fg(C_TEXT))];
    gauge_line.extend(div_spans);
    gauge_line.push(Span::styled(" +2σ", Style::default().fg(C_TEXT)));
    lines.push(Line::from(gauge_line));

    lines.push(Line::from(""));

    // === ROW 6: Action signal with interpretation ===
    let (signal_icon, signal_text, action_text) = match signals.divergence_z {
        Some(z) if z < -1.5 => ("▲", format!("{:+.1}σ BTC < ES", z), "→ WAIT"),
        Some(z) if z > 1.5 => ("▲", format!("{:+.1}σ BTC > ES", z), "→ WAIT"),
        Some(z) => ("○", format!("{:+.1}σ NEUTRAL", z), ""),
        None => ("○", "WARMING...".to_string(), ""),
    };

    // Signal color based on strength
    let signal_color = match signals.divergence_z {
        Some(z) if z.abs() > 1.5 => C_NEUTRAL,
        _ => C_TEXT,
    };

    lines.push(Line::from(vec![
        Span::styled(signal_icon, Style::default().fg(signal_color)),
        Span::styled(" ", Style::default()),
        Span::styled(&signal_text, Style::default().fg(signal_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}", action_text), Style::default().fg(C_TEXT)),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
}

/// Render divergence gauge with color gradient (red on left, green on right)
fn render_divergence_gauge_colored(z: f64, width: usize) -> Vec<Span<'static>> {
    if width < 5 {
        return vec![];
    }

    let normalized = ((z / 2.0) + 1.0) / 2.0; // Map -2σ..+2σ to 0..1
    let position = (normalized * width as f64).clamp(0.0, (width - 1) as f64) as usize;
    let center = width / 2;

    let mut spans = Vec::new();

    for i in 0..width {
        let ch = if i == position { "●" } else { "─" };

        // Color gradient: red on left, yellow in center, green on right
        let color = if i < center / 2 {
            C_SELL  // Strong red (far left)
        } else if i < center {
            Color::Rgb(200, 150, 100)  // Orange-ish (left of center)
        } else if i == center {
            C_NEUTRAL  // Yellow (center)
        } else if i < center + center / 2 {
            Color::Rgb(150, 200, 100)  // Yellow-green (right of center)
        } else {
            C_BUY  // Strong green (far right)
        };

        // Make the marker brighter
        let style = if i == position {
            Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(color)
        };

        spans.push(Span::styled(ch.to_string(), style));
    }

    spans
}
