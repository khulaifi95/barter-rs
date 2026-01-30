//! Deribit REST API Client for Options Data
//!
//! Implements the `DeribitClient` trait from `market_state.rs` for fetching
//! options chain data including greeks, open interest, and implied volatility.
//!
//! # API Endpoints Used
//! - `GET /public/get_instruments` - List available options contracts
//! - `GET /public/get_book_summary_by_currency` - Batch fetch OI/IV data
//!
//! # Rate Limiting
//! Deribit public API: 100 requests per second (non-matching engine).
//! This client handles HTTP 429 by returning `DeribitError::RateLimited`.

use reqwest::Client;
use serde::Deserialize;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;
use tracing::{debug, warn};

use crate::shared::market_state::{DeribitClient, DeribitError, OptionContract, OptionsChain};

// ============================================================================
// DERIBIT REST CLIENT
// ============================================================================

/// Deribit REST API client for fetching options chain data
pub struct DeribitRestClient {
    client: Client,
    base_url: String,
    /// Last fetch timestamp (Unix millis) for rate limiting
    last_fetch_ts: AtomicI64,
    /// Minimum interval between fetches (millis)
    min_fetch_interval_ms: i64,
}

impl DeribitRestClient {
    /// Create a new Deribit REST client with default settings
    pub fn new() -> Self {
        Self::with_config(
            "https://www.deribit.com/api/v2/public".to_string(),
            60_000, // 60 second minimum interval (optional caching)
        )
    }

    /// Create a new client with custom configuration
    pub fn with_config(base_url: String, min_fetch_interval_ms: i64) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to build HTTP client");

        Self {
            client,
            base_url,
            last_fetch_ts: AtomicI64::new(0),
            min_fetch_interval_ms,
        }
    }

    /// Map ticker symbol to Deribit currency (BTC/ETH)
    fn ticker_to_currency(ticker: &str) -> &'static str {
        let ticker_upper = ticker.to_uppercase();
        if ticker_upper.contains("ETH") {
            "ETH"
        } else {
            "BTC" // Default to BTC
        }
    }

    /// Check if we should throttle this request
    fn should_throttle(&self) -> bool {
        let now = chrono::Utc::now().timestamp_millis();
        let last = self.last_fetch_ts.load(Ordering::Relaxed);
        now - last < self.min_fetch_interval_ms
    }

    /// Update the last fetch timestamp
    fn update_last_fetch(&self) {
        let now = chrono::Utc::now().timestamp_millis();
        self.last_fetch_ts.store(now, Ordering::Relaxed);
    }

    /// Fetch instruments list for a currency
    async fn fetch_instruments(
        &self,
        currency: &str,
    ) -> Result<Vec<DeribitInstrument>, DeribitError> {
        let url = format!(
            "{}/get_instruments?currency={}&kind=option&expired=false",
            self.base_url, currency
        );

        debug!("Fetching Deribit instruments: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DeribitError::NetworkError(e.to_string()))?;

        // Check for rate limiting
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Deribit rate limited");
            return Err(DeribitError::RateLimited);
        }

        if !response.status().is_success() {
            return Err(DeribitError::NetworkError(format!(
                "HTTP {}",
                response.status()
            )));
        }

        let body: DeribitResponse<Vec<DeribitInstrument>> = response
            .json()
            .await
            .map_err(|e| DeribitError::ParseError(e.to_string()))?;

        Ok(body.result)
    }

    /// Fetch book summaries (OI, mark IV) for all options of a currency
    async fn fetch_book_summaries(
        &self,
        currency: &str,
    ) -> Result<Vec<DeribitBookSummary>, DeribitError> {
        let url = format!(
            "{}/get_book_summary_by_currency?currency={}&kind=option",
            self.base_url, currency
        );

        debug!("Fetching Deribit book summaries: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DeribitError::NetworkError(e.to_string()))?;

        // Check for rate limiting
        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            warn!("Deribit rate limited");
            return Err(DeribitError::RateLimited);
        }

        if !response.status().is_success() {
            return Err(DeribitError::NetworkError(format!(
                "HTTP {}",
                response.status()
            )));
        }

        let body: DeribitResponse<Vec<DeribitBookSummary>> = response
            .json()
            .await
            .map_err(|e| DeribitError::ParseError(e.to_string()))?;

        Ok(body.result)
    }

    /// Fetch ticker data for a specific instrument (includes greeks)
    async fn fetch_ticker(&self, instrument_name: &str) -> Result<DeribitTicker, DeribitError> {
        let url = format!(
            "{}/ticker?instrument_name={}",
            self.base_url, instrument_name
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DeribitError::NetworkError(e.to_string()))?;

        if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            return Err(DeribitError::RateLimited);
        }

        if !response.status().is_success() {
            return Err(DeribitError::NetworkError(format!(
                "HTTP {}",
                response.status()
            )));
        }

        let body: DeribitResponse<DeribitTicker> = response
            .json()
            .await
            .map_err(|e| DeribitError::ParseError(e.to_string()))?;

        Ok(body.result)
    }
}

impl Default for DeribitRestClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Number of top OI instruments to fetch greeks for (balance between accuracy and API calls)
const TOP_OI_GREEKS_COUNT: usize = 50;

impl DeribitClient for DeribitRestClient {
    async fn fetch_options_chain(&self, ticker: &str) -> Result<OptionsChain, DeribitError> {
        // Optional throttling - if you want to enforce minimum fetch interval
        // This is a soft check; callers can bypass by creating new clients
        if self.should_throttle() {
            debug!("Throttling Deribit request - using cached interval");
            // For now, we still proceed - Phase 2 would return cached data here
        }

        let currency = Self::ticker_to_currency(ticker);

        // Step 1: Fetch instruments (for strike/expiry info)
        let instruments = self.fetch_instruments(currency).await?;
        debug!("Fetched {} {} instruments", instruments.len(), currency);

        // Step 2: Fetch book summaries (for OI, mark IV)
        // NOTE: /get_book_summary_by_currency does NOT include greeks!
        let summaries = self.fetch_book_summaries(currency).await?;
        debug!("Fetched {} {} book summaries", summaries.len(), currency);

        // Update last fetch timestamp
        self.update_last_fetch();

        // Step 3: Build lookup map from summaries
        let summary_map: HashMap<&str, &DeribitBookSummary> = summaries
            .iter()
            .map(|s| (s.instrument_name.as_str(), s))
            .collect();

        // Step 4: Combine instruments with summaries (no greeks yet)
        let mut contracts = Vec::with_capacity(instruments.len());
        let timestamp = chrono::Utc::now().timestamp_millis();

        for instrument in instruments {
            // Skip if no summary data available
            let summary = match summary_map.get(instrument.instrument_name.as_str()) {
                Some(s) => *s,
                None => continue,
            };

            // Parse option type
            let is_call = instrument.option_type.to_lowercase() == "call";

            contracts.push(OptionContract {
                instrument_name: instrument.instrument_name,
                strike: instrument.strike,
                expiry: instrument.expiration_timestamp,
                is_call,
                open_interest: summary.open_interest.unwrap_or(0.0),
                mark_iv: summary.mark_iv.unwrap_or(0.0),
                // Greeks will be populated in step 5 for top OI contracts
                delta: 0.0,
                gamma: 0.0,
                vega: 0.0,
            });
        }

        // Step 5: Fetch greeks from /ticker for top N instruments by OI
        // This is necessary because /get_book_summary_by_currency doesn't include greeks
        contracts.sort_by(|a, b| {
            b.open_interest
                .partial_cmp(&a.open_interest)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        let top_instruments: Vec<String> = contracts
            .iter()
            .take(TOP_OI_GREEKS_COUNT)
            .map(|c| c.instrument_name.clone())
            .collect();

        debug!(
            "Fetching greeks for top {} instruments by OI",
            top_instruments.len()
        );

        // Fetch greeks concurrently for top instruments
        let ticker_futures: Vec<_> = top_instruments
            .iter()
            .map(|name| self.fetch_ticker(name))
            .collect();

        let ticker_results = futures::future::join_all(ticker_futures).await;

        // Build map of instrument -> greeks
        let mut greeks_map: HashMap<String, DeribitGreeks> = HashMap::new();
        let mut greeks_fetched = 0;
        for result in ticker_results {
            if let Ok(ticker_data) = result {
                if let Some(greeks) = ticker_data.greeks {
                    greeks_map.insert(ticker_data.instrument_name, greeks);
                    greeks_fetched += 1;
                }
            }
        }

        debug!(
            "Fetched greeks for {}/{} top OI instruments",
            greeks_fetched,
            top_instruments.len()
        );

        // Step 6: Merge greeks into contracts
        for contract in &mut contracts {
            if let Some(greeks) = greeks_map.get(&contract.instrument_name) {
                contract.delta = greeks.delta;
                contract.gamma = greeks.gamma;
                contract.vega = greeks.vega;
            }
        }

        debug!(
            "Built options chain with {} contracts for {} ({} with greeks)",
            contracts.len(),
            ticker,
            greeks_fetched
        );

        Ok(OptionsChain {
            contracts,
            timestamp,
        })
    }
}

// ============================================================================
// DERIBIT API RESPONSE STRUCTURES
// ============================================================================

/// Generic Deribit API response wrapper
#[derive(Debug, Deserialize)]
struct DeribitResponse<T> {
    result: T,
}

/// Deribit instrument (from /get_instruments)
#[derive(Debug, Deserialize)]
struct DeribitInstrument {
    instrument_name: String,
    strike: f64,
    expiration_timestamp: i64,
    option_type: String, // "call" or "put"
}

/// Deribit book summary (from /get_book_summary_by_currency)
#[derive(Debug, Deserialize)]
struct DeribitBookSummary {
    instrument_name: String,
    open_interest: Option<f64>,
    mark_iv: Option<f64>,
    #[serde(default)]
    #[allow(dead_code)]
    greeks: Option<DeribitGreeks>,
}

/// Greeks from Deribit API
#[derive(Debug, Deserialize)]
struct DeribitGreeks {
    delta: f64,
    gamma: f64,
    vega: f64,
    #[serde(default)]
    #[allow(dead_code)]
    theta: f64,
    #[serde(default)]
    #[allow(dead_code)]
    rho: f64,
}

/// Deribit ticker response (from /ticker - includes greeks)
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct DeribitTicker {
    instrument_name: String,
    open_interest: Option<f64>,
    mark_iv: Option<f64>,
    mark_price: Option<f64>,
    underlying_price: Option<f64>,
    greeks: Option<DeribitGreeks>,
}

// ============================================================================
// MOCK IMPLEMENTATION FOR TESTING
// ============================================================================

/// Mock Deribit client for testing
#[derive(Debug, Default)]
pub struct MockDeribitClient {
    /// Predefined chain to return
    pub mock_chain: Option<OptionsChain>,
    /// Error to return (if set, takes precedence over mock_chain)
    pub mock_error: Option<DeribitError>,
    /// Track number of calls
    pub call_count: std::sync::atomic::AtomicUsize,
}

impl MockDeribitClient {
    /// Create a mock client that returns an empty chain
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a mock client with a predefined chain
    pub fn with_chain(chain: OptionsChain) -> Self {
        Self {
            mock_chain: Some(chain),
            mock_error: None,
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Create a mock client that returns an error
    pub fn with_error(error: DeribitError) -> Self {
        Self {
            mock_chain: None,
            mock_error: Some(error),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }

    /// Create a mock chain with sample BTC options
    pub fn sample_btc_chain() -> OptionsChain {
        let now = chrono::Utc::now().timestamp_millis();
        let expiry = now + 7 * 24 * 60 * 60 * 1000; // 7 days from now

        OptionsChain {
            contracts: vec![
                OptionContract {
                    instrument_name: "BTC-27DEC24-100000-C".to_string(),
                    strike: 100000.0,
                    expiry,
                    is_call: true,
                    open_interest: 5000.0,
                    mark_iv: 55.0,
                    delta: 0.45,
                    gamma: 0.00002,
                    vega: 150.0,
                },
                OptionContract {
                    instrument_name: "BTC-27DEC24-100000-P".to_string(),
                    strike: 100000.0,
                    expiry,
                    is_call: false,
                    open_interest: 4500.0,
                    mark_iv: 54.0,
                    delta: -0.55,
                    gamma: 0.00002,
                    vega: 145.0,
                },
                OptionContract {
                    instrument_name: "BTC-27DEC24-95000-C".to_string(),
                    strike: 95000.0,
                    expiry,
                    is_call: true,
                    open_interest: 8000.0,
                    mark_iv: 52.0,
                    delta: 0.60,
                    gamma: 0.000018,
                    vega: 130.0,
                },
                OptionContract {
                    instrument_name: "BTC-27DEC24-95000-P".to_string(),
                    strike: 95000.0,
                    expiry,
                    is_call: false,
                    open_interest: 7000.0,
                    mark_iv: 51.0,
                    delta: -0.40,
                    gamma: 0.000018,
                    vega: 125.0,
                },
                OptionContract {
                    instrument_name: "BTC-27DEC24-105000-C".to_string(),
                    strike: 105000.0,
                    expiry,
                    is_call: true,
                    open_interest: 3000.0,
                    mark_iv: 58.0,
                    delta: 0.30,
                    gamma: 0.000015,
                    vega: 120.0,
                },
                OptionContract {
                    instrument_name: "BTC-27DEC24-105000-P".to_string(),
                    strike: 105000.0,
                    expiry,
                    is_call: false,
                    open_interest: 2500.0,
                    mark_iv: 57.0,
                    delta: -0.70,
                    gamma: 0.000015,
                    vega: 115.0,
                },
            ],
            timestamp: now,
        }
    }

    /// Get the number of times fetch was called
    pub fn get_call_count(&self) -> usize {
        self.call_count.load(std::sync::atomic::Ordering::Relaxed)
    }
}

impl DeribitClient for MockDeribitClient {
    async fn fetch_options_chain(&self, _ticker: &str) -> Result<OptionsChain, DeribitError> {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);

        // Return error if set
        if let Some(ref err) = self.mock_error {
            return Err(err.clone());
        }

        // Return mock chain or default empty chain
        Ok(self.mock_chain.clone().unwrap_or_default())
    }
}

// ============================================================================
// TESTS
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ticker_to_currency() {
        assert_eq!(DeribitRestClient::ticker_to_currency("BTCUSDT"), "BTC");
        assert_eq!(DeribitRestClient::ticker_to_currency("ETHUSDT"), "ETH");
        assert_eq!(DeribitRestClient::ticker_to_currency("btc"), "BTC");
        assert_eq!(DeribitRestClient::ticker_to_currency("eth"), "ETH");
        assert_eq!(DeribitRestClient::ticker_to_currency("XRP"), "BTC"); // Default
    }

    #[test]
    fn test_default_client_creation() {
        let client = DeribitRestClient::new();
        assert_eq!(client.base_url, "https://www.deribit.com/api/v2/public");
        assert_eq!(client.min_fetch_interval_ms, 60_000);
    }

    #[test]
    fn test_custom_client_creation() {
        let client = DeribitRestClient::with_config(
            "https://test.deribit.com/api/v2/public".to_string(),
            30_000,
        );
        assert_eq!(client.base_url, "https://test.deribit.com/api/v2/public");
        assert_eq!(client.min_fetch_interval_ms, 30_000);
    }

    #[test]
    fn test_mock_client_empty() {
        let mock = MockDeribitClient::new();
        assert!(mock.mock_chain.is_none());
        assert!(mock.mock_error.is_none());
    }

    #[test]
    fn test_mock_client_with_chain() {
        let chain = MockDeribitClient::sample_btc_chain();
        let mock = MockDeribitClient::with_chain(chain);
        assert!(mock.mock_chain.is_some());
        assert_eq!(mock.mock_chain.as_ref().unwrap().contracts.len(), 6);
    }

    #[test]
    fn test_mock_client_with_error() {
        let mock = MockDeribitClient::with_error(DeribitError::RateLimited);
        assert!(mock.mock_error.is_some());
    }

    #[test]
    fn test_sample_btc_chain() {
        let chain = MockDeribitClient::sample_btc_chain();
        assert_eq!(chain.contracts.len(), 6);

        // Check we have both calls and puts
        let calls: Vec<_> = chain.contracts.iter().filter(|c| c.is_call).collect();
        let puts: Vec<_> = chain.contracts.iter().filter(|c| !c.is_call).collect();
        assert_eq!(calls.len(), 3);
        assert_eq!(puts.len(), 3);

        // Check strikes
        let strikes: Vec<f64> = chain.contracts.iter().map(|c| c.strike).collect();
        assert!(strikes.contains(&95000.0));
        assert!(strikes.contains(&100000.0));
        assert!(strikes.contains(&105000.0));
    }

    #[tokio::test]
    async fn test_mock_client_fetch() {
        let chain = MockDeribitClient::sample_btc_chain();
        let mock = MockDeribitClient::with_chain(chain);

        let result = mock.fetch_options_chain("BTCUSDT").await;
        assert!(result.is_ok());

        let fetched_chain = result.unwrap();
        assert_eq!(fetched_chain.contracts.len(), 6);
        assert_eq!(mock.get_call_count(), 1);
    }

    #[tokio::test]
    async fn test_mock_client_fetch_error() {
        let mock = MockDeribitClient::with_error(DeribitError::RateLimited);

        let result = mock.fetch_options_chain("BTCUSDT").await;
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DeribitError::RateLimited));
    }

    #[tokio::test]
    async fn test_mock_client_call_counting() {
        let mock = MockDeribitClient::new();
        assert_eq!(mock.get_call_count(), 0);

        let _ = mock.fetch_options_chain("BTC").await;
        assert_eq!(mock.get_call_count(), 1);

        let _ = mock.fetch_options_chain("ETH").await;
        assert_eq!(mock.get_call_count(), 2);
    }

    #[test]
    fn test_deribit_error_display() {
        let network_err = DeribitError::NetworkError("connection refused".to_string());
        assert!(network_err.to_string().contains("connection refused"));

        let rate_err = DeribitError::RateLimited;
        assert!(rate_err.to_string().contains("Rate limited"));

        let parse_err = DeribitError::ParseError("invalid JSON".to_string());
        assert!(parse_err.to_string().contains("invalid JSON"));
    }

    #[test]
    fn test_throttle_logic() {
        let client = DeribitRestClient::with_config(
            "https://test.deribit.com".to_string(),
            1000, // 1 second throttle
        );

        // Initially should not throttle
        assert!(!client.should_throttle());

        // After updating timestamp, should throttle
        client.update_last_fetch();
        assert!(client.should_throttle());
    }

    #[test]
    fn test_option_contract_fields() {
        let contract = OptionContract {
            instrument_name: "BTC-27DEC24-100000-C".to_string(),
            strike: 100000.0,
            expiry: 1735257600000,
            is_call: true,
            open_interest: 5000.0,
            mark_iv: 55.0,
            delta: 0.45,
            gamma: 0.00002,
            vega: 150.0,
        };

        assert_eq!(contract.strike, 100000.0);
        assert!(contract.is_call);
        assert!(contract.delta > 0.0); // Call should have positive delta
    }

    /// Integration test - actually calls Deribit API to verify greeks are fetched
    /// Run with: cargo test -p barter-trading-tuis test_live_deribit_greeks -- --nocapture --ignored
    #[tokio::test]
    #[ignore] // Only run manually - makes real API calls
    async fn test_live_deribit_greeks() {
        use crate::shared::options_state::OptionsContextBuilder;

        let client = DeribitRestClient::with_config(
            "https://www.deribit.com/api/v2/public".to_string(),
            0, // No throttling for test
        );

        println!("Fetching BTC options chain from Deribit...");
        let result = client.fetch_options_chain("BTC").await;

        assert!(result.is_ok(), "Failed to fetch options chain: {:?}", result.err());

        let chain = result.unwrap();
        println!("Got {} contracts", chain.contracts.len());

        // Check we got contracts
        assert!(!chain.contracts.is_empty(), "No contracts fetched");

        // Count contracts with greeks (non-zero delta/gamma/vega)
        let with_greeks: Vec<_> = chain
            .contracts
            .iter()
            .filter(|c| c.delta.abs() > 0.0 || c.gamma > 0.0 || c.vega > 0.0)
            .collect();

        println!(
            "Contracts with greeks: {}/{} ({:.1}%)",
            with_greeks.len(),
            chain.contracts.len(),
            with_greeks.len() as f64 / chain.contracts.len() as f64 * 100.0
        );

        // We should have greeks for at least the top OI contracts
        assert!(
            with_greeks.len() >= 30,
            "Expected at least 30 contracts with greeks, got {}",
            with_greeks.len()
        );

        // Print some sample greeks
        println!("\nSample contracts with greeks:");
        for contract in with_greeks.iter().take(5) {
            println!(
                "  {} OI={:.0} delta={:.4} gamma={:.6} vega={:.2}",
                contract.instrument_name,
                contract.open_interest,
                contract.delta,
                contract.gamma,
                contract.vega
            );
        }

        // Now test the full GEX calculation
        println!("\n=== Testing GEX calculation ===");
        let spot = 93700.0; // Approximate current BTC price
        let builder = OptionsContextBuilder::new();
        let ctx = builder.build(&chain, spot);

        println!("OptionsContext results:");
        println!("  gamma_flip_price: ${:.0}", ctx.gamma_flip_price);
        println!("  gexp: ${:.2}M", ctx.gexp / 1_000_000.0);
        println!("  dexp: ${:.2}M", ctx.dexp / 1_000_000.0);
        println!("  vexp: ${:.2}M", ctx.vexp / 1_000_000.0);
        println!("  put_call_oi_ratio: {:.2}", ctx.put_call_oi_ratio);
        println!("  max_pain: ${:.0}", ctx.max_pain);

        // GEX should be non-zero if we have greeks
        assert!(
            ctx.gexp.abs() > 1.0 || with_greeks.iter().all(|c| c.gamma < 1e-10),
            "GEX should be non-zero when we have {} contracts with greeks. GEX={}",
            with_greeks.len(),
            ctx.gexp
        );
    }
}
