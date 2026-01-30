//! Gap Detection Tool
//!
//! Checks for gaps in data coverage.
//!
//! Usage:
//!   check-gaps ./parquet --interval 60
//!   check-gaps ./parquet/bars_1m/BTCUSDT --interval 60 --verbose

use barter_tools::verify::{check_gaps, GapCheckResult};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "check-gaps")]
#[command(about = "Check for gaps in Parquet data coverage")]
struct Args {
    /// Directory containing Parquet files
    path: PathBuf,

    /// Expected bar interval in seconds (default: 60 for 1m bars)
    #[arg(long, default_value = "60")]
    interval: u64,

    /// Show all gaps (default: only summary)
    #[arg(short, long)]
    verbose: bool,

    /// Maximum gaps to display
    #[arg(long, default_value = "20")]
    max_gaps: usize,
}

fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("barter_tools=info".parse().unwrap()),
        )
        .init();

    let args = Args::parse();

    println!("========================================");
    println!("Gap Detection");
    println!("========================================");
    println!("Path:     {:?}", args.path);
    println!("Interval: {}s", args.interval);
    println!("========================================\n");

    // Convert interval to nanoseconds
    let interval_ns = args.interval * 1_000_000_000;

    let result = check_gaps(&args.path, interval_ns)?;

    print_result(&result, args.verbose, args.max_gaps);

    if result.gaps.is_empty() {
        println!("\n✓ No gaps detected!");
    } else {
        println!("\n⚠ {} gaps detected", result.gaps.len());
        std::process::exit(1);
    }

    Ok(())
}

fn print_result(result: &GapCheckResult, verbose: bool, max_gaps: usize) {
    println!("========================================");
    println!("Results");
    println!("========================================");
    println!("Files checked:  {}", result.files_checked);
    println!("Total rows:     {}", result.total_rows);

    if let (Some(min), Some(max)) = (result.min_ts_ns, result.max_ts_ns) {
        let min_secs = min / 1_000_000_000;
        let max_secs = max / 1_000_000_000;
        let duration_hours = (max - min) as f64 / 3_600_000_000_000.0;

        // Convert to human-readable dates
        let min_dt = chrono::DateTime::from_timestamp(min_secs as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| min_secs.to_string());
        let max_dt = chrono::DateTime::from_timestamp(max_secs as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| max_secs.to_string());

        println!("Time range:     {} to {}", min_dt, max_dt);
        println!("Duration:       {:.2} hours", duration_hours);
    }

    println!("Coverage:       {:.2}%", result.coverage_pct);
    println!("Gaps found:     {}", result.gaps.len());

    if !result.gaps.is_empty() && verbose {
        println!("\n--- Gaps ---");
        for (i, gap) in result.gaps.iter().take(max_gaps).enumerate() {
            let start_dt = chrono::DateTime::from_timestamp((gap.start_ns / 1_000_000_000) as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| (gap.start_ns / 1_000_000_000).to_string());
            let end_dt = chrono::DateTime::from_timestamp((gap.end_ns / 1_000_000_000) as i64, 0)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
                .unwrap_or_else(|| (gap.end_ns / 1_000_000_000).to_string());

            println!(
                "  {}. {} → {} ({:.1} mins, {} missing)",
                i + 1,
                start_dt,
                end_dt,
                gap.duration_secs() / 60.0,
                gap.missing_intervals
            );
        }

        if result.gaps.len() > max_gaps {
            println!("  ... and {} more gaps", result.gaps.len() - max_gaps);
        }
    }
}
