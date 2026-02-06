//! Parquet Verification Tool
//!
//! Validates Parquet file integrity and schema compliance.
//!
//! Usage:
//!   verify-parquet ./test.parquet
//!   verify-parquet ./parquet/bars_1m --type bars --recursive

use barter_tools::verify::{verify_bar_file, verify_directory, verify_trade_file};
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "verify-parquet")]
#[command(about = "Verify Parquet file integrity and schema")]
struct Args {
    /// Parquet file or directory to verify
    path: PathBuf,

    /// Data type: bars, trades
    #[arg(long, default_value = "bars")]
    r#type: String,

    /// Verify all files in directory recursively
    #[arg(short, long)]
    recursive: bool,

    /// Show detailed output
    #[arg(short, long)]
    verbose: bool,
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
    println!("Parquet Verification");
    println!("========================================");
    println!("Path:     {:?}", args.path);
    println!("Type:     {}", args.r#type);
    println!("========================================\n");

    let mut total_files = 0u64;
    let mut total_rows = 0u64;
    let mut total_errors = 0u64;
    let mut files_ok = 0u64;

    if args.path.is_file() {
        // Single file mode
        let result = match args.r#type.as_str() {
            "bars" | "klines" => verify_bar_file(&args.path),
            "trades" | "aggTrades" => verify_trade_file(&args.path),
            _ => anyhow::bail!("Unknown type: {}", args.r#type),
        };

        match result {
            Ok(r) => {
                total_files = 1;
                total_rows = r.rows;
                if r.is_ok() {
                    files_ok = 1;
                    println!("✓ {:?}", args.path);
                } else {
                    total_errors = 1;
                    println!("✗ {:?}", args.path);
                }
                if args.verbose || !r.is_ok() {
                    print_result(&r);
                }
            }
            Err(e) => {
                total_files = 1;
                total_errors = 1;
                println!("✗ {:?}: {}", args.path, e);
            }
        }
    } else if args.path.is_dir() {
        // Directory mode
        let results = verify_directory(&args.path, &args.r#type)?;

        for (path, result) in results {
            total_files += 1;
            total_rows += result.rows;

            if result.is_ok() {
                files_ok += 1;
                if args.verbose {
                    println!("✓ {:?} - {} rows", path, result.rows);
                }
            } else {
                total_errors += 1;
                println!("✗ {:?}", path);
                print_result(&result);
            }
        }
    } else {
        anyhow::bail!("Path does not exist: {:?}", args.path);
    }

    // Summary
    println!("\n========================================");
    println!("Summary");
    println!("========================================");
    println!("Files verified: {}", total_files);
    println!("Total rows:     {}", total_rows);
    println!("Files OK:       {}", files_ok);
    println!("Files with errors: {}", total_errors);

    if total_errors > 0 {
        println!(
            "\n⚠ Verification found {} file(s) with issues",
            total_errors
        );
        std::process::exit(1);
    } else {
        println!("\n✓ All files verified successfully!");
    }

    Ok(())
}

fn print_result(result: &barter_tools::verify::VerifyResult) {
    println!("  Rows:         {}", result.rows);
    println!("  Duplicates:   {}", result.duplicates);
    println!("  Out of order: {}", result.out_of_order);
    println!("  Nulls:        {}", result.nulls);

    if let (Some(min), Some(max)) = (result.min_ts_ns, result.max_ts_ns) {
        let min_dt = chrono::DateTime::from_timestamp((min / 1_000_000_000) as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| (min / 1_000_000_000).to_string());
        let max_dt = chrono::DateTime::from_timestamp((max / 1_000_000_000) as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S").to_string())
            .unwrap_or_else(|| (max / 1_000_000_000).to_string());
        println!("  Time range:   {} to {}", min_dt, max_dt);
    }

    for err in &result.errors {
        println!("  ERROR: {}", err);
    }
}
