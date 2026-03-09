//! Main processing pipeline orchestration.

use crate::checkpoint::Checkpoint;
use crate::config::Config;
use crate::error::Result;
use crate::features::{
    EventDetector, LargeTrade, LargeTradeDetector, ProfileEvent, TpoBracket, TpoProcessor,
};
use crate::output::{FeatureWriter, OutputMetadata};
use crate::precision::PrecisionMode;
use crate::reader::{ExtendedBar, read_extended_bars, trade_instrument_id_from_schema};
use crate::watcher::{AsyncFileWatcher, scan_ready_files, scan_ready_files_since};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::SystemTime;
use tracing::{debug, error, info, warn};

/// Session key: (instrument_id, session_date)
type SessionKey = (String, String);
type LargeTradesOutput = (Vec<LargeTrade>, PrecisionMode, Vec<(PathBuf, u64)>);

/// Accumulator for an active session in watch mode.
struct SessionAccumulator {
    /// All bars received for this session
    bars: Vec<ExtendedBar>,
    /// TPO processor maintaining session state (for future incremental processing)
    #[allow(dead_code)]
    tpo_processor: TpoProcessor,
    /// Event detector (for future incremental processing)
    #[allow(dead_code)]
    event_detector: EventDetector,
    /// Last update timestamp (for timeout detection)
    last_update_ns: i64,
}

/// Main feature processing pipeline.
pub struct Pipeline {
    config: Config,
    checkpoint: Checkpoint,
    checkpoint_path: PathBuf,
    writer: FeatureWriter,
    /// Active sessions being accumulated (watch mode only)
    active_sessions: HashMap<SessionKey, SessionAccumulator>,
    /// Large trade detector
    trade_detector: LargeTradeDetector,
    /// Accumulated large trades per session (prevents data loss when processing multiple files)
    accumulated_large_trades: HashMap<SessionKey, Vec<LargeTrade>>,
    /// Detected precision mode per session for trades (for accurate metadata)
    session_trades_precision: HashMap<SessionKey, PrecisionMode>,
    /// Optional allowlist of instruments to process (normalized)
    allowed_instruments: Option<HashSet<String>>,
    /// Timestamp of last rescan (for incremental scanning in watch mode)
    last_rescan_time: SystemTime,
}

impl Pipeline {
    /// Create a new pipeline with the given configuration.
    pub fn new(config: Config) -> Result<Self> {
        let config_hash = config.config_hash();
        let checkpoint_path = config.general.checkpoint_dir.join(&config.checkpoint.file);

        let checkpoint = if config.checkpoint.enabled {
            Checkpoint::load_or_create(&checkpoint_path, &config_hash)?
        } else {
            Checkpoint::new(&config_hash)
        };

        // Extended bars are always Int64 × 1e9, so source_precision is always "standard"
        let metadata = OutputMetadata {
            schema_version: config.general.schema_version.clone(),
            config_hash: config_hash.clone(),
            source_precision: "standard".to_string(), // Extended bars are always 1e9
            output_precision: 9,
        };

        let writer = FeatureWriter::new(metadata);
        let trade_detector = LargeTradeDetector::new(&config.large_trades);
        let allowed_instruments = if config.filter.instruments.is_empty() {
            None
        } else {
            Some(
                config
                    .filter
                    .instruments
                    .iter()
                    .map(|v| normalize_instrument(v))
                    .collect(),
            )
        };

        info!(
            "Pipeline initialized: source_precision=standard, config_hash={}",
            config_hash
        );

        Ok(Self {
            config,
            checkpoint,
            checkpoint_path,
            writer,
            active_sessions: HashMap::new(),
            trade_detector,
            accumulated_large_trades: HashMap::new(),
            session_trades_precision: HashMap::new(),
            allowed_instruments,
            last_rescan_time: SystemTime::UNIX_EPOCH,
        })
    }

    fn is_instrument_allowed(&self, instrument_id: &str) -> bool {
        match &self.allowed_instruments {
            Some(allowlist) => allowlist.contains(&normalize_instrument(instrument_id)),
            None => true,
        }
    }

    /// Run the pipeline in batch mode (process existing files).
    pub async fn run_batch(&mut self) -> Result<()> {
        info!(
            "Starting batch processing from {:?}",
            self.config.general.input_dir
        );

        let files = scan_ready_files(
            &self.config.general.input_dir,
            self.config.watcher.stable_mtime_secs,
        );

        info!("Found {} files to process", files.len());

        // Group files by instrument and date for aggregated processing
        // Separate groups for extended_bars and trades
        let mut extended_bar_groups: std::collections::HashMap<(String, String), Vec<PathBuf>> =
            std::collections::HashMap::new();
        let mut trades_groups: std::collections::HashMap<(String, String), Vec<PathBuf>> =
            std::collections::HashMap::new();

        for file in files {
            let path_str = file.to_string_lossy();
            let instrument_id = extract_instrument_id(&file);
            let session_date = extract_session_date(&file);

            if !self.is_instrument_allowed(&instrument_id) {
                debug!("Skipping filtered instrument in batch: {}", instrument_id);
                continue;
            }

            if path_str.contains("trades") {
                // Trades files go to trades group
                trades_groups
                    .entry((instrument_id, session_date))
                    .or_default()
                    .push(file);
            } else if path_str.contains("extended_bars")
                || file.extension().map(|e| e == "parquet").unwrap_or(false)
            {
                // Extended bars and other parquet files go to extended_bar group
                extended_bar_groups
                    .entry((instrument_id, session_date))
                    .or_default()
                    .push(file);
            }
        }

        info!(
            "Grouped into {} extended_bar and {} trades instrument-date combinations",
            extended_bar_groups.len(),
            trades_groups.len()
        );

        // Process each extended_bar group (all files for one instrument-date together)
        for ((instrument_id, session_date), group_files) in &extended_bar_groups {
            info!(
                "Processing {} extended_bar files for {} on {}",
                group_files.len(),
                instrument_id,
                session_date
            );
            self.process_session_files(group_files, instrument_id, session_date)
                .await?;
        }

        // Process trades files for each instrument-date
        for ((instrument_id, session_date), trade_files) in &trades_groups {
            info!(
                "Processing {} trades files for {} on {}",
                trade_files.len(),
                instrument_id,
                session_date
            );
            self.process_session_trades_batch(trade_files, instrument_id, session_date)
                .await?;
        }

        // Save final checkpoint
        if self.config.checkpoint.enabled {
            self.checkpoint.save(&self.checkpoint_path)?;
        }

        info!("Batch processing complete");
        Ok(())
    }

    /// Process all files for a single session (instrument + date).
    async fn process_session_files(
        &mut self,
        files: &[PathBuf],
        instrument_id: &str,
        session_date: &str,
    ) -> Result<()> {
        // Read all bars from all files
        let mut all_bars = Vec::new();
        for file in files {
            match read_extended_bars(file) {
                Ok(bars) => all_bars.extend(bars),
                Err(e) => {
                    warn!("Failed to read {:?}: {}", file, e);
                    continue;
                }
            }
        }

        if all_bars.is_empty() {
            return Ok(());
        }

        // Sort bars by timestamp
        all_bars.sort_by_key(|b| b.ts_event);
        info!("Loaded {} total bars for session", all_bars.len());

        let output_instrument_id = output_instrument_id_from_bars(&all_bars, instrument_id);

        // Process through TPO
        let mut tpo_processor = TpoProcessor::new(self.config.tpo.clone());
        let mut event_detector = EventDetector::new(&self.config.profile);

        let mut all_brackets = tpo_processor.process_bars(&all_bars, &output_instrument_id)?;

        // Finalize any remaining bracket
        if let Some(final_bracket) = tpo_processor.finalize_session(&output_instrument_id)? {
            all_brackets.push(final_bracket);
        }

        // Detect events
        let mut all_events = Vec::new();
        for bracket in &all_brackets {
            let events = event_detector.process_bracket(bracket);
            all_events.extend(events);
        }

        // Create gap events
        let gaps = tpo_processor.get_gaps();
        if !gaps.is_empty() && !all_brackets.is_empty() {
            let session_id = &all_brackets[0].session_id;
            let gap_events = event_detector.create_gap_events(
                gaps,
                &output_instrument_id,
                session_id,
                &all_brackets[0],
            );
            all_events.extend(gap_events);
        }

        // Sort outputs by ts_event
        all_brackets.sort_by_key(|b| b.ts_event);
        all_events.sort_by_key(|e| e.ts_event);

        // Write outputs
        if !all_brackets.is_empty() {
            let output_dir = self
                .config
                .general
                .output_dir
                .join(instrument_id)
                .join(session_date);

            let tpo_path = output_dir.join("tpo_brackets.parquet");
            self.writer.write_tpo_brackets(&tpo_path, &all_brackets)?;
            info!(
                "Wrote {} TPO brackets to {:?}",
                all_brackets.len(),
                tpo_path
            );

            if !all_events.is_empty() {
                let events_path = output_dir.join("profile_events.parquet");
                self.writer
                    .write_profile_events(&events_path, &all_events)?;
                info!("Wrote {} events to {:?}", all_events.len(), events_path);
            }
        }

        // Update checkpoint for all processed files
        if self.config.checkpoint.enabled {
            let last_ts = all_brackets.last().map(|b| b.ts_event).unwrap_or(0);
            for file in files {
                self.checkpoint.mark_processed(
                    instrument_id,
                    file,
                    all_bars.len() as u64,
                    last_ts,
                )?;
            }
        }

        Ok(())
    }

    /// Run the pipeline in watch mode (continuous processing).
    pub async fn run_watch(&mut self) -> Result<()> {
        info!("Starting watch mode on {:?}", self.config.general.input_dir);

        // First process existing files
        self.run_batch().await?;

        // Then watch for new files
        let (_watcher, mut rx) = AsyncFileWatcher::new(
            self.config.general.input_dir.clone(),
            Some(self.config.watcher.stable_mtime_secs),
        )?;

        info!("Watching for new files...");

        let mut checkpoint_interval = tokio::time::interval(tokio::time::Duration::from_secs(
            self.config.checkpoint.save_interval_secs,
        ));

        // Session timeout: 15 minutes in nanoseconds (was 1 hour — reduced to limit memory)
        const SESSION_TIMEOUT_NS: i64 = 900 * 1_000_000_000;

        loop {
            tokio::select! {
                Some(path) = rx.recv() => {
                    if let Err(e) = self.process_file(&path).await {
                        error!("Error processing {:?}: {}", path, e);
                    }
                }
                _ = checkpoint_interval.tick() => {
                    // Save checkpoint
                    if self.config.checkpoint.enabled
                        && let Err(e) = self.checkpoint.save(&self.checkpoint_path)
                    {
                        warn!("Failed to save checkpoint: {}", e);
                    }

                    // Clean up old sessions to prevent unbounded memory growth
                    let sessions_before = self.active_sessions.len();
                    let trades_before = self.accumulated_large_trades.len();
                    if let Err(e) = self.finalize_old_sessions(SESSION_TIMEOUT_NS) {
                        warn!("Failed to finalize old sessions: {}", e);
                    }
                    let sessions_after = self.active_sessions.len();
                    let trades_after = self.accumulated_large_trades.len();
                    if sessions_before != sessions_after || trades_before != trades_after {
                        info!(
                            "Session cleanup: {} -> {} active sessions, {} -> {} large trade accumulators",
                            sessions_before, sessions_after, trades_before, trades_after
                        );
                    }

                    // Safety net: incremental rescan — only descend into dirs
                    // modified since last scan (skips ~99% of the 62K file tree).
                    let scan_cutoff = self.last_rescan_time;
                    let rescan_files = scan_ready_files_since(
                        &self.config.general.input_dir,
                        self.config.watcher.stable_mtime_secs,
                        scan_cutoff,
                        0,
                    );
                    let mut scanned = 0u32;
                    for path in rescan_files {
                        scanned += 1;
                        if let Err(e) = self.process_file(&path).await {
                            error!("Error processing {:?} during rescan: {}", path, e);
                        }
                    }
                    self.last_rescan_time = SystemTime::now();
                    if scanned > 0 {
                        debug!("Incremental rescan processed {} new files", scanned);
                    }
                }
                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                    break;
                }
            }
        }

        // Save final checkpoint
        if self.config.checkpoint.enabled {
            self.checkpoint.save(&self.checkpoint_path)?;
        }

        Ok(())
    }

    /// Process a single file.
    async fn process_file(&mut self, path: &Path) -> Result<()> {
        // Extract instrument ID from path
        let instrument_id = extract_instrument_id(path);
        if !self.is_instrument_allowed(&instrument_id) {
            debug!("Skipping filtered instrument: {}", instrument_id);
            return Ok(());
        }

        // Check if already processed
        if self.config.checkpoint.enabled && self.checkpoint.is_file_processed(&instrument_id, path)
        {
            debug!("Skipping already processed file: {:?}", path);
            return Ok(());
        }

        info!("Processing {:?} for {}", path, instrument_id);

        // Determine file type from path (check parent directories)
        let path_str = path.to_string_lossy();

        if path_str.contains("extended_bars") {
            self.process_extended_bars_file(path, &instrument_id)
                .await?;
        } else if path_str.contains("trades") {
            self.process_trades_file(path, &instrument_id).await?;
        } else if path_str.contains("bars_1m") {
            // Standard bars - skip for now (extended_bars has more data)
            debug!("Skipping standard bars file: {:?}", path);
            return Ok(());
        } else {
            // Default: try as extended bars if it's a .parquet file
            if path.extension().map(|e| e == "parquet").unwrap_or(false) {
                debug!("Processing as extended bars (default): {:?}", path);
                self.process_extended_bars_file(path, &instrument_id)
                    .await?;
            } else {
                debug!("Skipping unknown file type: {:?}", path);
                return Ok(());
            }
        }

        Ok(())
    }

    async fn process_extended_bars_file(&mut self, path: &Path, instrument_id: &str) -> Result<()> {
        let mut bars = read_extended_bars(path)?;
        if bars.is_empty() {
            return Ok(());
        }

        // Sort bars by timestamp
        bars.sort_by_key(|b| b.ts_event);
        let bars_count = bars.len();
        info!("Read {} bars from {:?}", bars_count, path);

        // Extract session date from first bar
        let session_date = extract_date_from_timestamp(bars[0].ts_event);
        let session_key = (instrument_id.to_string(), session_date.clone());
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        // Hydrate session from disk on first sight to avoid overwriting full outputs
        if !self.active_sessions.contains_key(&session_key) {
            let session_dir = path.parent().unwrap_or(path);
            let mut all_bars = load_session_bars(session_dir)?;
            if all_bars.is_empty() {
                all_bars = bars;
            } else {
                all_bars.sort_by_key(|b| b.ts_event);
            }

            self.active_sessions.insert(
                session_key.clone(),
                SessionAccumulator {
                    bars: all_bars,
                    tpo_processor: TpoProcessor::new(self.config.tpo.clone()),
                    event_detector: EventDetector::new(&self.config.profile),
                    last_update_ns: now_ns,
                },
            );
        } else {
            // Add new bars to existing accumulator
            if let Some(accumulator) = self.active_sessions.get_mut(&session_key) {
                accumulator.bars.extend(bars);
                accumulator.last_update_ns = now_ns;
                accumulator.bars.sort_by_key(|b| b.ts_event);
            }
        }

        // Borrow accumulated bars as a slice — no clone needed since compute_session_output
        // only requires &self (shared borrow), same as active_sessions.get().
        let (all_brackets, all_events) = {
            let bars = self
                .active_sessions
                .get(&session_key)
                .map(|acc| acc.bars.as_slice())
                .unwrap_or(&[]);
            self.compute_session_output(bars, instrument_id, &session_date)?
        };

        // Write outputs (overwrites previous - this is correct for incremental updates)
        if !all_brackets.is_empty() {
            let output_dir = self
                .config
                .general
                .output_dir
                .join(instrument_id)
                .join(&session_date);

            let tpo_path = output_dir.join("tpo_brackets.parquet");
            self.writer.write_tpo_brackets(&tpo_path, &all_brackets)?;
            info!(
                "Wrote {} TPO brackets to {:?} (session accumulation)",
                all_brackets.len(),
                tpo_path
            );

            if !all_events.is_empty() {
                let events_path = output_dir.join("profile_events.parquet");
                self.writer
                    .write_profile_events(&events_path, &all_events)?;
                info!("Wrote {} events to {:?}", all_events.len(), events_path);
            }
        }

        // Update checkpoint
        if self.config.checkpoint.enabled {
            let last_ts = all_brackets.last().map(|b| b.ts_event).unwrap_or(0);
            self.checkpoint
                .mark_processed(instrument_id, path, bars_count as u64, last_ts)?;
        }

        Ok(())
    }

    /// Compute complete session output from accumulated bars.
    fn compute_session_output(
        &self,
        bars: &[ExtendedBar],
        instrument_id: &str,
        _session_date: &str,
    ) -> Result<(Vec<TpoBracket>, Vec<ProfileEvent>)> {
        if bars.is_empty() {
            return Ok((Vec::new(), Vec::new()));
        }

        let output_instrument_id = output_instrument_id_from_bars(bars, instrument_id);

        // Create fresh processors and compute from all accumulated bars
        let mut tpo_processor = TpoProcessor::new(self.config.tpo.clone());
        let mut event_detector = EventDetector::new(&self.config.profile);

        let mut all_brackets = tpo_processor.process_bars(bars, &output_instrument_id)?;

        // Finalize any remaining bracket
        if let Some(final_bracket) = tpo_processor.finalize_session(&output_instrument_id)? {
            all_brackets.push(final_bracket);
        }

        // Detect events
        let mut all_events = Vec::new();
        for bracket in &all_brackets {
            let events = event_detector.process_bracket(bracket);
            all_events.extend(events);
        }

        // Create gap events
        let gaps = tpo_processor.get_gaps();
        if !gaps.is_empty() && !all_brackets.is_empty() {
            let session_id = &all_brackets[0].session_id;
            let gap_events = event_detector.create_gap_events(
                gaps,
                &output_instrument_id,
                session_id,
                &all_brackets[0],
            );
            all_events.extend(gap_events);
        }

        // Sort outputs by ts_event
        all_brackets.sort_by_key(|b| b.ts_event);
        all_events.sort_by_key(|e| e.ts_event);

        Ok((all_brackets, all_events))
    }

    /// Finalize and clear old sessions (call periodically in watch mode).
    ///
    /// Cleans up `active_sessions`, `accumulated_large_trades`, and `session_trades_precision`
    /// for sessions that haven't been updated within `max_age_ns` nanoseconds.
    pub fn finalize_old_sessions(&mut self, max_age_ns: i64) -> Result<()> {
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let cutoff = now_ns - max_age_ns;

        // Find old session keys based on active_sessions timestamps
        let old_keys: Vec<SessionKey> = self
            .active_sessions
            .iter()
            .filter(|(_, acc)| acc.last_update_ns < cutoff)
            .map(|(k, _)| k.clone())
            .collect();

        for key in &old_keys {
            info!("Finalizing old session: {:?}", key);
            self.active_sessions.remove(key);
            // Also clean up accumulated large trades and precision tracking for this session
            if self.accumulated_large_trades.remove(key).is_some() {
                info!("Cleaned up accumulated large trades for session: {:?}", key);
            }
            self.session_trades_precision.remove(key);
        }

        // Also clean up orphan large trade accumulators (sessions with no active_session entry)
        // This handles cases where trades were processed but no extended bars were received
        let orphan_trade_keys: Vec<SessionKey> = self
            .accumulated_large_trades
            .keys()
            .filter(|k| !self.active_sessions.contains_key(*k))
            .cloned()
            .collect();

        for key in orphan_trade_keys {
            info!("Cleaning up orphan large trade accumulator: {:?}", key);
            self.accumulated_large_trades.remove(&key);
            self.session_trades_precision.remove(&key);
        }

        Ok(())
    }

    async fn process_trades_file(&mut self, path: &Path, instrument_id: &str) -> Result<()> {
        let session_date = extract_session_date(path);
        let session_key = (instrument_id.to_string(), session_date.clone());
        let mut needs_write = false;
        let output_instrument_id =
            trade_instrument_id_from_schema(path)?.unwrap_or_else(|| instrument_id.to_string());

        // Hydrate from disk on first sight to avoid overwriting full outputs
        if !self.accumulated_large_trades.contains_key(&session_key) {
            let session_dir = path.parent().unwrap_or(path);
            let files = scan_session_parquet_files(session_dir);
            let (mut accumulated, precision, file_rows) =
                self.compute_large_trades_from_files(&files, &output_instrument_id)?;

            // Track precision for this session
            self.session_trades_precision
                .insert(session_key.clone(), precision);

            // Store accumulated large trades
            accumulated.sort_by_key(|t| t.ts_event);
            self.accumulated_large_trades
                .insert(session_key.clone(), accumulated);
            needs_write = self
                .accumulated_large_trades
                .get(&session_key)
                .map(|v| !v.is_empty())
                .unwrap_or(false);

            // Update checkpoint for all files in the session dir
            if self.config.checkpoint.enabled {
                let last_ts = self
                    .accumulated_large_trades
                    .get(&session_key)
                    .and_then(|v| v.last())
                    .map(|t| t.ts_event)
                    .unwrap_or(0);
                for (file, rows) in file_rows {
                    self.checkpoint
                        .mark_processed(instrument_id, &file, rows, last_ts)?;
                }
            }
        } else {
            // Incremental update from this file only — stream trades, never hold full file
            let mut reader = crate::reader::trades::TradeReader::open(path)?;
            let precision = reader.precision_mode();

            self.session_trades_precision
                .entry(session_key.clone())
                .and_modify(|existing| {
                    if precision == PrecisionMode::High {
                        *existing = PrecisionMode::High;
                    }
                })
                .or_insert(precision);

            let mut new_large_trades = Vec::new();
            let mut trade_count = 0u64;
            while let Some(trade) = reader.next()? {
                trade_count += 1;
                if let Some(large_trade) = self.trade_detector.process_trade(
                    trade.ts_event,
                    &output_instrument_id,
                    &trade.trade_id,
                    trade.price,
                    trade.size,
                    &trade.side,
                ) {
                    new_large_trades.push(large_trade);
                }
            }

            if trade_count == 0 {
                return Ok(());
            }

            info!(
                "Streamed {} trades from {:?} (precision: {:?})",
                trade_count, path, precision
            );

            if !new_large_trades.is_empty() {
                let accumulated = self
                    .accumulated_large_trades
                    .entry(session_key.clone())
                    .or_default();
                accumulated.extend(new_large_trades);
                accumulated.sort_by_key(|t| t.ts_event);
                needs_write = true;
            }

            if self.config.checkpoint.enabled {
                let last_ts = self
                    .accumulated_large_trades
                    .get(&session_key)
                    .and_then(|v| v.last())
                    .map(|t| t.ts_event)
                    .unwrap_or(0);
                self.checkpoint
                    .mark_processed(instrument_id, path, trade_count, last_ts)?;
            }
        }

        // Skip rewriting if no new large trades were added this cycle
        if !needs_write {
            return Ok(());
        }

        // Write output if we have accumulated large trades
        let accumulated = match self.accumulated_large_trades.get(&session_key) {
            Some(trades) if !trades.is_empty() => trades,
            _ => return Ok(()),
        };

        let source_precision = self
            .session_trades_precision
            .get(&session_key)
            .map(|p| p.as_str())
            .unwrap_or("standard");

        let output_dir = self
            .config
            .general
            .output_dir
            .join(instrument_id)
            .join(&session_date);

        let trades_path = output_dir.join("large_trades.parquet");
        self.writer.write_large_trades_with_precision(
            &trades_path,
            accumulated,
            source_precision,
        )?;
        info!(
            "Wrote {} large trades to {:?} (session accumulation, source_precision={})",
            accumulated.len(),
            trades_path,
            source_precision
        );

        Ok(())
    }

    /// Detect large trades by streaming through files one at a time.
    ///
    /// Instead of loading all trades into memory, reads each file with a streaming
    /// reader and processes trades individually through the detector.
    fn compute_large_trades_from_files(
        &self,
        files: &[PathBuf],
        output_instrument_id: &str,
    ) -> Result<LargeTradesOutput> {
        let mut large_trades = Vec::new();
        let mut detected_precision = PrecisionMode::Standard;
        let mut file_rows = Vec::new();

        for file in files {
            match crate::reader::trades::TradeReader::open(file) {
                Ok(mut reader) => {
                    let precision = reader.precision_mode();
                    if precision == PrecisionMode::High {
                        detected_precision = PrecisionMode::High;
                    }
                    let mut row_count = 0u64;
                    // Stream trades one at a time — never hold full file in memory
                    while let Some(trade) = reader.next()? {
                        row_count += 1;
                        if let Some(large_trade) = self.trade_detector.process_trade(
                            trade.ts_event,
                            output_instrument_id,
                            &trade.trade_id,
                            trade.price,
                            trade.size,
                            &trade.side,
                        ) {
                            large_trades.push(large_trade);
                        }
                    }
                    file_rows.push((file.clone(), row_count));
                }
                Err(e) => {
                    warn!("Failed to read trades from {:?}: {}", file, e);
                }
            }
        }

        large_trades.sort_by_key(|t| t.ts_event);

        Ok((large_trades, detected_precision, file_rows))
    }

    /// Process all trades files for a single session in batch mode.
    ///
    /// This aggregates all trades from multiple files for the same instrument-date,
    /// detects large trades, and writes a single `large_trades.parquet` output file.
    async fn process_session_trades_batch(
        &mut self,
        files: &[PathBuf],
        instrument_id: &str,
        session_date: &str,
    ) -> Result<()> {
        let output_instrument_id = files
            .first()
            .and_then(|file| trade_instrument_id_from_schema(file).ok().flatten())
            .unwrap_or_else(|| instrument_id.to_string());
        // Stream trades from all files — never hold all trades in memory
        let mut large_trades = Vec::new();
        let mut detected_precision = PrecisionMode::Standard;
        let mut total_trade_count = 0u64;

        for file in files {
            match crate::reader::trades::TradeReader::open(file) {
                Ok(mut reader) => {
                    let precision = reader.precision_mode();
                    if precision == PrecisionMode::High {
                        detected_precision = PrecisionMode::High;
                    }
                    while let Some(trade) = reader.next()? {
                        total_trade_count += 1;
                        if let Some(large_trade) = self.trade_detector.process_trade(
                            trade.ts_event,
                            &output_instrument_id,
                            &trade.trade_id,
                            trade.price,
                            trade.size,
                            &trade.side,
                        ) {
                            large_trades.push(large_trade);
                        }
                    }
                }
                Err(e) => {
                    warn!("Failed to read trades from {:?}: {}", file, e);
                    continue;
                }
            }
        }

        if total_trade_count == 0 {
            return Ok(());
        }

        info!(
            "Streamed {} total trades for session (precision: {:?})",
            total_trade_count, detected_precision
        );

        info!(
            "Detected {} large trades from {} total trades",
            large_trades.len(),
            total_trade_count
        );

        // Skip writing if no large trades detected
        if large_trades.is_empty() {
            debug!(
                "No large trades detected for session {}/{}",
                instrument_id, session_date
            );
            // Update checkpoint for all processed files
            if self.config.checkpoint.enabled {
                for file in files {
                    self.checkpoint
                        .mark_processed(instrument_id, file, total_trade_count, 0)?;
                }
            }
            return Ok(());
        }

        // Sort large trades by timestamp
        large_trades.sort_by_key(|t| t.ts_event);

        // Write output with correct source precision
        let output_dir = self
            .config
            .general
            .output_dir
            .join(instrument_id)
            .join(session_date);

        let trades_path = output_dir.join("large_trades.parquet");
        self.writer.write_large_trades_with_precision(
            &trades_path,
            &large_trades,
            detected_precision.as_str(),
        )?;
        info!(
            "Wrote {} large trades to {:?} (source_precision={})",
            large_trades.len(),
            trades_path,
            detected_precision.as_str()
        );

        // Update checkpoint for all processed files
        if self.config.checkpoint.enabled {
            let last_ts = large_trades.last().map(|t| t.ts_event).unwrap_or(0);
            for file in files {
                self.checkpoint
                    .mark_processed(instrument_id, file, total_trade_count, last_ts)?;
            }
        }

        Ok(())
    }
}

/// Extract instrument ID from a file path.
///
/// Expected path format: `.../instrument_id/date/filename.parquet`
fn extract_instrument_id(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

/// Normalize instrument IDs to a stable form for filtering.
fn normalize_instrument(value: &str) -> String {
    value.replace(['.', '-'], "_").to_uppercase()
}

fn output_instrument_id_from_bars(bars: &[ExtendedBar], fallback: &str) -> String {
    bars.iter()
        .find(|bar| !bar.instrument_id.is_empty())
        .map(|bar| bar.instrument_id.clone())
        .unwrap_or_else(|| fallback.to_string())
}

#[cfg(test)]
mod instrument_id_tests {
    use super::output_instrument_id_from_bars;
    use crate::reader::ExtendedBar;

    #[test]
    fn test_output_instrument_id_from_bars_prefers_bar_value() {
        let bars = vec![ExtendedBar {
            ts_event: 1,
            ts_init: 1,
            ts_open: 0,
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            open: 0.0,
            high: 0.0,
            low: 0.0,
            close: 0.0,
            volume: 0.0,
            quote_volume: 0.0,
            trade_count: 0,
            buy_volume: 0.0,
            sell_volume: 0.0,
            delta: 0.0,
            cvd: 0.0,
            open_interest: 0.0,
            oi_change: 0.0,
            funding_rate: 0.0,
        }];

        let output = output_instrument_id_from_bars(&bars, "FALLBACK");
        assert_eq!(output, "BTCUSDT-PERP.BINANCE");
    }
}

fn scan_session_parquet_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "parquet").unwrap_or(false)
                && !path.to_string_lossy().contains(".tmp")
            {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn load_session_bars(dir: &Path) -> Result<Vec<ExtendedBar>> {
    let mut all_bars = Vec::new();
    for file in scan_session_parquet_files(dir) {
        match read_extended_bars(&file) {
            Ok(mut bars) => all_bars.append(&mut bars),
            Err(e) => {
                warn!("Failed to read extended bars from {:?}: {}", file, e);
            }
        }
    }
    Ok(all_bars)
}

/// Extract session date from a file path.
///
/// Expected path format: `.../instrument_id/date/filename.parquet`
fn extract_session_date(path: &Path) -> String {
    path.parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| "UNKNOWN".to_string())
}

/// Extract date string from timestamp (nanoseconds).
fn extract_date_from_timestamp(ts_nanos: i64) -> String {
    use chrono::{DateTime, Utc};
    let secs = ts_nanos / 1_000_000_000;
    DateTime::from_timestamp(secs, 0)
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
        .format("%Y-%m-%d")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_instrument_id() {
        let path = Path::new("/data/raw/BTCUSDT-PERP.BINANCE/2024-02-02/extended_bars_1m.parquet");
        assert_eq!(extract_instrument_id(path), "BTCUSDT-PERP.BINANCE");
    }

    #[test]
    fn test_extract_instrument_id_short_path() {
        let path = Path::new("test.parquet");
        assert_eq!(extract_instrument_id(path), "UNKNOWN");
    }

    #[test]
    fn test_extract_date_from_timestamp() {
        // Test various nanosecond timestamps
        // 2024-02-02 00:00:00 UTC = 1706832000 seconds
        let ts_midnight = 1_706_832_000_000_000_000_i64;
        assert_eq!(extract_date_from_timestamp(ts_midnight), "2024-02-02");

        // 2024-02-02 12:30:45 UTC (midday)
        let ts_midday = (1706832000_i64 + 12 * 3600 + 30 * 60 + 45) * 1_000_000_000;
        assert_eq!(extract_date_from_timestamp(ts_midday), "2024-02-02");

        // 2024-02-02 23:59:59 UTC (end of day)
        let ts_end_of_day = (1706832000_i64 + 23 * 3600 + 59 * 60 + 59) * 1_000_000_000;
        assert_eq!(extract_date_from_timestamp(ts_end_of_day), "2024-02-02");

        // 2024-02-03 00:00:00 UTC (next day)
        let ts_next_day = (1706832000_i64 + 24 * 3600) * 1_000_000_000;
        assert_eq!(extract_date_from_timestamp(ts_next_day), "2024-02-03");

        // Edge case: timestamp 0 (1970-01-01)
        assert_eq!(extract_date_from_timestamp(0), "1970-01-01");
    }

    #[test]
    fn test_session_key_grouping() {
        // Test that files are grouped correctly by instrument+date
        let files = vec![
            PathBuf::from("/data/raw/BTCUSDT-PERP.BINANCE/2024-02-02/extended_bars_1m_00.parquet"),
            PathBuf::from("/data/raw/BTCUSDT-PERP.BINANCE/2024-02-02/extended_bars_1m_01.parquet"),
            PathBuf::from("/data/raw/BTCUSDT-PERP.BINANCE/2024-02-03/extended_bars_1m_00.parquet"),
            PathBuf::from("/data/raw/ETHUSDT-PERP.BINANCE/2024-02-02/extended_bars_1m_00.parquet"),
        ];

        // Simulate the grouping logic from run_batch
        let mut groups: std::collections::HashMap<(String, String), Vec<PathBuf>> =
            std::collections::HashMap::new();

        for file in files {
            let path_str = file.to_string_lossy();
            if path_str.contains("extended_bars") {
                let instrument_id = extract_instrument_id(&file);
                let session_date = extract_session_date(&file);
                groups
                    .entry((instrument_id, session_date))
                    .or_default()
                    .push(file);
            }
        }

        // Should have 3 groups
        assert_eq!(groups.len(), 3);

        // Check BTCUSDT 2024-02-02 has 2 files
        let btc_feb02 = groups
            .get(&("BTCUSDT-PERP.BINANCE".to_string(), "2024-02-02".to_string()))
            .unwrap();
        assert_eq!(btc_feb02.len(), 2);

        // Check BTCUSDT 2024-02-03 has 1 file
        let btc_feb03 = groups
            .get(&("BTCUSDT-PERP.BINANCE".to_string(), "2024-02-03".to_string()))
            .unwrap();
        assert_eq!(btc_feb03.len(), 1);

        // Check ETHUSDT 2024-02-02 has 1 file
        let eth_feb02 = groups
            .get(&("ETHUSDT-PERP.BINANCE".to_string(), "2024-02-02".to_string()))
            .unwrap();
        assert_eq!(eth_feb02.len(), 1);
    }

    /// Helper function to create test bars (same pattern as tpo.rs)
    fn make_test_bar(ts_event: i64, price: f64, volume: f64) -> ExtendedBar {
        ExtendedBar {
            ts_event,
            ts_init: ts_event,
            ts_open: ts_event - 60_000_000_000, // 1 minute before close
            instrument_id: "BTCUSDT-PERP.BINANCE".to_string(),
            open: price,
            high: price + 10.0,
            low: price - 10.0,
            close: price,
            volume,
            quote_volume: price * volume,
            trade_count: 100,
            buy_volume: volume * 0.6,
            sell_volume: volume * 0.4,
            delta: volume * 0.2,
            cvd: 0.0,
            open_interest: 0.0,
            oi_change: 0.0,
            funding_rate: 0.0,
        }
    }

    /// Helper to create a minimal test config
    fn make_test_config() -> Config {
        Config {
            general: crate::config::GeneralConfig {
                schema_version: "1.2.0".to_string(),
                input_dir: PathBuf::from("/tmp/test_input"),
                output_dir: PathBuf::from("/tmp/test_output"),
                checkpoint_dir: PathBuf::from("/tmp/test_checkpoint"),
                mode: "batch".to_string(),
            },
            filter: crate::config::FilterConfig::default(),
            precision: crate::config::PrecisionConfig::default(),
            session: crate::config::SessionConfig::default(),
            tpo: crate::config::TpoConfig::default(),
            large_trades: crate::config::LargeTradesConfig::default(),
            profile: crate::config::ProfileConfig::default(),
            gaps: crate::config::GapsConfig::default(),
            watcher: crate::config::WatcherConfig::default(),
            checkpoint: crate::config::CheckpointConfig {
                enabled: false, // Disable checkpoint for tests
                ..Default::default()
            },
            writer: crate::config::WriterConfig::default(),
        }
    }

    #[test]
    fn test_compute_session_output_empty_bars() {
        let config = make_test_config();
        let pipeline = Pipeline::new(config).unwrap();

        let empty_bars: Vec<ExtendedBar> = vec![];
        let (brackets, events) = pipeline
            .compute_session_output(&empty_bars, "BTCUSDT", "2024-02-02")
            .unwrap();

        assert!(
            brackets.is_empty(),
            "Empty input should produce no brackets"
        );
        assert!(events.is_empty(), "Empty input should produce no events");
    }

    #[test]
    fn test_compute_session_output_single_bar() {
        let config = make_test_config();
        let pipeline = Pipeline::new(config).unwrap();

        // Create a single bar at 00:00:00 UTC on 2024-02-02
        let base_ts: i64 = 1_706_832_000_000_000_000;
        let bars = vec![make_test_bar(base_ts, 45000.0, 100.0)];

        let (brackets, _events) = pipeline
            .compute_session_output(&bars, "BTCUSDT", "2024-02-02")
            .unwrap();

        // Single bar should produce exactly one bracket when finalized
        assert_eq!(brackets.len(), 1, "Single bar should produce one bracket");

        let bracket = &brackets[0];
        assert_eq!(bracket.label, "A", "First bracket should have label 'A'");
        assert_eq!(
            bracket.bracket_index, 0,
            "First bracket should have index 0"
        );
        assert_eq!(bracket.bracket_bar_count, 1, "Bracket should contain 1 bar");
        assert_eq!(bracket.instrument_id, "BTCUSDT-PERP.BINANCE");
        assert_eq!(bracket.session_date, "2024-02-02");

        // Verify OHLCV from the single bar
        assert_eq!(bracket.bracket_open, 45000.0);
        assert_eq!(bracket.bracket_close, 45000.0);
        assert_eq!(bracket.bracket_high, 45010.0); // price + 10
        assert_eq!(bracket.bracket_low, 44990.0); // price - 10
        assert_eq!(bracket.bracket_volume, 100.0);
    }

    #[test]
    fn test_compute_session_output_multiple_brackets() {
        let config = make_test_config();
        let pipeline = Pipeline::new(config).unwrap();

        // Create bars spanning multiple brackets (30 min each)
        // Bracket A: 00:00-00:30
        // Bracket B: 00:30-01:00
        let base_ts: i64 = 1_706_832_000_000_000_000; // 2024-02-02 00:00:00 UTC
        let minute_ns: i64 = 60_000_000_000;

        let mut bars = Vec::new();

        // Add 30 bars for bracket A (00:00 - 00:29)
        for i in 0..30 {
            bars.push(make_test_bar(base_ts + i * minute_ns, 45000.0, 10.0));
        }

        // Add 30 bars for bracket B (00:30 - 00:59)
        for i in 30..60 {
            bars.push(make_test_bar(base_ts + i * minute_ns, 45100.0, 15.0));
        }

        let (brackets, _events) = pipeline
            .compute_session_output(&bars, "BTCUSDT", "2024-02-02")
            .unwrap();

        // Should produce 2 brackets
        assert_eq!(brackets.len(), 2, "Should produce 2 brackets");

        // Verify bracket A
        let bracket_a = &brackets[0];
        assert_eq!(bracket_a.label, "A");
        assert_eq!(bracket_a.bracket_index, 0);
        assert_eq!(bracket_a.bracket_bar_count, 30);
        assert_eq!(bracket_a.bracket_volume, 300.0); // 30 bars * 10.0 volume

        // Verify bracket B
        let bracket_b = &brackets[1];
        assert_eq!(bracket_b.label, "B");
        assert_eq!(bracket_b.bracket_index, 1);
        assert_eq!(bracket_b.bracket_bar_count, 30);
        assert_eq!(bracket_b.bracket_volume, 450.0); // 30 bars * 15.0 volume
    }

    #[test]
    fn test_finalize_old_sessions_removes_stale() {
        let config = make_test_config();
        let mut pipeline = Pipeline::new(config).unwrap();

        // Manually add a session that is old (last_update_ns is in the past)
        let old_time_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0) - 3_600_000_000_000; // 1 hour ago

        let session_key = ("BTCUSDT".to_string(), "2024-02-01".to_string());
        pipeline.active_sessions.insert(
            session_key.clone(),
            SessionAccumulator {
                bars: vec![make_test_bar(1_706_745_600_000_000_000, 44000.0, 50.0)],
                tpo_processor: TpoProcessor::new(crate::config::TpoConfig::default()),
                event_detector: EventDetector::new(&crate::config::ProfileConfig::default()),
                last_update_ns: old_time_ns,
            },
        );

        // Verify session exists
        assert_eq!(pipeline.active_sessions.len(), 1);

        // Finalize with max_age of 30 minutes (1800 seconds * 1e9 ns)
        let max_age_ns = 1_800_000_000_000_i64;
        pipeline.finalize_old_sessions(max_age_ns).unwrap();

        // Session should be removed (it's 1 hour old, max age is 30 min)
        assert!(
            pipeline.active_sessions.is_empty(),
            "Old session should be removed"
        );
    }

    #[test]
    fn test_finalize_old_sessions_keeps_recent() {
        let config = make_test_config();
        let mut pipeline = Pipeline::new(config).unwrap();

        // Add a recent session (last_update_ns is now)
        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);

        let session_key = ("BTCUSDT".to_string(), "2024-02-02".to_string());
        pipeline.active_sessions.insert(
            session_key.clone(),
            SessionAccumulator {
                bars: vec![make_test_bar(1_706_832_000_000_000_000, 45000.0, 100.0)],
                tpo_processor: TpoProcessor::new(crate::config::TpoConfig::default()),
                event_detector: EventDetector::new(&crate::config::ProfileConfig::default()),
                last_update_ns: now_ns,
            },
        );

        // Verify session exists
        assert_eq!(pipeline.active_sessions.len(), 1);

        // Finalize with max_age of 30 minutes
        let max_age_ns = 1_800_000_000_000_i64;
        pipeline.finalize_old_sessions(max_age_ns).unwrap();

        // Session should still exist (it's recent)
        assert_eq!(
            pipeline.active_sessions.len(),
            1,
            "Recent session should be retained"
        );
        assert!(
            pipeline.active_sessions.contains_key(&session_key),
            "Session key should still exist"
        );
    }

    #[test]
    fn test_finalize_old_sessions_mixed_ages() {
        let config = make_test_config();
        let mut pipeline = Pipeline::new(config).unwrap();

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let old_time_ns = now_ns - 7_200_000_000_000; // 2 hours ago

        // Add an old session
        let old_session_key = ("BTCUSDT".to_string(), "2024-02-01".to_string());
        pipeline.active_sessions.insert(
            old_session_key.clone(),
            SessionAccumulator {
                bars: vec![make_test_bar(1_706_745_600_000_000_000, 44000.0, 50.0)],
                tpo_processor: TpoProcessor::new(crate::config::TpoConfig::default()),
                event_detector: EventDetector::new(&crate::config::ProfileConfig::default()),
                last_update_ns: old_time_ns,
            },
        );

        // Add a recent session
        let recent_session_key = ("ETHUSDT".to_string(), "2024-02-02".to_string());
        pipeline.active_sessions.insert(
            recent_session_key.clone(),
            SessionAccumulator {
                bars: vec![make_test_bar(1_706_832_000_000_000_000, 3000.0, 200.0)],
                tpo_processor: TpoProcessor::new(crate::config::TpoConfig::default()),
                event_detector: EventDetector::new(&crate::config::ProfileConfig::default()),
                last_update_ns: now_ns,
            },
        );

        // Verify both sessions exist
        assert_eq!(pipeline.active_sessions.len(), 2);

        // Finalize with max_age of 1 hour
        let max_age_ns = 3_600_000_000_000_i64;
        pipeline.finalize_old_sessions(max_age_ns).unwrap();

        // Only recent session should remain
        assert_eq!(pipeline.active_sessions.len(), 1);
        assert!(
            !pipeline.active_sessions.contains_key(&old_session_key),
            "Old session should be removed"
        );
        assert!(
            pipeline.active_sessions.contains_key(&recent_session_key),
            "Recent session should be retained"
        );
    }

    #[test]
    fn test_extract_session_date() {
        let path = Path::new("/data/raw/BTCUSDT-PERP.BINANCE/2024-02-02/extended_bars_1m.parquet");
        assert_eq!(extract_session_date(path), "2024-02-02");

        let path2 = Path::new("/data/raw/ETHUSDT-PERP.BINANCE/2024-12-31/trades.parquet");
        assert_eq!(extract_session_date(path2), "2024-12-31");

        // Short path should return UNKNOWN
        let short_path = Path::new("test.parquet");
        assert_eq!(extract_session_date(short_path), "UNKNOWN");
    }
}
