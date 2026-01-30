//! Binance Public Data Downloader
//!
//! Downloads historical klines, trades, funding rates from data.binance.vision
//!
//! Usage:
//!   binance-downloader --symbol BTCUSDT --start 2024-01-01 --end 2024-12-31 --output ./data
//!
//! Data types:
//!   --klines    Download 1m klines (default)
//!   --trades    Download aggregated trades
//!   --funding   Download funding rate history
//!   --all       Download all data types

use barter_tools::binance::{BinanceDownloader, DataType};
use chrono::NaiveDate;
use clap::Parser;
use indicatif::{ProgressBar, ProgressStyle};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

#[derive(Parser, Debug)]
#[command(name = "binance-downloader")]
#[command(about = "Download historical data from Binance Public Data")]
struct Args {
    /// Symbol to download (e.g., BTCUSDT)
    #[arg(short, long)]
    symbol: String,

    /// Start date (YYYY-MM-DD)
    #[arg(long)]
    start: String,

    /// End date (YYYY-MM-DD)
    #[arg(long)]
    end: String,

    /// Output directory
    #[arg(short, long, default_value = "./binance_data")]
    output: PathBuf,

    /// Download klines
    #[arg(long)]
    klines: bool,

    /// Download trades
    #[arg(long)]
    trades: bool,

    /// Download funding rates
    #[arg(long)]
    funding: bool,

    /// Download all data types
    #[arg(long)]
    all: bool,

    /// Download spot data instead of futures (perps)
    #[arg(long)]
    spot: bool,

    /// Continue on errors
    #[arg(long)]
    continue_on_error: bool,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("info".parse()?))
        .init();

    let args = Args::parse();

    // Parse dates
    let start_date = NaiveDate::parse_from_str(&args.start, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid start date: {}", e))?;
    let end_date = NaiveDate::parse_from_str(&args.end, "%Y-%m-%d")
        .map_err(|e| anyhow::anyhow!("Invalid end date: {}", e))?;

    // Determine data types to download
    let mut data_types = Vec::new();
    if args.all {
        data_types.push(DataType::Klines1m);
        data_types.push(DataType::AggTrades);
        data_types.push(DataType::FundingRate);
    } else {
        if args.klines || (!args.trades && !args.funding) {
            data_types.push(DataType::Klines1m);
        }
        if args.trades {
            data_types.push(DataType::AggTrades);
        }
        if args.funding {
            data_types.push(DataType::FundingRate);
        }
    }

    let futures = !args.spot;

    println!("========================================");
    println!("Binance Public Data Downloader");
    println!("========================================");
    println!("Symbol:     {}", args.symbol);
    println!("Date range: {} to {}", start_date, end_date);
    println!("Market:     {}", if futures { "Futures (Perps)" } else { "Spot" });
    println!("Output:     {:?}", args.output);
    println!("Data types: {:?}", data_types);
    println!("========================================\n");

    // Create downloader
    let downloader = BinanceDownloader::new(&args.output);

    // Calculate total days for progress bar
    let total_days = (end_date - start_date).num_days() as u64 + 1;

    for data_type in data_types {
        println!("\nDownloading {:?}...", data_type);

        let pb = ProgressBar::new(total_days);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let stats = downloader
            .download_range(
                &args.symbol,
                data_type,
                start_date,
                end_date,
                futures,
            )
            .await?;

        pb.finish_with_message("Done");

        println!("  {}", stats);
        println!(
            "  Output: {:?}",
            downloader.output_path(&args.symbol, data_type)
        );
    }

    println!("\n========================================");
    println!("Download complete!");
    println!("========================================");

    Ok(())
}
