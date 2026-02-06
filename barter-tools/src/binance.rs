//! Binance Public Data API client.
//!
//! Downloads historical data from https://data.binance.vision/

use chrono::{Duration, NaiveDate};
use reqwest::Client;
use std::path::Path;
use thiserror::Error;
use tokio::fs;
use tokio::io::AsyncWriteExt;

/// Base URL for Binance Public Data
pub const BINANCE_DATA_URL: &str = "https://data.binance.vision";

/// Data types available for download
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DataType {
    /// 1-minute klines (OHLCV)
    Klines1m,
    /// Aggregated trades
    AggTrades,
    /// Funding rate history
    FundingRate,
    /// Liquidation snapshots
    Liquidations,
}

impl DataType {
    pub fn path_segment(&self) -> &'static str {
        match self {
            DataType::Klines1m => "klines",
            DataType::AggTrades => "aggTrades",
            DataType::FundingRate => "fundingRate",
            DataType::Liquidations => "liquidationSnapshot",
        }
    }

    pub fn interval(&self) -> Option<&'static str> {
        match self {
            DataType::Klines1m => Some("1m"),
            _ => None,
        }
    }
}

#[derive(Debug, Error)]
pub enum DownloadError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("File not found on server: {url}")]
    NotFound { url: String },

    #[error("Invalid date range: start {start} > end {end}")]
    InvalidDateRange { start: NaiveDate, end: NaiveDate },
}

/// Binance Public Data downloader
pub struct BinanceDownloader {
    client: Client,
    output_dir: std::path::PathBuf,
}

impl BinanceDownloader {
    pub fn new(output_dir: impl AsRef<Path>) -> Self {
        Self {
            client: Client::builder()
                .gzip(true)
                .build()
                .expect("Failed to create HTTP client"),
            output_dir: output_dir.as_ref().to_path_buf(),
        }
    }

    /// Build the URL for a daily data file.
    pub fn build_url(
        &self,
        symbol: &str,
        data_type: DataType,
        date: NaiveDate,
        futures: bool,
    ) -> String {
        let market = if futures { "futures/um" } else { "spot" };
        let date_str = date.format("%Y-%m-%d").to_string();

        match data_type {
            DataType::Klines1m => {
                format!(
                    "{}/data/{}/daily/klines/{}/1m/{}-1m-{}.zip",
                    BINANCE_DATA_URL, market, symbol, symbol, date_str
                )
            }
            DataType::AggTrades => {
                format!(
                    "{}/data/{}/daily/aggTrades/{}/{}-aggTrades-{}.zip",
                    BINANCE_DATA_URL, market, symbol, symbol, date_str
                )
            }
            DataType::FundingRate => {
                // Funding rate is monthly for futures
                let month_str = date.format("%Y-%m").to_string();
                format!(
                    "{}/data/futures/um/monthly/fundingRate/{}/{}-fundingRate-{}.zip",
                    BINANCE_DATA_URL, symbol, symbol, month_str
                )
            }
            DataType::Liquidations => {
                format!(
                    "{}/data/futures/um/daily/liquidationSnapshot/{}/{}-liquidationSnapshot-{}.zip",
                    BINANCE_DATA_URL, symbol, symbol, date_str
                )
            }
        }
    }

    /// Download a single file.
    pub async fn download_file(
        &self,
        url: &str,
        output_path: &Path,
    ) -> Result<bool, DownloadError> {
        // Skip if already exists
        if output_path.exists() {
            return Ok(false);
        }

        // Create parent directory
        if let Some(parent) = output_path.parent() {
            fs::create_dir_all(parent).await?;
        }

        // Download
        let response = self.client.get(url).send().await?;

        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(DownloadError::NotFound {
                url: url.to_string(),
            });
        }

        let bytes = response.error_for_status()?.bytes().await?;

        // Write to file
        let mut file = fs::File::create(output_path).await?;
        file.write_all(&bytes).await?;

        Ok(true)
    }

    /// Download data for a date range.
    pub async fn download_range(
        &self,
        symbol: &str,
        data_type: DataType,
        start_date: NaiveDate,
        end_date: NaiveDate,
        futures: bool,
    ) -> Result<DownloadStats, DownloadError> {
        if start_date > end_date {
            return Err(DownloadError::InvalidDateRange {
                start: start_date,
                end: end_date,
            });
        }

        let mut stats = DownloadStats::default();
        let mut current = start_date;

        while current <= end_date {
            let url = self.build_url(symbol, data_type, current, futures);
            let filename = url.rsplit('/').next().unwrap_or("unknown.zip");
            let output_path = self
                .output_dir
                .join(symbol)
                .join(data_type.path_segment())
                .join(filename);

            match self.download_file(&url, &output_path).await {
                Ok(true) => stats.downloaded += 1,
                Ok(false) => stats.skipped += 1,
                Err(DownloadError::NotFound { .. }) => stats.not_found += 1,
                Err(e) => {
                    stats.errors += 1;
                    tracing::warn!("Download failed for {}: {}", url, e);
                }
            }

            current += Duration::days(1);
        }

        Ok(stats)
    }

    /// Get output directory path for a symbol and data type.
    pub fn output_path(&self, symbol: &str, data_type: DataType) -> std::path::PathBuf {
        self.output_dir.join(symbol).join(data_type.path_segment())
    }
}

/// Statistics from a download operation.
#[derive(Debug, Default)]
pub struct DownloadStats {
    pub downloaded: u32,
    pub skipped: u32,
    pub not_found: u32,
    pub errors: u32,
}

impl std::fmt::Display for DownloadStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "Downloaded: {}, Skipped: {}, Not Found: {}, Errors: {}",
            self.downloaded, self.skipped, self.not_found, self.errors
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_klines_url() {
        let downloader = BinanceDownloader::new("/tmp");
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();

        let url = downloader.build_url("BTCUSDT", DataType::Klines1m, date, true);
        assert!(url.contains("futures/um"));
        assert!(url.contains("klines"));
        assert!(url.contains("BTCUSDT"));
        assert!(url.contains("2024-01-15"));
    }

    #[test]
    fn test_build_trades_url() {
        let downloader = BinanceDownloader::new("/tmp");
        let date = NaiveDate::from_ymd_opt(2024, 1, 15).unwrap();

        let url = downloader.build_url("ETHUSDT", DataType::AggTrades, date, true);
        assert!(url.contains("aggTrades"));
        assert!(url.contains("ETHUSDT"));
    }
}
