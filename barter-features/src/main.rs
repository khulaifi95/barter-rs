//! barter-features CLI entry point.

use barter_features::{Config, Pipeline, Result};
use clap::Parser;
use std::path::PathBuf;
use tracing::{Level, info};
use tracing_subscriber::FmtSubscriber;

/// Feature computation layer for barter-data-server.
#[derive(Parser, Debug)]
#[command(name = "barter-features")]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Input directory containing parquet files from barter-data-server
    #[arg(long, default_value = "/data/parquet")]
    input: PathBuf,

    /// Output directory for computed features
    #[arg(long, default_value = "/data/features")]
    output: PathBuf,

    /// Processing mode: "watch" for continuous, "batch" for one-time
    #[arg(long, default_value = "watch")]
    mode: String,

    /// Path to configuration file
    #[arg(long)]
    config: Option<PathBuf>,

    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialize logging
    let level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let subscriber = FmtSubscriber::builder()
        .with_max_level(level)
        .with_target(false)
        .with_thread_ids(false)
        .with_file(false)
        .with_line_number(false)
        .finish();

    tracing::subscriber::set_global_default(subscriber).expect("Failed to set tracing subscriber");

    info!("barter-features starting...");
    info!("Input directory: {:?}", args.input);
    info!("Output directory: {:?}", args.output);
    info!("Mode: {}", args.mode);

    // Load configuration
    let mut config = if let Some(config_path) = &args.config {
        Config::from_file(config_path)?
    } else {
        Config::load().unwrap_or_else(|_| {
            info!("Using default configuration");
            Config::from_env().expect("Failed to create config from environment")
        })
    };

    // Override config with CLI args
    config.general.input_dir = args.input;
    config.general.output_dir = args.output;
    config.general.mode = args.mode.clone();

    // Create and run pipeline
    let mut pipeline = Pipeline::new(config)?;

    match args.mode.as_str() {
        "batch" => {
            info!("Running in batch mode");
            pipeline.run_batch().await?;
        }
        "watch" => {
            info!("Running in watch mode (Ctrl+C to stop)");
            pipeline.run_watch().await?;
        }
        _ => {
            return Err(barter_features::FeatureError::Config(format!(
                "Unknown mode: {}. Use 'batch' or 'watch'",
                args.mode
            )));
        }
    }

    info!("barter-features shutdown complete");
    Ok(())
}
