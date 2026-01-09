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
const C_HEADER: Color = Color::Rgb(140, 140, 140);

const PRICE_PCT_THRESHOLD: f64 = 0.0;
const VWAP_POINTS_THRESHOLD: f64 = 1.0;
const FLOW_CONTRACT_THRESHOLD: f64 = 10.0;
const COL_PRICE: usize = 8;
const COL_FLOW: usize = 8;
const COL_VWAP: usize = 6;
const COL_BIAS: usize = 6;
const COL_GAP: &str = " ";
const COL_GAP_LEN: usize = 1;
const SEP_LEN: usize = 4 + COL_PRICE + COL_FLOW + COL_VWAP + COL_BIAS + (COL_GAP_LEN * 3);

/// Render the TRAD MARKETS panel - clean card-based layout
pub fn render_trad_markets_panel(
    f: &mut Frame,
    area: Rect,
    signals: &CorrelationSignals,
    ibkr_status: IbkrConnectionStatus,
) {
    let border_color = match ibkr_status {
        IbkrConnectionStatus::Connected => C_ACCENT,
        IbkrConnectionStatus::Stale | IbkrConnectionStatus::Reconnecting => C_NEUTRAL,
        IbkrConnectionStatus::Disconnected => C_SELL,
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
            Line::from(Span::styled("Waiting for trad feed...", Style::default().fg(C_TEXT))),
        ];
        f.render_widget(Paragraph::new(placeholder), inner);
        return;
    }

    let mut lines = Vec::new();

    let vwap_pct = |price: f64, vwap: Option<f64>| -> Option<f64> {
        vwap.and_then(|v| if v > 0.0 { Some((price - v) / v * 100.0) } else { None })
    };
    let vwap_dev = |price: f64, vwap: Option<f64>| -> Option<f64> {
        vwap.and_then(|v| if v > 0.0 { Some(price - v) } else { None })
    };

    let es_vwap_pct = vwap_pct(signals.es_price, signals.es_vwap);
    let nq_vwap_pct = vwap_pct(signals.nq_price, signals.nq_vwap);
    let es_vwap_dev = vwap_dev(signals.es_price, signals.es_vwap);
    let nq_vwap_dev = vwap_dev(signals.nq_price, signals.nq_vwap);

    // === ROW 1: Price + VWAP deviation % ===
    let es_vwap_pct_str = es_vwap_pct.map(|v| format!("{:+.2}%", v)).unwrap_or_else(|| "--".to_string());
    let nq_vwap_pct_str = nq_vwap_pct.map(|v| format!("{:+.2}%", v)).unwrap_or_else(|| "--".to_string());
    let es_vwap_pct_color = match es_vwap_pct {
        Some(v) if v > 0.0 => C_BUY,
        Some(v) if v < 0.0 => C_SELL,
        Some(_) => C_TEXT,
        None => C_DIM,
    };
    let nq_vwap_pct_color = match nq_vwap_pct {
        Some(v) if v > 0.0 => C_BUY,
        Some(v) if v < 0.0 => C_SELL,
        Some(_) => C_TEXT,
        None => C_DIM,
    };

    lines.push(Line::from(vec![
        Span::styled("ES ", Style::default().fg(C_TEXT)),
        Span::styled(format!("{:.2}", signals.es_price), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}", es_vwap_pct_str), Style::default().fg(es_vwap_pct_color)),
        Span::styled("   NQ ", Style::default().fg(C_TEXT)),
        Span::styled(format!("{:.2}", signals.nq_price), Style::default().fg(C_BRIGHT).add_modifier(Modifier::BOLD)),
        Span::styled(format!(" {}", nq_vwap_pct_str), Style::default().fg(nq_vwap_pct_color)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // === ROW 2: Matrix header ===
    let header_style = Style::default().fg(C_HEADER);
    lines.push(Line::from(vec![
        Span::styled(format!("{:<4}", ""), header_style),
        Span::styled(format!("{:>width$}", "PRICE", width = COL_PRICE), header_style),
        Span::styled(COL_GAP, header_style),
        Span::styled(format!("{:>width$}", "FLOW", width = COL_FLOW), header_style),
        Span::styled(COL_GAP, header_style),
        Span::styled(format!("{:>width$}", "VWAP", width = COL_VWAP), header_style),
        Span::styled(COL_GAP, header_style),
        Span::styled(format!("{:^width$}", "BIAS", width = COL_BIAS), header_style),
    ]));

    // === ROW 3-4: ES/NQ matrix rows ===
    let es_ret_pct = signals.es_return * 100.0;
    let nq_ret_pct = signals.nq_return * 100.0;
    let es_arrow = if es_ret_pct >= 0.0 { "▲" } else { "▼" };
    let nq_arrow = if nq_ret_pct >= 0.0 { "▲" } else { "▼" };
    let es_price_str = format!("{}{:>+6.2}%", es_arrow, es_ret_pct);
    let nq_price_str = format!("{}{:>+6.2}%", nq_arrow, nq_ret_pct);
    let es_flow_str = format!("δ{:+.0}c", signals.es_delta);
    let nq_flow_str = format!("δ{:+.0}c", signals.nq_delta);
    let es_vwap_str = es_vwap_dev.map(|v| format!("{:+.1}", v)).unwrap_or_else(|| "--".to_string());
    let nq_vwap_str = nq_vwap_dev.map(|v| format!("{:+.1}", v)).unwrap_or_else(|| "--".to_string());

    let es_price_color = if es_ret_pct > PRICE_PCT_THRESHOLD {
        C_BUY
    } else if es_ret_pct < -PRICE_PCT_THRESHOLD {
        C_SELL
    } else {
        C_TEXT
    };
    let nq_price_color = if nq_ret_pct > PRICE_PCT_THRESHOLD {
        C_BUY
    } else if nq_ret_pct < -PRICE_PCT_THRESHOLD {
        C_SELL
    } else {
        C_TEXT
    };
    let es_flow_color = if signals.es_delta > FLOW_CONTRACT_THRESHOLD {
        C_BUY
    } else if signals.es_delta < -FLOW_CONTRACT_THRESHOLD {
        C_SELL
    } else {
        C_TEXT
    };
    let nq_flow_color = if signals.nq_delta > FLOW_CONTRACT_THRESHOLD {
        C_BUY
    } else if signals.nq_delta < -FLOW_CONTRACT_THRESHOLD {
        C_SELL
    } else {
        C_TEXT
    };
    let es_vwap_color = match es_vwap_dev {
        Some(v) if v > VWAP_POINTS_THRESHOLD => C_BUY,
        Some(v) if v < -VWAP_POINTS_THRESHOLD => C_SELL,
        Some(_) => C_TEXT,
        None => C_DIM,
    };
    let nq_vwap_color = match nq_vwap_dev {
        Some(v) if v > VWAP_POINTS_THRESHOLD => C_BUY,
        Some(v) if v < -VWAP_POINTS_THRESHOLD => C_SELL,
        Some(_) => C_TEXT,
        None => C_DIM,
    };
    let es_bias = if signals.es_delta > FLOW_CONTRACT_THRESHOLD {
        ("BUY", C_BUY)
    } else if signals.es_delta < -FLOW_CONTRACT_THRESHOLD {
        ("SELL", C_SELL)
    } else {
        ("FLAT", C_TEXT)
    };
    let nq_bias = if signals.nq_delta > FLOW_CONTRACT_THRESHOLD {
        ("BUY", C_BUY)
    } else if signals.nq_delta < -FLOW_CONTRACT_THRESHOLD {
        ("SELL", C_SELL)
    } else {
        ("FLAT", C_TEXT)
    };

    lines.push(Line::from(vec![
        Span::styled(format!("{:<4}", "ES"), Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", es_price_str, width = COL_PRICE), Style::default().fg(es_price_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", es_flow_str, width = COL_FLOW), Style::default().fg(es_flow_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", es_vwap_str, width = COL_VWAP), Style::default().fg(es_vwap_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:^width$}", es_bias.0, width = COL_BIAS), Style::default().fg(es_bias.1).add_modifier(Modifier::BOLD)),
    ]));

    lines.push(Line::from(vec![
        Span::styled(format!("{:<4}", "NQ"), Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", nq_price_str, width = COL_PRICE), Style::default().fg(nq_price_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", nq_flow_str, width = COL_FLOW), Style::default().fg(nq_flow_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", nq_vwap_str, width = COL_VWAP), Style::default().fg(nq_vwap_color)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:^width$}", nq_bias.0, width = COL_BIAS), Style::default().fg(nq_bias.1).add_modifier(Modifier::BOLD)),
    ]));

    lines.push(Line::from(vec![
        Span::styled("─".repeat(SEP_LEN), Style::default().fg(C_DIM)),
    ]));

    // === ROW 6: Combined summary ===
    let summary_price = if es_ret_pct > PRICE_PCT_THRESHOLD {
        ("▲ UP", C_BUY)
    } else if es_ret_pct < -PRICE_PCT_THRESHOLD {
        ("▼ DN", C_SELL)
    } else {
        ("• FLAT", C_TEXT)
    };
    let summary_flow = if signals.es_delta > FLOW_CONTRACT_THRESHOLD {
        ("BID", C_BUY)
    } else if signals.es_delta < -FLOW_CONTRACT_THRESHOLD {
        ("OFFER", C_SELL)
    } else {
        ("FLAT", C_TEXT)
    };
    let es_side = es_vwap_dev.map(|v| if v > VWAP_POINTS_THRESHOLD { 1 } else if v < -VWAP_POINTS_THRESHOLD { -1 } else { 0 });
    let nq_side = nq_vwap_dev.map(|v| if v > VWAP_POINTS_THRESHOLD { 1 } else if v < -VWAP_POINTS_THRESHOLD { -1 } else { 0 });
    let vwap_summary = match (es_side, nq_side) {
        (Some(1), Some(1)) => ("ABOVE", C_BUY),
        (Some(-1), Some(-1)) => ("BELOW", C_SELL),
        (Some(0), Some(0)) => ("FLAT", C_TEXT),
        (Some(_), Some(_)) => ("SPLIT", C_NEUTRAL),
        _ => ("----", C_DIM),
    };
    let trad_bias = match (es_bias.0, nq_bias.0) {
        ("BUY", "BUY") => ("BUY", C_BUY),
        ("SELL", "SELL") => ("SELL", C_SELL),
        ("BUY", "FLAT") | ("FLAT", "BUY") => ("BUY", C_BUY),
        ("SELL", "FLAT") | ("FLAT", "SELL") => ("SELL", C_SELL),
        _ => ("----", C_DIM),
    };

    lines.push(Line::from(vec![
        Span::styled(format!("{:<4}", ""), Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", summary_price.0, width = COL_PRICE), Style::default().fg(summary_price.1)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", summary_flow.0, width = COL_FLOW), Style::default().fg(summary_flow.1)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:>width$}", vwap_summary.0, width = COL_VWAP), Style::default().fg(vwap_summary.1)),
        Span::styled(COL_GAP, Style::default().fg(C_TEXT)),
        Span::styled(format!("{:^width$}", trad_bias.0, width = COL_BIAS), Style::default().fg(trad_bias.1).add_modifier(Modifier::BOLD)),
    ]));

    lines.push(Line::from(""));
    lines.push(Line::from(""));

    // === ROW 7: 60s comparisons ===
    let btc_es_pct = signals.btc_es_spread * 100.0;
    let (es_btc_sym, es_btc_color) = {
        let es_dir = if es_ret_pct > 0.0 {
            1
        } else if es_ret_pct < 0.0 {
            -1
        } else {
            0
        };
        let sym = if btc_es_pct < -0.05 {
            ">"
        } else if btc_es_pct > 0.05 {
            "<"
        } else {
            "="
        };
        let color = if btc_es_pct.abs() < 0.05 || es_dir == 0 {
            C_TEXT
        } else if es_dir > 0 && btc_es_pct < 0.0 {
            C_BUY
        } else if es_dir < 0 && btc_es_pct > 0.0 {
            C_SELL
        } else {
            C_TEXT
        };
        (sym, color)
    };
    let nq_es_pct = signals.nq_es_spread * 100.0;
    let (es_nq_sym, es_nq_color) = if nq_es_pct < -0.05 {
        (">", C_NEUTRAL)
    } else if nq_es_pct > 0.05 {
        ("<", C_NEUTRAL)
    } else {
        ("=", C_TEXT)
    };
    let sync_label = if nq_es_pct.abs() < 0.05 { "(SYNC)" } else { "" };

    lines.push(Line::from(vec![
        Span::styled("60s: ", Style::default().fg(C_TEXT)),
        Span::styled(format!("ES {} BTC ", es_btc_sym), Style::default().fg(es_btc_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:+.2}%", btc_es_pct.abs()), Style::default().fg(es_btc_color)),
        Span::styled(" │ ", Style::default().fg(C_TEXT)),
        Span::styled(format!("ES {} NQ ", es_nq_sym), Style::default().fg(es_nq_color).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:+.2}% {}", nq_es_pct.abs(), sync_label), Style::default().fg(es_nq_color)),
    ]));

    // === ROW 8: Three cards - ES/NQ, ES/BTC, LEAD ===
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
    let lead_corr_ok = signals
        .lead_lag_corr
        .map(|c| c.abs() >= 0.50)
        .unwrap_or(false);
    let lead_val = if !lead_corr_ok {
        "N/A".to_string()
    } else if signals.lead_lag_secs > 0 {
        "ES".to_string()
    } else if signals.lead_lag_secs < 0 {
        "BTC".to_string()
    } else {
        "SYNC".to_string()
    };
    let lead_time = if lead_corr_ok && signals.lead_lag_secs != 0 {
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

    // === ROW 12: Intelligent message ===
    let es_flow = if signals.es_delta > FLOW_CONTRACT_THRESHOLD {
        "buy"
    } else if signals.es_delta < -FLOW_CONTRACT_THRESHOLD {
        "sell"
    } else {
        "flat"
    };
    let nq_flow = if signals.nq_delta > FLOW_CONTRACT_THRESHOLD {
        "buy"
    } else if signals.nq_delta < -FLOW_CONTRACT_THRESHOLD {
        "sell"
    } else {
        "flat"
    };
    let price_dir = if es_ret_pct > 0.01 {
        "up"
    } else if es_ret_pct < -0.01 {
        "down"
    } else {
        "flat"
    };
    let vwap_pos = match es_vwap_dev {
        Some(v) if v > VWAP_POINTS_THRESHOLD => "above",
        Some(v) if v < -VWAP_POINTS_THRESHOLD => "below",
        Some(_) => "near",
        None => "none",
    };

    let es_state = if price_dir == "up" && es_flow == "sell" {
        "ES up but offers hitting".to_string()
    } else if price_dir == "down" && es_flow == "buy" {
        "ES down but bids lifting".to_string()
    } else if es_flow == "buy" {
        let suffix = match vwap_pos {
            "above" => "above VWAP",
            "below" => "below VWAP",
            "near" => "near VWAP",
            _ => "no VWAP",
        };
        format!("ES buying {}", suffix)
    } else if es_flow == "sell" {
        let suffix = match vwap_pos {
            "above" => "above VWAP",
            "below" => "below VWAP",
            "near" => "near VWAP",
            _ => "no VWAP",
        };
        format!("ES selling {}", suffix)
    } else if price_dir == "up" {
        "ES drifting up".to_string()
    } else if price_dir == "down" {
        "ES drifting down".to_string()
    } else {
        "ES quiet".to_string()
    };

    let nq_note = if es_flow != "flat" && nq_flow == es_flow {
        "NQ confirms"
    } else if nq_flow == "flat" {
        "NQ flat"
    } else if es_flow == "flat" {
        if nq_flow == "buy" { "NQ buying" } else { "NQ selling" }
    } else {
        "NQ diverges"
    };

    let crypto_note = match signals.es_btc_corr {
        Some(c) if c <= -0.5 => format!("BTC inverse to TradFi ⚡ (ρ={:.2})", c),
        Some(c) if c.abs() >= 0.5 => {
            let direction = if es_flow == "buy" {
                if vwap_pos == "below" { "BTC may bounce ▲" } else { "BTC likely follows ▲" }
            } else if es_flow == "sell" {
                if vwap_pos == "above" { "BTC may drop ▼" } else { "BTC likely follows ▼" }
            } else {
                "BTC direction unclear"
            };
            format!("{} (ρ={:.2})", direction, c)
        }
        Some(c) if c.abs() >= 0.3 => format!("BTC weak link (ρ={:.2})", c),
        Some(c) => format!("BTC decoupled (ρ={:.2})", c),
        None => "BTC link unknown".to_string(),
    };

    let max_width = inner.width as usize;
    let fit_line = |text: String| -> String {
        if max_width == 0 || text.len() <= max_width {
            return text;
        }
        if max_width <= 3 {
            return text.chars().take(max_width).collect();
        }
        let truncated: String = text.chars().take(max_width - 3).collect();
        format!("{}...", truncated)
    };

    let trad_line = format!("→ {}, {}", es_state, nq_note);
    let crypto_line = format!("→ {}", crypto_note);

    lines.push(Line::from(vec![
        Span::styled(fit_line(trad_line), Style::default().fg(C_TEXT)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(fit_line(crypto_line), Style::default().fg(C_TEXT)),
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
