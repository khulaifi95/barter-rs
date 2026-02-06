//! CSV to Parquet Converter
//!
//! Converts Binance CSV files to Nautilus-compatible Parquet format.
//!
//! Usage:
//!   csv-to-parquet --input ./binance_data/BTCUSDT/klines --output ./parquet --symbol BTCUSDT
//!   csv-to-parquet --input ./data.csv --output ./output.parquet --symbol BTCUSDT --single-file

use barter_tools::convert::{
    convert_directory, convert_klines_to_parquet, convert_trades_to_parquet,
};
use barter_tools::PrecisionMode;
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(name = "csv-to-parquet")]
#[command(about = "Convert Binance CSV files to Nautilus Parquet format")]
struct Args {
    /// Input directory containing CSV/ZIP files, or a single file
    #[arg(short, long)]
    input: PathBuf,

    /// Output directory for Parquet files, or output file path for single file mode
    #[arg(short, long)]
    output: PathBuf,

    /// Symbol (e.g., BTCUSDT)
    #[arg(short, long)]
    symbol: String,

    /// Data type: klines, trades, aggTrades
    #[arg(long, default_value = "klines")]
    data_type: String,

    /// Price precision (decimal places)
    #[arg(long, default_value = "2")]
    price_precision: u8,

    /// Size precision (decimal places)
    #[arg(long, default_value = "3")]
    size_precision: u8,

    /// Process a single file instead of a directory
    #[arg(long)]
    single_file: bool,
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
    let precision_mode = PrecisionMode::from_env();

    println!("========================================");
    println!("CSV to Parquet Converter");
    println!("========================================");
    println!("Input:      {:?}", args.input);
    println!("Output:     {:?}", args.output);
    println!("Symbol:     {}", args.symbol);
    println!("Type:       {}", args.data_type);
    println!(
        "Precision:  {:?} ({}-byte)",
        precision_mode,
        if matches!(precision_mode, PrecisionMode::High) {
            16
        } else {
            8
        }
    );
    println!("Price prec: {}", args.price_precision);
    println!("Size prec:  {}", args.size_precision);
    println!("========================================\n");

    if args.single_file {
        // Single file mode
        let result = match args.data_type.as_str() {
            "klines" => convert_klines_to_parquet(
                &args.input,
                &args.output,
                &args.symbol,
                args.price_precision,
                args.size_precision,
            ),
            "trades" | "aggTrades" => convert_trades_to_parquet(
                &args.input,
                &args.output,
                &args.symbol,
                args.price_precision,
                args.size_precision,
            ),
            _ => anyhow::bail!("Unknown data type: {}", args.data_type),
        };

        match result {
            Ok(rows) => {
                println!("✓ Converted {} rows", rows);
                println!("  Output: {:?}", args.output);
            }
            Err(e) => {
                eprintln!("✗ Conversion failed: {}", e);
                std::process::exit(1);
            }
        }
    } else {
        // Directory mode
        if !args.input.is_dir() {
            anyhow::bail!("Input must be a directory (use --single-file for single file mode)");
        }

        let stats = convert_directory(
            &args.input,
            &args.output,
            &args.symbol,
            &args.data_type,
            args.price_precision,
            args.size_precision,
        )?;

        println!("========================================");
        println!("Conversion Complete");
        println!("========================================");
        println!("Files processed: {}", stats.files_processed);
        println!("Rows written:    {}", stats.rows_written);

        if !stats.errors.is_empty() {
            println!("\nErrors ({}):", stats.errors.len());
            for err in &stats.errors {
                println!("  - {}", err);
            }
        }
    }

    Ok(())
}
