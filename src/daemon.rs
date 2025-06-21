use crate::config::load_config;
use crate::downloader::Downloader;
use crate::installer::Installer;
use crate::types::*;
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{Mutex, RwLock};
use tokio::time::{interval, sleep, timeout};
use tracing::{debug, error, info, warn};

/// Main daemon service orchestrating OTA updates
pub struct OtaDaemon {
    config: Arc<RwLock<OtaConfig>>,
    downloader: Arc<Mutex<Downloader>>,
    installer: Arc<Mutex<Installer>>,
    state: Arc<RwLock<DaemonState>>,
    update_history: Arc<Mutex<Vec<UpdateRecord>>>,
    start_time: Instant,
    last_check: Arc<RwLock<Option<DateTime<Utc>>>>,
    shutdown_requested: Arc<RwLock<bool>>,
    log_file_path: String,
    config_path: String,
}

impl OtaDaemon {
    /// Create new daemon instance
    pub async fn new(config_path: &str) -> Result<Self> {
        let config = load_config(config_path)
            .await
            .context("Failed to load configuration")?;

        let downloader = Downloader::new(config.clone());
        let installer = Installer::new(config.clone()).context("Failed to initialize installer")?;

        // Create log file path
        let log_file_path = format!("{}/ota_update_history.json", config.download_path);

        // Load existing update history
        let update_history = Self::load_update_history(&log_file_path).await?;

        Ok(Self {
            config: Arc::new(RwLock::new(config)),
            downloader: Arc::new(Mutex::new(downloader)),
            installer: Arc::new(Mutex::new(installer)),
            state: Arc::new(RwLock::new(DaemonState::Starting)),
            update_history: Arc::new(Mutex::new(update_history)),
            start_time: Instant::now(),
            last_check: Arc::new(RwLock::new(None)),
            shutdown_requested: Arc::new(RwLock::new(false)),
            log_file_path,
            config_path: config_path.to_string(),
        })
    }

    /// Start the daemon main loop
    pub async fn run(self: Arc<Self>) -> Result<()> {
        info!("Starting OTA daemon");

        // Setup signal handlers
        self.setup_signal_handlers().await?;

        // Transition to idle state
        self.set_state(DaemonState::Idle).await;

        // Main service loop
        let config = self.config.read().await;
        let check_interval = Duration::from_secs(config.check_interval_minutes * 60);
        drop(config);

        let mut check_timer = interval(check_interval);

        loop {
            tokio::select! {
                _ = check_timer.tick() => {
                    if *self.shutdown_requested.read().await {
                        break;
                    }

                    info!("Periodic update check triggered");
                    if let Err(e) = self.perform_update_cycle().await {
                        error!("Update cycle failed: {}", e);
                        self.set_state(DaemonState::Error(e.to_string())).await;

                        // Wait before next attempt
                        sleep(Duration::from_secs(300)).await; // 5 minutes
                        self.set_state(DaemonState::Idle).await;
                    }
                }

                _ = tokio::signal::ctrl_c() => {
                    info!("Received shutdown signal");
                    break;
                }
            }
        }

        self.shutdown().await
    }

    /// Perform complete update cycle with retry logic
    async fn perform_update_cycle(&self) -> Result<()> {
        let start_time = Instant::now();
        let mut last_error = None;

        // Try up to 3 times
        for attempt in 1..=3 {
            match self.try_update_cycle(attempt).await {
                Ok(update_record) => {
                    // Success - save to history
                    self.save_update_record(update_record).await?;
                    *self.last_check.write().await = Some(Utc::now());
                    return Ok(());
                }
                Err(e) => {
                    warn!("Update attempt {} failed: {}", attempt, e);
                    last_error = Some(e);

                    if attempt < 3 {
                        // Wait before retry (exponential backoff)
                        let wait_time = Duration::from_secs((60 * attempt).into());
                        info!("Waiting {} seconds before retry", wait_time.as_secs());
                        sleep(wait_time).await;
                    }
                }
            }
        }

        // All attempts failed - perform rollback if needed
        let error = last_error.unwrap();
        error!("All update attempts failed: {}", error);

        // Check if we need to rollback
        if self.should_rollback(&error).await {
            warn!("Performing automatic rollback");
            if let Err(rollback_err) = self.perform_rollback().await {
                error!("Rollback failed: {}", rollback_err);
            }
        }

        // Record the failure
        let failure_record = UpdateRecord {
            timestamp: Utc::now(),
            version: "unknown".to_string(),
            status: UpdateStatus::Failed,
            error_message: Some(error.to_string()),
            duration_seconds: start_time.elapsed().as_secs(),
        };

        self.save_update_record(failure_record).await?;
        *self.last_check.write().await = Some(Utc::now());

        Err(error)
    }

    /// Single update cycle attempt
    async fn try_update_cycle(&self, attempt: u8) -> Result<UpdateRecord> {
        let start_time = Instant::now();
        info!("Starting update cycle (attempt {})", attempt);

        // Get timeout from config
        let config = self.config.read().await;
        let download_timeout = Duration::from_secs(config.download_timeout_secs);
        drop(config);

        // Wrap download operations with timeout
        let download_result = timeout(download_timeout, async {
            // 1. Server Discovery
            self.set_state(DaemonState::Discovering).await;
            let mut downloader = self.downloader.lock().await;
            let server_info = downloader
                .discover_server()
                .await
                .context("Failed to discover server")?;
            info!(
                "Discovered server: {} at {}",
                server_info.name, server_info.address
            );

            // 2. Check for Updates
            self.set_state(DaemonState::CheckingUpdates).await;
            let metadata = match downloader.check_for_updates().await? {
                Some(metadata) => {
                    info!("Update available: version {}", metadata.latest_version);
                    metadata
                }
                None => {
                    info!("No updates available");
                    return Ok((None, String::new()));
                }
            };

            // 3. Download Update
            let downloaded_path = {
                let progress_callback = {
                    let state = Arc::clone(&self.state);
                    Box::new(move |progress: DownloadProgress| {
                        tokio::spawn({
                            let state = Arc::clone(&state);
                            async move {
                                let mut state_guard = state.write().await;
                                *state_guard = DaemonState::Downloading(progress);
                            }
                        });
                    })
                };

                downloader
                    .download_with_retries(&metadata, Some(progress_callback))
                    .await
                    .context("Failed to download kernel")?
            };

            Ok((Some(metadata), downloaded_path))
        })
        .await;

        // Handle timeout or download results
        let (metadata, downloaded_path) = match download_result {
            Ok(Ok((Some(metadata), downloaded_path))) => (metadata, downloaded_path),
            Ok(Ok((None, _))) => {
                // No updates available
                return Ok(UpdateRecord {
                    timestamp: Utc::now(),
                    version: "no-update".to_string(),
                    status: UpdateStatus::Success,
                    error_message: None,
                    duration_seconds: start_time.elapsed().as_secs(),
                });
            }
            Ok(Err(e)) => {
                return Err(e);
            }
            Err(_) => {
                anyhow::bail!(
                    "Download operation timed out after {} seconds",
                    download_timeout.as_secs()
                );
            }
        };

        info!("Kernel downloaded to: {}", downloaded_path);

        // 4. Install Update
        let installation_callback = {
            let state = Arc::clone(&self.state);
            move |status: crate::installer::InstallationStatus| {
                tokio::spawn({
                    let state = Arc::clone(&state);
                    async move {
                        let mut state_guard = state.write().await;
                        *state_guard = DaemonState::Installing(status);
                    }
                });
            }
        };

        let mut installer = self.installer.lock().await;
        installer
            .install_kernel(&downloaded_path, &metadata, Some(&installation_callback))
            .await
            .context("Failed to install kernel")?;

        info!("Kernel installation completed successfully");

        // 5. Cleanup
        if let Err(e) = tokio::fs::remove_file(&downloaded_path).await {
            warn!("Failed to cleanup downloaded file: {}", e);
        }

        // 6. Schedule reboot (if needed)
        self.set_state(DaemonState::Rebooting).await;
        info!("Kernel update completed. System reboot may be required.");

        Ok(UpdateRecord {
            timestamp: Utc::now(),
            version: metadata.latest_version,
            status: UpdateStatus::Success,
            error_message: None,
            duration_seconds: start_time.elapsed().as_secs(),
        })
    }

    /// Determine if rollback is needed based on error type
    async fn should_rollback(&self, error: &anyhow::Error) -> bool {
        let error_str = error.to_string().to_lowercase();

        // Rollback for installation failures but not for network/discovery issues
        error_str.contains("install")
            || error_str.contains("backup")
            || error_str.contains("kernel")
            || error_str.contains("checksum")
    }

    /// Perform automatic rollback
    async fn perform_rollback(&self) -> Result<()> {
        info!("Performing automatic rollback");

        let installer = self.installer.lock().await;
        installer
            .rollback()
            .await
            .context("Rollback operation failed")?;

        // Record rollback
        let rollback_record = UpdateRecord {
            timestamp: Utc::now(),
            version: "rollback".to_string(),
            status: UpdateStatus::RolledBack,
            error_message: Some("Automatic rollback after failed update".to_string()),
            duration_seconds: 0,
        };

        drop(installer);
        self.save_update_record(rollback_record).await?;

        info!("Rollback completed successfully");
        Ok(())
    }

    /// Get current daemon status
    pub async fn get_status(&self) -> DaemonStatus {
        let state = self.state.read().await.clone();
        let last_check = *self.last_check.read().await;
        let history = self.update_history.lock().await;
        let last_update = history.last().cloned();
        let update_count = history.len();
        drop(history);

        let config = self.config.read().await;
        let check_interval = Duration::from_secs(config.check_interval_minutes * 60);
        drop(config);

        let next_check_in = if let Some(last) = last_check {
            let elapsed = Utc::now().signed_duration_since(last);
            let elapsed_duration = Duration::from_secs(elapsed.num_seconds().max(0) as u64);
            check_interval.saturating_sub(elapsed_duration)
        } else {
            Duration::from_secs(0) // First check will happen soon
        };

        DaemonStatus {
            current_state: state,
            last_check,
            last_update,
            update_count,
            uptime: self.start_time.elapsed(),
            next_check_in,
        }
    }

    /// Reload configuration from file
    pub async fn reload_config(&self, config_path: &str) -> Result<()> {
        info!("Reloading configuration");

        let new_config = load_config(config_path)
            .await
            .context("Failed to reload configuration")?;

        // Update components with new config
        let mut config_guard = self.config.write().await;
        *config_guard = new_config.clone();
        drop(config_guard);

        // Update downloader
        let mut downloader = self.downloader.lock().await;
        *downloader = Downloader::new(new_config.clone());
        drop(downloader);

        // Update installer
        let mut installer = self.installer.lock().await;
        *installer = Installer::new(new_config).context("Failed to reinitialize installer")?;
        drop(installer);

        info!("Configuration reloaded successfully");
        Ok(())
    }

    /// Request graceful shutdown
    pub async fn request_shutdown(&self) {
        info!("Shutdown requested");
        *self.shutdown_requested.write().await = true;
    }

    /// Force immediate update check
    pub async fn force_update_check(&self) -> Result<()> {
        info!("Forcing immediate update check");
        self.perform_update_cycle().await
    }

    /// Perform manual rollback
    pub async fn manual_rollback(&self) -> Result<()> {
        info!("Manual rollback requested");
        self.perform_rollback().await
    }

    /// Setup signal handlers for graceful shutdown and config reload
    async fn setup_signal_handlers(self: &Arc<Self>) -> Result<()> {
        // Setup SIGTERM handler
        let shutdown_flag = Arc::clone(&self.shutdown_requested);
        tokio::spawn(async move {
            if let Ok(()) =
                tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                    .expect("Failed to setup SIGTERM handler")
                    .recv()
                    .await
                    .ok_or(())
            {
                info!("Received SIGTERM, requesting shutdown");
                *shutdown_flag.write().await = true;
            }
        });

        // Setup SIGHUP handler for config reload
        let daemon_weak = Arc::downgrade(self);
        let config_path = self.config_path.clone();
        tokio::spawn(async move {
            let mut signal = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::hangup())
                .expect("Failed to setup SIGHUP handler");

            while signal.recv().await.is_some() {
                info!("Received SIGHUP, reloading configuration");
                if let Some(daemon) = daemon_weak.upgrade() {
                    if let Err(e) = daemon.reload_config(&config_path).await {
                        error!("Failed to reload config via SIGHUP: {}", e);
                    } else {
                        info!("Configuration reloaded successfully via SIGHUP");
                    }
                } else {
                    // Daemon has been dropped, exit the signal handler
                    break;
                }
            }
        });

        Ok(())
    }

    /// Set daemon state
    async fn set_state(&self, new_state: DaemonState) {
        let mut state = self.state.write().await;
        debug!("State transition: {:?} -> {:?}", *state, new_state);
        *state = new_state;
    }

    /// Save update record to history and persistent storage
    async fn save_update_record(&self, record: UpdateRecord) -> Result<()> {
        // Add to in-memory history
        let mut history = self.update_history.lock().await;
        history.push(record);

        // Keep only last 100 records
        let len = history.len();
        if len > 100 {
            history.drain(0..len - 100);
        }

        // Save to file
        self.save_update_history(&history).await?;

        Ok(())
    }

    /// Load update history from file
    async fn load_update_history(log_file_path: &str) -> Result<Vec<UpdateRecord>> {
        if !Path::new(log_file_path).exists() {
            return Ok(Vec::new());
        }

        let content = tokio::fs::read_to_string(log_file_path)
            .await
            .context("Failed to read update history file")?;

        let history: Vec<UpdateRecord> =
            serde_json::from_str(&content).context("Failed to parse update history")?;

        info!("Loaded {} update records from history", history.len());
        Ok(history)
    }

    /// Save update history to file
    async fn save_update_history(&self, history: &[UpdateRecord]) -> Result<()> {
        let content =
            serde_json::to_string_pretty(history).context("Failed to serialize update history")?;

        tokio::fs::write(&self.log_file_path, content)
            .await
            .context("Failed to write update history file")?;

        debug!("Saved {} update records to history", history.len());
        Ok(())
    }

    /// Graceful shutdown
    async fn shutdown(&self) -> Result<()> {
        info!("Shutting down OTA daemon");

        self.set_state(DaemonState::Shutdown).await;

        // Save final state
        let history = self.update_history.lock().await;
        if let Err(e) = self.save_update_history(&history).await {
            error!("Failed to save update history during shutdown: {}", e);
        }

        info!("OTA daemon shutdown complete");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    async fn create_test_daemon() -> (TempDir, OtaDaemon) {
        let temp_dir = TempDir::new().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = OtaConfig {
            check_interval_minutes: 1, // 1 minute for testing
            download_path: temp_dir.path().to_string_lossy().to_string(),
            kernel_path: temp_dir
                .path()
                .join("kernel.img")
                .to_string_lossy()
                .to_string(),
            backup_path: temp_dir
                .path()
                .join("kernel.img.backup")
                .to_string_lossy()
                .to_string(),
            max_retries: 2,
            mdns_service: "_ota._tcp.local".to_string(),
            fallback_server: Some("http://localhost:8080".to_string()),
            download_timeout_secs: 30,
        };

        let config_content = toml::to_string(&config).unwrap();
        fs::write(&config_path, config_content).unwrap();

        let daemon = OtaDaemon::new(config_path.to_str().unwrap()).await.unwrap();
        (temp_dir, daemon)
    }

    #[tokio::test]
    async fn test_daemon_creation() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        let status = daemon.get_status().await;
        assert!(matches!(status.current_state, DaemonState::Starting));
        assert_eq!(status.update_count, 0);
        assert!(status.last_check.is_none());
    }

    #[tokio::test]
    async fn test_state_transitions() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        daemon.set_state(DaemonState::Idle).await;
        let status = daemon.get_status().await;
        assert!(matches!(status.current_state, DaemonState::Idle));

        daemon.set_state(DaemonState::CheckingUpdates).await;
        let status = daemon.get_status().await;
        assert!(matches!(status.current_state, DaemonState::CheckingUpdates));
    }

    #[tokio::test]
    async fn test_update_history_persistence() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        let record = UpdateRecord {
            timestamp: Utc::now(),
            version: "1.0.0".to_string(),
            status: UpdateStatus::Success,
            error_message: None,
            duration_seconds: 120,
        };

        daemon.save_update_record(record.clone()).await.unwrap();

        let status = daemon.get_status().await;
        assert_eq!(status.update_count, 1);
        assert_eq!(status.last_update.as_ref().unwrap().version, "1.0.0");
    }

    #[tokio::test]
    async fn test_config_reload() {
        let (temp_dir, daemon) = create_test_daemon().await;
        let config_path = temp_dir.path().join("config.toml");

        // Modify config
        let mut new_config = OtaConfig::default();
        new_config.check_interval_minutes = 5;
        new_config.download_path = temp_dir.path().to_string_lossy().to_string();

        let config_content = toml::to_string(&new_config).unwrap();
        fs::write(&config_path, config_content).unwrap();

        daemon
            .reload_config(config_path.to_str().unwrap())
            .await
            .unwrap();

        let config = daemon.config.read().await;
        assert_eq!(config.check_interval_minutes, 5);
    }

    #[tokio::test]
    async fn test_shutdown_request() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        assert!(!*daemon.shutdown_requested.read().await);

        daemon.request_shutdown().await;

        assert!(*daemon.shutdown_requested.read().await);
    }

    #[tokio::test]
    async fn test_rollback_decision() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        let install_error = anyhow::anyhow!("Installation failed: checksum mismatch");
        assert!(daemon.should_rollback(&install_error).await);

        let network_error = anyhow::anyhow!("Network timeout during discovery");
        assert!(!daemon.should_rollback(&network_error).await);
    }

    #[tokio::test]
    async fn test_status_reporting() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        // Add some test data
        let record = UpdateRecord {
            timestamp: Utc::now(),
            version: "1.0.0".to_string(),
            status: UpdateStatus::Success,
            error_message: None,
            duration_seconds: 60,
        };

        daemon.save_update_record(record).await.unwrap();
        daemon.set_state(DaemonState::Idle).await;

        let status = daemon.get_status().await;
        assert!(matches!(status.current_state, DaemonState::Idle));
        assert_eq!(status.update_count, 1);
    }

    #[tokio::test]
    async fn test_history_size_limit() {
        let (_temp_dir, daemon) = create_test_daemon().await;

        // Add more than 100 records
        for i in 0..105 {
            let record = UpdateRecord {
                timestamp: Utc::now(),
                version: format!("1.0.{}", i),
                status: UpdateStatus::Success,
                error_message: None,
                duration_seconds: 60,
            };
            daemon.save_update_record(record).await.unwrap();
        }

        let status = daemon.get_status().await;
        assert_eq!(status.update_count, 100); // Should be limited to 100
    }
}
