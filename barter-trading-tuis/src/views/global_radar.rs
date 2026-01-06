//! Global Radar view - Design C v3 "Clean Layout"
//!
//! Clean institutional layout with:
//! - Spaced tables (no line separators)
//! - Gradient gamma bar with neutral indicator
//! - Flow + L2 Depth integration

use crate::shared::market_state::{Direction, GammaPosition, State, TradingBias, VolRegime, Warning};
use crate::views::{format_compact, render_header, ActiveView, ViewContext};
use ratatui::{
    layout::{Constraint, Direction as LayoutDir, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

pub fn render(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let chunks = Layout::default()
        .direction(LayoutDir::Vertical)
        .constraints([
            Constraint::Length(4),   // Header
            Constraint::Length(5),   // State banner
            Constraint::Length(9),   // L1/L2/L3 cards
            Constraint::Length(12),  // Flow + Liquidity (room for L2 + walls)
            Constraint::Length(13),  // GAMMA CONTEXT (with spacing)
            Constraint::Min(0),      // Spare
            Constraint::Length(2),   // Footer
        ])
        .split(area);

    render_header(f, chunks[0], ctx, ActiveView::GlobalRadar);
    render_state_banner(f, chunks[1], ctx);
    render_filter_cards(f, chunks[2], ctx);
    render_flow_and_depth(f, chunks[3], ctx);
    render_gamma_context(f, chunks[4], ctx);
    render_warnings_strip(f, chunks[5], ctx);
    render_footer(f, chunks[6], ctx);
}

/// Big colored state banner
fn render_state_banner(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    if let Some(result) = ctx.state {
        let state = &result.state;
        let (state_str, state_color, bg_color) = match state.state {
            State::Ready => ("R E A D Y", Color::Black, Color::Green),
            State::Caution => ("C A U T I O N", Color::Black, Color::Yellow),
            State::Wait => ("W A I T", Color::White, Color::Red),
        };

        let bias_str = state.bias.map(|b| match b {
            TradingBias::MeanReversion => "MEAN REV",
            TradingBias::Momentum => "MOMENTUM",
        }).unwrap_or("--");

        let direction_str = match state.components.flow_consensus.consensus_direction {
            Direction::Long => "LONG",
            Direction::Short => "SHORT",
            Direction::Neutral => "NEUTRAL",
        };

        let line1 = Line::from(vec![
            Span::raw("                              "),
            Span::styled(
                format!(" {} ", state_str),
                Style::default().fg(state_color).bg(bg_color).add_modifier(Modifier::BOLD),
            ),
        ]);

        let line2 = Line::from(vec![
            Span::raw("  Bias: "),
            Span::styled(bias_str, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Span::raw("  |  Flow: "),
            Span::styled(
                direction_str,
                Style::default().fg(match direction_str {
                    "LONG" => Color::Green,
                    "SHORT" => Color::Red,
                    _ => Color::Gray,
                }),
            ),
            Span::raw("  |  Confidence: "),
            Span::styled(format!("{}%", state.confidence), Style::default().fg(Color::White)),
        ]);

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(match state.state {
                State::Ready => Color::Green,
                State::Caution => Color::Yellow,
                State::Wait => Color::Red,
            }));

        f.render_widget(Paragraph::new(vec![line1, line2]).block(block), area);
    } else {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        f.render_widget(Paragraph::new("  Waiting for state engine...").block(block), area);
    }
}

/// Four horizontal cards: L1/L2/L3 Trigger/L3 Fuel
fn render_filter_cards(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let cols = Layout::default()
        .direction(LayoutDir::Horizontal)
        .constraints([
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
            Constraint::Percentage(25),
        ])
        .split(area);

    if let Some(result) = ctx.state {
        let vol = &result.state.components.vol_regime;
        let gamma = &result.state.components.gamma_context;
        let flow = &result.state.components.flow_consensus;
        let funding = &result.state.components.funding_context;
        let fuel = &result.state.components.fuel_context;

        // L1: REGIME (Dual-Vol: Percentile + Z-Score)
        let l1_color = if vol.passed { Color::Green } else { Color::Red };
        let vol_regime_str = match vol.regime {
            VolRegime::Low => "LOW",
            VolRegime::Normal => "NORMAL",
            VolRegime::High => "HIGH",
            VolRegime::Extreme => "EXTREME",
        };
        let zscore_color = if vol.zscore_1m.abs() >= 3.5 {
            Color::Red
        } else if vol.zscore_1m.abs() >= 2.0 {
            Color::Yellow
        } else {
            Color::Green
        };
        let l1_lines = vec![
            Line::from(vec![
                Span::styled(
                    if vol.passed { "● PASS" } else { "✗ FAIL" },
                    Style::default().fg(l1_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw("  "),
                Span::styled(vol_regime_str, Style::default().fg(l1_color)),
            ]),
            Line::from(format!("  Pctl: {:.0}th  RV: {:.2}%", vol.percentile, vol.current_rv * 100.0)),
            Line::from(vec![
                Span::raw("  σ: "),
                Span::styled(format!("{:.1}", vol.zscore_1m), Style::default().fg(zscore_color)),
                Span::raw("  "),
                if vol.is_shock {
                    Span::styled("SHOCK!", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD))
                } else {
                    Span::styled("OK", Style::default().fg(Color::Green))
                },
            ]),
            Line::from(Span::styled(
                format!("  Status: {}", if vol.passed { "PASS" } else { "FAIL" }),
                Style::default().fg(l1_color).add_modifier(Modifier::BOLD),
            )),
        ];
        let l1_block = Block::default()
            .borders(Borders::ALL)
            .title(" L1 REGIME ")
            .border_style(Style::default().fg(l1_color));
        f.render_widget(Paragraph::new(l1_lines).block(l1_block), cols[0]);

        // L2: GAMMA
        let flip_dist = gamma.distance_pct.abs();
        let l2_color = if flip_dist > 8.0 { Color::Yellow } else { Color::Cyan };
        let position_str = match gamma.position {
            GammaPosition::AboveFlip => "ABOVE FLIP",
            GammaPosition::BelowFlip => "BELOW FLIP",
            GammaPosition::AtFlip => "AT FLIP",
        };
        let bias_str = match gamma.bias {
            TradingBias::MeanReversion => "MEAN REV",
            TradingBias::Momentum => "MOMENTUM",
        };

        let l2_lines = vec![
            Line::from(vec![
                Span::styled("● ", Style::default().fg(l2_color)),
                Span::styled(position_str, Style::default().fg(l2_color)),
                if flip_dist > 8.0 {
                    Span::styled(" (FAR)", Style::default().fg(Color::Yellow))
                } else {
                    Span::raw("")
                },
            ]),
            Line::from(format!("  {:+.1}% from flip", gamma.distance_pct)),
            Line::from(vec![
                Span::raw("  Bias: "),
                Span::styled(bias_str, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            ]),
            Line::from(if flip_dist > 8.0 {
                Span::styled("  Status: LOW VALUE", Style::default().fg(Color::Yellow))
            } else {
                Span::styled("  Status: PASS", Style::default().fg(Color::Green))
            }),
        ];
        let l2_block = Block::default()
            .borders(Borders::ALL)
            .title(" L2 GAMMA ")
            .border_style(Style::default().fg(l2_color));
        f.render_widget(Paragraph::new(l2_lines).block(l2_block), cols[1]);

        // L3: TRIGGER
        let l3_passed = flow.passed && funding.passed && !flow.absorption_detected;
        let l3_color = if l3_passed {
            Color::Green
        } else if flow.absorption_detected {
            Color::Red
        } else {
            Color::Yellow
        };
        let direction_str = match flow.consensus_direction {
            Direction::Long => "LONG",
            Direction::Short => "SHORT",
            Direction::Neutral => "FLAT",
        };
        let funding_str = format!("{:.3}%", funding.current_rate * 100.0);
        let vel_pct = funding.velocity * 100.0;
        let vel_icon = if vel_pct.abs() < 0.005 {
            "→"
        } else if vel_pct > 0.0 {
            "▲"
        } else {
            "▼"
        };
        let vel_color = if funding.is_spiking {
            Color::Red
        } else if vel_pct.abs() >= 0.01 {
            Color::Yellow
        } else {
            Color::Gray
        };
        let l3_lines = vec![
            Line::from(vec![
                Span::styled(
                    if flow.passed { "✓" } else { "✗" },
                    Style::default().fg(if flow.passed { Color::Green } else { Color::Red }),
                ),
                Span::raw(format!(" CVD: {}/{} {}", flow.venues_agreeing, flow.venues_total, direction_str)),
            ]),
            Line::from(vec![
                Span::styled(
                    if funding.passed { "✓" } else { "✗" },
                    Style::default().fg(if funding.passed { Color::Green } else { Color::Red }),
                ),
                Span::raw(format!(" Fund: {}", funding_str)),
                Span::raw("  "),
                Span::styled(vel_icon, Style::default().fg(vel_color).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {:+.2}%/15m", vel_pct)),
            ]),
            Line::from(vec![
                Span::styled(
                    if !flow.absorption_detected { "✓" } else { "✗" },
                    Style::default().fg(if !flow.absorption_detected { Color::Green } else { Color::Red }),
                ),
                Span::raw(if flow.absorption_detected { " Absorb: YES" } else { " Absorb: NO" }),
            ]),
            Line::from(Span::styled(
                format!("  Status: {}", if l3_passed { "PASS" } else { "CAUTION" }),
                Style::default().fg(l3_color).add_modifier(Modifier::BOLD),
            )),
        ];
        let l3_block = Block::default()
            .borders(Borders::ALL)
            .title(" L3 TRIGGER ")
            .border_style(Style::default().fg(l3_color));
        f.render_widget(Paragraph::new(l3_lines).block(l3_block), cols[2]);

        // L3: FUEL
        let rvol_line = match fuel.rvol_status {
            crate::shared::market_state::RvolStatus::Strong => ("✓", Color::Green, "STRONG"),
            crate::shared::market_state::RvolStatus::Normal => ("✓", Color::Green, "OK"),
            crate::shared::market_state::RvolStatus::Thin => ("⚠", Color::Yellow, "THIN"),
            crate::shared::market_state::RvolStatus::Fail => ("✗", Color::Red, "FAIL"),
        };
        let (oi_icon, oi_color, oi_label) = match fuel.oi_trend {
            crate::shared::market_state::OiTrend::NewMoney => ("✓", Color::Green, "NEW"),
            crate::shared::market_state::OiTrend::Squeeze => ("⚠", Color::Yellow, "SQZ"),
            crate::shared::market_state::OiTrend::Flat => ("•", Color::Gray, "FLAT"),
        };
        let (liq_icon, liq_color, liq_label) = match fuel.liq_state {
            crate::shared::market_state::LiqState::Low => ("✓", Color::Green, "LOW"),
            crate::shared::market_state::LiqState::Moderate => ("•", Color::Gray, "MED"),
            crate::shared::market_state::LiqState::High => ("⚠", Color::Yellow, "HIGH"),
            crate::shared::market_state::LiqState::Exhaustion => ("✗", Color::Red, "EXHT"),
        };
        let (quality_label, quality_color) = match fuel.quality {
            crate::shared::market_state::FuelQuality::High => ("HIGH", Color::Green),
            crate::shared::market_state::FuelQuality::Medium => ("MED", Color::Yellow),
            crate::shared::market_state::FuelQuality::Low => ("LOW", Color::Yellow),
            crate::shared::market_state::FuelQuality::Fail => ("FAIL", Color::Red),
        };

        let fuel_lines = vec![
            Line::from(vec![
                Span::styled(rvol_line.0, Style::default().fg(rvol_line.1)),
                Span::raw(format!(" RVOL: {:.2}x ", fuel.rvol)),
                Span::styled(rvol_line.2, Style::default().fg(rvol_line.1)),
            ]),
            Line::from(vec![
                Span::styled(oi_icon, Style::default().fg(oi_color)),
                Span::raw(" OI Δ: $"),
                Span::styled(format_compact(fuel.oi_delta_usd_5m), Style::default().fg(oi_color)),
                Span::raw(" "),
                Span::styled(oi_label, Style::default().fg(oi_color)),
            ]),
            Line::from(vec![
                Span::styled(liq_icon, Style::default().fg(liq_color)),
                Span::raw(" LIQ: "),
                Span::styled(liq_label, Style::default().fg(liq_color)),
                Span::raw(" "),
                Span::styled(format_compact(fuel.liq_rate_usd_per_min), Style::default().fg(liq_color)),
                Span::raw("/m"),
            ]),
            Line::from(Span::styled(
                format!("  Quality: {}", quality_label),
                Style::default().fg(quality_color).add_modifier(Modifier::BOLD),
            )),
        ];
        let fuel_block = Block::default()
            .borders(Borders::ALL)
            .title(" L3 FUEL ")
            .border_style(Style::default().fg(quality_color));
        f.render_widget(Paragraph::new(fuel_lines).block(fuel_block), cols[3]);
    } else {
        for col in cols.iter() {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray));
            f.render_widget(Paragraph::new("--").block(block), *col);
        }
    }
}

/// Flow table + Liquidity context (smoothed L2 + walls)
fn render_flow_and_depth(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let ticker = ctx.focused_ticker;
    let snapshot = ctx.snapshot.tickers.get(ticker);

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" FLOW + LIQUIDITY ")
        .border_style(Style::default().fg(Color::Cyan));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let columns = Layout::default()
        .direction(LayoutDir::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(inner);

    // Left: Flow table
    let mut lines = vec![Line::from(vec![
        Span::styled(format!("{:^8}", "VENUE"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:^12}", "CVD 5m"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:^10}", "FLOW 30s"), Style::default().add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:^8}", "VERDICT"), Style::default().add_modifier(Modifier::BOLD)),
    ])];

    if let Some(snap) = snapshot {
        // CVD lookup uses full exchange names
        let cvd_lookup = |needle: &str| -> f64 {
            let needle = needle.to_lowercase();
            snap.cvd_per_exchange_5m
                .iter()
                .find(|(k, _)| k.to_lowercase().contains(&needle))
                .map(|(_, v)| *v)
                .unwrap_or(0.0)
        };

        // Flow lookup uses full exchange names
        let flow_pct_lookup = |needle: &str| -> Option<f64> {
            let needle = needle.to_lowercase();
            snap.per_exchange_30s
                .iter()
                .find(|(k, _)| k.to_lowercase().contains(&needle))
                .map(|(_, v)| v)
                .and_then(|v| {
                    if v.total_30s <= 0.0 {
                        None
                    } else {
                        let buy = (v.total_30s + v.cvd_30s) / 2.0;
                        Some((buy / v.total_30s * 100.0).round())
                    }
                })
        };

        // Binance (key: BNC)
        lines.push(format_venue_row_clean("Binance", cvd_lookup("binance"), flow_pct_lookup("binance")));
        // Bybit (key: BBT)
        lines.push(format_venue_row_clean("Bybit", cvd_lookup("bybit"), flow_pct_lookup("bybit")));
        // OKX (key: OKX)
        lines.push(format_venue_row_clean("OKX", cvd_lookup("okx"), flow_pct_lookup("okx")));

        lines.push(Line::from("")); // Separator

        // Totals row
        if let Some(result) = ctx.state {
            let flow = &result.state.components.flow_consensus;

            lines.push(Line::from(vec![
                Span::styled(format!("{:^8}", "TOTAL"), Style::default().add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!("{:^12}", format_compact(flow.cvd_net)),
                    Style::default().fg(if flow.cvd_net >= 0.0 { Color::Green } else { Color::Red }).add_modifier(Modifier::BOLD),
                ),
                Span::styled(
                    format!("{:^10}", format!("{}/{} {:?}", flow.venues_agreeing, flow.venues_total, flow.consensus_direction)),
                    Style::default().fg(if flow.passed { Color::Green } else { Color::Yellow }),
                ),
                Span::raw(format!("{:^8}", "")),
            ]));
        }

        let left_block = Paragraph::new(lines);
        f.render_widget(left_block, columns[0]);

        // Right: L2 imbalance + walls
        let mut right_lines = Vec::new();
        right_lines.push(Line::from(Span::styled(
            "L2 IMBALANCE",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )));

        let smoothed = snap.aggregated_book_imbalance_smoothed;
        let ask_pct = (100.0 - smoothed).max(0.0);
        let ratio = if ask_pct > 0.0 { smoothed / ask_pct } else { 0.0 };
        let ratio_color = if smoothed > 55.0 {
            Color::Green
        } else if smoothed < 45.0 {
            Color::Red
        } else {
            Color::Gray
        };
        right_lines.push(Line::from(vec![
            Span::styled("BID/ASK ", Style::default().fg(Color::Gray)),
            Span::styled(
                format!("{:>4.2} ", ratio),
                Style::default().fg(ratio_color).add_modifier(Modifier::BOLD),
            ),
            render_depth_bar_clean(smoothed),
            Span::raw(" "),
            Span::styled("EMA 3s", Style::default().fg(Color::DarkGray)),
        ]));
        right_lines.push(Line::from(""));
        let use_base = ticker.to_uppercase().contains("BTC");
        right_lines.extend(render_wall_lines(
            "SELL WALL",
            &snap.ask_walls,
            Color::Red,
            use_base,
        ));
        right_lines.extend(render_wall_lines(
            "BID WALL",
            &snap.bid_walls,
            Color::Green,
            use_base,
        ));

        f.render_widget(Paragraph::new(right_lines), columns[1]);
        return;
    } else {
        lines.push(Line::from("  No data"));
    }

    f.render_widget(Paragraph::new(lines), columns[0]);
}

fn format_venue_row_clean(name: &str, cvd: f64, flow_pct: Option<f64>) -> Line<'static> {
    let cvd_color = if cvd >= 0.0 { Color::Green } else { Color::Red };
    let verdict = if cvd >= 0.0 { "BUY" } else { "SELL" };
    let verdict_color = if cvd >= 0.0 { Color::Green } else { Color::Red };
    let flow_str = flow_pct.map(|p| format!("{:.0}%", p)).unwrap_or_else(|| "--".to_string());

    Line::from(vec![
        Span::styled(format!("{:^8}", name), Style::default().fg(Color::White)),
        Span::styled(format!("{:^12}", format_compact(cvd)), Style::default().fg(cvd_color)),
        Span::styled(format!("{:^10}", flow_str), Style::default().fg(Color::Gray)),
        Span::styled(format!("{:^8}", verdict), Style::default().fg(verdict_color).add_modifier(Modifier::BOLD)),
    ])
}

/// Clean depth bar without brackets: ████░░░░ 65%
fn render_depth_bar_clean(imbalance: f64) -> Span<'static> {
    let bid_pct = imbalance.clamp(0.0, 100.0);
    let filled = ((bid_pct / 100.0) * 8.0).round() as usize;
    let empty = 8 - filled;

    let bar = format!("{}{} {:>3.0}%", "█".repeat(filled), "░".repeat(empty), bid_pct);
    let color = if bid_pct > 55.0 {
        Color::Green
    } else if bid_pct < 45.0 {
        Color::Red
    } else {
        Color::Gray
    };

    Span::styled(format!("{:^16}", bar), Style::default().fg(color))
}

fn render_wall_lines(
    label: &str,
    walls: &[crate::shared::state::BookWall],
    color: Color,
    use_base: bool,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    for idx in 0..2 {
        let suffix = if idx == 0 { "" } else { " 2" };
        if let Some(wall) = walls.get(idx) {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{label}{suffix}: "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled(format!("${:.0}", wall.price), Style::default().fg(Color::White)),
                Span::raw("  "),
                Span::styled(
                    format!("{:+.2}%", wall.distance_pct),
                    Style::default().fg(Color::Gray),
                ),
                Span::raw("  "),
                Span::styled(
                    if use_base {
                        format!("{:.0} BTC", wall.size_base)
                    } else {
                        format_compact_abs(wall.notional_usd)
                    },
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(" "),
                Span::styled(
                    wall.tier.label(),
                    Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
                ),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{label}{suffix}: "),
                    Style::default().fg(color).add_modifier(Modifier::BOLD),
                ),
                Span::styled("--", Style::default().fg(Color::Gray)),
            ]));
        }
    }
    lines
}

fn format_compact_abs(value: f64) -> String {
    let mut out = format_compact(value.abs());
    if let Some(stripped) = out.strip_prefix('+') {
        out = stripped.to_string();
    }
    out
}

/// GAMMA CONTEXT with breathing room and gradient bar
fn render_gamma_context(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let mut lines: Vec<Line> = Vec::new();

    if let Some(result) = ctx.state {
        let gamma = &result.state.components.gamma_context;
        let spot = gamma.current_price;

        // Get front bucket data
        let (front_flip, front_label, put_wall, call_wall) = if let Some(opt) = &result.state.components.options_context {
            let front = opt.buckets.iter().find(|b| b.is_front);
            let flip = front.map(|b| b.gamma_flip).unwrap_or(gamma.gamma_flip_price);
            let label = front.map(|b| b.label.clone()).unwrap_or_else(|| "all".to_string());
            let pw = opt.put_wall.as_ref().map(|w| w.strike).unwrap_or(0.0);
            let cw = opt.call_wall.as_ref().map(|w| w.strike).unwrap_or(0.0);
            (flip, label, pw, cw)
        } else {
            (gamma.gamma_flip_price, "all".to_string(), 0.0, 0.0)
        };

        let flip = if front_flip > 0.0 { front_flip } else { gamma.gamma_flip_price };

        let (regime_str, regime_color) = match gamma.position {
            GammaPosition::AboveFlip => ("+γ MEAN REV", Color::Green),
            GammaPosition::BelowFlip => ("-γ MOMENTUM", Color::Red),
            GammaPosition::AtFlip => ("FLIP ZONE", Color::Yellow),
        };

        // Line 1: Header with spot/flip/regime
        lines.push(Line::from(vec![
            Span::raw(" SPOT "),
            Span::styled(format!("${:.0}", spot), Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
            Span::raw("    FLIP "),
            Span::styled(format!("${:.0}", flip), Style::default().fg(Color::Magenta).add_modifier(Modifier::BOLD)),
            Span::styled(format!(" ({})", front_label), Style::default().fg(Color::Gray)),
            Span::raw("    "),
            Span::styled(regime_str, Style::default().fg(regime_color).add_modifier(Modifier::BOLD)),
        ]));

        // Line 2: Empty for spacing
        lines.push(Line::from(""));

        // Line 3: Gradient gamma bar (like reference)
        lines.push(render_gamma_gradient_bar(spot, flip, put_wall, call_wall));

        // Line 4: Empty for spacing
        lines.push(Line::from(""));

        // Line 5: Bucket table header
        lines.push(Line::from(vec![
            Span::styled(format!("{:^8}", "BUCKET"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^9}", "FLIP"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^7}", "DIST"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^7}", "MAX"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^8}", "GEX"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^8}", "DEX"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^7}", "WALL"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^5}", "P/C"), Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
            Span::styled(format!("{:^5}", "EXP"), Style::default().add_modifier(Modifier::BOLD)),
        ]));

        // Bucket rows
        if let Some(opt) = &result.state.components.options_context {
            for bucket in &opt.buckets {
                if bucket.contract_count == 0 {
                    continue;
                }

                let label = if bucket.is_front {
                    format!("▶{}", bucket.label)
                } else {
                    format!(" {}", bucket.label)
                };

                let flip_str = if bucket.gamma_flip > 0.0 {
                    format!("${:.0}", bucket.gamma_flip)
                } else {
                    "--".to_string()
                };

                let max_pain_str = if bucket.max_pain > 0.0 {
                    format!("${:.0}", bucket.max_pain)
                } else {
                    "--".to_string()
                };

                // Calculate distance from spot to this bucket's flip
                let dist_pct = if bucket.gamma_flip > 0.0 {
                    ((spot - bucket.gamma_flip) / bucket.gamma_flip) * 100.0
                } else {
                    0.0
                };
                let dist_str = format!("{:+.1}%", dist_pct);
                let dist_color = if dist_pct.abs() > 8.0 {
                    Color::Yellow  // FAR - low tactical value
                } else if dist_pct > 0.0 {
                    Color::Green   // Above flip (mean rev zone)
                } else {
                    Color::Red     // Below flip (momentum zone)
                };

                let gex_color = if bucket.gex >= 0.0 { Color::Green } else { Color::Red };
                let dex_color = if bucket.dex >= 0.0 { Color::Green } else { Color::Red };
                let ratio_pct = bucket.put_call_ratio * 100.0;
                let ratio_color = if ratio_pct >= 110.0 { Color::Red } else if ratio_pct <= 90.0 { Color::Green } else { Color::Gray };

                let (wall_str, wall_color) = if let Some(wall) = bucket.call_wall.as_ref() {
                    (format!("${:.0}", wall.strike), Color::Red)
                } else if let Some(wall) = bucket.put_wall.as_ref() {
                    (format!("${:.0}", wall.strike), Color::Green)
                } else {
                    ("--".to_string(), Color::Gray)
                };

                let expiry_str = if bucket.hours_to_expiry < f64::MAX {
                    if bucket.hours_to_expiry < 24.0 {
                        format!("{:.0}h", bucket.hours_to_expiry)
                    } else {
                        format!("{:.0}d", bucket.hours_to_expiry / 24.0)
                    }
                } else {
                    "--".to_string()
                };

                let row_style = if bucket.is_front {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                };

                lines.push(Line::from(vec![
                    Span::styled(format!("{:^8}", label), row_style),
                    Span::raw("  "),
                    Span::styled(format!("{:^9}", flip_str), Style::default().fg(Color::Magenta)),
                    Span::raw("  "),
                    Span::styled(format!("{:^7}", dist_str), Style::default().fg(dist_color)),
                    Span::raw("  "),
                    Span::styled(format!("{:^7}", max_pain_str), Style::default().fg(Color::Yellow)),
                    Span::raw("  "),
                    Span::styled(format!("{:^8}", format_compact(bucket.gex)), Style::default().fg(gex_color)),
                    Span::raw("  "),
                    Span::styled(format!("{:^8}", format_compact(bucket.dex)), Style::default().fg(dex_color)),
                    Span::raw("  "),
                    Span::styled(format!("{:^7}", wall_str), Style::default().fg(wall_color)),
                    Span::raw("  "),
                    Span::styled(format!("{:^5}", format!("{:.0}%", ratio_pct)), Style::default().fg(ratio_color)),
                    Span::raw("  "),
                    Span::styled(format!("{:^5}", expiry_str), Style::default().fg(Color::Gray)),
                ]));
            }

            // Totals row
            lines.push(Line::from(vec![
                Span::styled("TOTAL: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::styled("GEX ", Style::default().fg(Color::Gray)),
                Span::styled(
                    format_compact(opt.gexp),
                    Style::default().fg(if opt.gexp >= 0.0 { Color::Green } else { Color::Red }),
                ),
                Span::raw("  DEX "),
                Span::styled(
                    format_compact(opt.dexp),
                    Style::default().fg(if opt.dexp >= 0.0 { Color::Green } else { Color::Red }),
                ),
                Span::raw("  VEX "),
                Span::styled(format_compact(opt.vexp), Style::default().fg(Color::Cyan)),
                Span::raw("  P/C "),
                Span::styled(
                    format!("{:.0}%", opt.put_call_oi_ratio * 100.0),
                    Style::default().fg(Color::Gray),
                ),
            ]));
        } else {
            lines.push(Line::from(Span::styled("  No Deribit data", Style::default().fg(Color::Gray))));
        }
    } else {
        lines.push(Line::from("  Waiting for data..."));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" GAMMA CONTEXT ")
        .border_style(Style::default().fg(Color::Magenta));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Gradient gamma bar like reference: red─────●─────green with neutral indicator
fn render_gamma_gradient_bar(spot: f64, flip: f64, put_wall: f64, call_wall: f64) -> Line<'static> {
    let bar_width = 50;

    // Determine price range
    let min_price = if put_wall > 0.0 { put_wall.min(flip * 0.85) } else { flip * 0.85 };
    let max_price = if call_wall > 0.0 { call_wall.max(flip * 1.15) } else { flip * 1.15 };
    let range = max_price - min_price;

    if range <= 0.0 || spot <= 0.0 || flip <= 0.0 {
        return Line::from(Span::styled("  [No price data]", Style::default().fg(Color::Gray)));
    }

    // Calculate positions (0 to bar_width)
    let flip_pos = ((flip - min_price) / range * bar_width as f64).round() as i32;
    let spot_pos = ((spot - min_price) / range * bar_width as f64).round().clamp(0.0, bar_width as f64) as i32;

    // Build bar with gradient: red on left of flip, green on right
    let mut spans: Vec<Span> = Vec::new();

    // Left label
    let put_str = if put_wall > 0.0 { format!("${:.0}K", put_wall / 1000.0) } else { format!("${:.0}K", min_price / 1000.0) };
    spans.push(Span::styled(format!(" {:>6} ", put_str), Style::default().fg(Color::Red)));

    // Build the bar character by character
    let mut bar_str = String::new();
    for i in 0..bar_width as i32 {
        if i == spot_pos {
            bar_str.push('●');
        } else if i == flip_pos {
            bar_str.push('┃');
        } else {
            bar_str.push('─');
        }
    }

    // Split bar at flip position for coloring
    let flip_idx = flip_pos.clamp(0, bar_width as i32) as usize;
    let left_part: String = bar_str.chars().take(flip_idx).collect();
    let right_part: String = bar_str.chars().skip(flip_idx).collect();

    if !left_part.is_empty() {
        spans.push(Span::styled(left_part, Style::default().fg(Color::Red)));
    }
    if !right_part.is_empty() {
        spans.push(Span::styled(right_part, Style::default().fg(Color::Green)));
    }

    // Right label
    let call_str = if call_wall > 0.0 { format!("${:.0}K", call_wall / 1000.0) } else { format!("${:.0}K", max_price / 1000.0) };
    spans.push(Span::styled(format!(" {:<6}", call_str), Style::default().fg(Color::Green)));

    Line::from(spans)
}

/// Warnings strip
fn render_warnings_strip(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let mut warn_spans: Vec<Span> = vec![Span::styled(" WARNINGS: ", Style::default().add_modifier(Modifier::BOLD))];

    if let Some(result) = ctx.state {
        let has_warnings = !result.state.components.warnings.is_empty()
            || !result.freshness_warnings.is_empty();

        if !has_warnings {
            warn_spans.push(Span::styled("None", Style::default().fg(Color::Green)));
        } else {
            for (i, warn) in result.state.components.warnings.iter().enumerate() {
                if i > 0 {
                    warn_spans.push(Span::raw("  "));
                }
                warn_spans.push(Span::styled(
                    warning_label(warn),
                    Style::default().fg(Color::Yellow),
                ));
            }
            for (signal, age_ms) in &result.freshness_warnings {
                warn_spans.push(Span::raw("  "));
                let age_str = if *age_ms > 60000 {
                    format!("{:.1}m", *age_ms as f64 / 60000.0)
                } else {
                    format!("{:.1}s", *age_ms as f64 / 1000.0)
                };
                warn_spans.push(Span::styled(
                    format!("{:?}: {}", signal, age_str),
                    Style::default().fg(Color::Red),
                ));
            }
        }
    } else {
        warn_spans.push(Span::styled("--", Style::default().fg(Color::Gray)));
    }

    f.render_widget(Paragraph::new(Line::from(warn_spans)), area);
}

fn render_footer(f: &mut Frame, area: Rect, ctx: &ViewContext<'_>) {
    let ticker = ctx.focused_ticker;
    let snapshot = ctx.snapshot.tickers.get(ticker);
    let price = snapshot.and_then(|t| t.binance_perp_last).unwrap_or(0.0);
    let price_str = if price >= 1000.0 {
        format!("${:.2}", price)
    } else if price > 0.0 {
        format!("${:.4}", price)
    } else {
        "--".to_string()
    };

    let line = if let Some(result) = ctx.state {
        let state_color = match result.state.state {
            State::Ready => Color::Green,
            State::Caution => Color::Yellow,
            State::Wait => Color::Red,
        };
        Line::from(vec![
            Span::styled(
                format!(" {} {} ", ticker, price_str),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::raw("| #"),
            Span::styled(format!("{}", result.state.audit_id), Style::default().fg(Color::Cyan)),
            Span::raw(" | "),
            Span::styled(format!("{:?}", result.state.state), Style::default().fg(state_color)),
            Span::raw(" | "),
            Span::styled(
                if result.no_gamma_mode { "NO-OPTS" } else { "OPTS-OK" },
                Style::default().fg(if result.no_gamma_mode { Color::Yellow } else { Color::Green }),
            ),
            Span::raw(" | "),
            Span::styled(
                if result.no_trad_mode { "NO-T" } else { "T-OK" },
                Style::default().fg(if result.no_trad_mode { Color::Yellow } else { Color::Green }),
            ),
        ])
    } else {
        Line::from(format!(" {} {} | #-- | State: --", ticker, price_str))
    };

    f.render_widget(Paragraph::new(line), area);
}

fn warning_label(warn: &Warning) -> String {
    match warn {
        Warning::ApproachingGammaFlip { distance_pct } => format!("Flip {:+.1}%", distance_pct),
        Warning::FundingElevated { rate } => format!("Fund {:.3}%", rate * 100.0),
        Warning::LiquidationClusterNearby { price, size_usd } => {
            format!("Liq ${:.0}K ({})", price / 1000.0, format_compact(*size_usd))
        }
        Warning::SingleVenueDisagreeing { venue } => format!("{} ✗", venue),
        Warning::LowLiquiditySession => "Low Liq".to_string(),
        Warning::StaleData { signal, .. } => format!("{:?} stale", signal),
        Warning::NoGammaData => "NO-OPTS".to_string(),
        Warning::NoTradMarketsData => "NO-T".to_string(),
        Warning::ExpiryBucketConflict => "Bucket ⚠".to_string(),
    }
}
