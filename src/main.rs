use anyhow::{Context, Result};
use clap::Parser;
use ota_client::config::load_config;
use ota_client::daemon::OtaDaemon;
use ota_client::downloader::Downloader;
use ota_client::installer::Installer;
use ota_client::types::{Cli, Commands, UpdateRecord};
use std::sync::Arc;
use tokio::fs;
use tracing::{error, info, warn};
use tracing_subscriber::{filter::EnvFilter, fmt, prelude::*};

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    setup_logging();

    // Parse command line arguments
    let cli = Cli::parse();

    match &cli.command {
        Commands::Daemon { config } => {
            info!("Starting OTA daemon with config: {}", config);
            run_daemon(config).await
        }
        Commands::Check { config } => {
            info!("Performing one-time update check with config: {}", config);
            run_check(config).await
        }
        Commands::Update { config } => {
            info!("Forcing update with config: {}", config);
            run_update(config).await
        }
        Commands::Status { config } => {
            info!("Showing status with config: {}", config);
            run_status(config).await
        }
        Commands::Rollback { config } => {
            info!("Performing rollback with config: {}", config);
            run_rollback(config).await
        }
    }
}

/// Setup structured logging with environment variable support
fn setup_logging() {
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::registry()
        .with(fmt::layer().with_target(false))
        .with(env_filter)
        .init();
}

/// Run the daemon in background mode
async fn run_daemon(config_path: &str) -> Result<()> {
    info!("Initializing OTA daemon");

    // Ensure config file exists
    ensure_config_exists(config_path).await?;

    let daemon = OtaDaemon::new(config_path)
        .await
        .context("Failed to create daemon instance")?;

    let daemon_arc = Arc::new(daemon);

    info!("Starting daemon main loop");
    daemon_arc.run().await.context("Daemon execution failed")
}

/// Perform a one-time update check
async fn run_check(config_path: &str) -> Result<()> {
    info!("Loading configuration and checking for updates");

    ensure_config_exists(config_path).await?;
    let config = load_config(config_path).await?;

    let mut downloader = Downloader::new(config);

    // Discover server
    info!("Discovering OTA server...");
    match downloader.discover_server().await {
        Ok(server_info) => {
            info!(
                "Found OTA server: {} at {}",
                server_info.name, server_info.address
            );

            // Check for updates
            match downloader.check_for_updates().await? {
                Some(metadata) => {
                    info!("✅ Update available!");
                    info!("  Version: {}", metadata.latest_version);
                    info!("  Size: {} bytes", metadata.file_size);
                    info!("  Released: {}", metadata.release_date);
                    info!("  Description: {}", metadata.description);
                    info!("Run 'ota-client update' to install this update");
                }
                None => {
                    info!("✅ No updates available - system is up to date");
                }
            }
        }
        Err(e) => {
            error!("❌ Failed to discover OTA server: {}", e);
            return Err(e);
        }
    }

    Ok(())
}

/// Force update download and installation
async fn run_update(config_path: &str) -> Result<()> {
    info!("Loading configuration and forcing update");

    ensure_config_exists(config_path).await?;
    let config = load_config(config_path).await?;

    let mut downloader = Downloader::new(config.clone());

    // Discover server
    info!("Discovering OTA server...");
    let server_info = downloader
        .discover_server()
        .await
        .context("Failed to discover server")?;
    info!(
        "Found OTA server: {} at {}",
        server_info.name, server_info.address
    );

    // Check for updates
    let metadata = match downloader.check_for_updates().await? {
        Some(metadata) => {
            info!("Update available: version {}", metadata.latest_version);
            metadata
        }
        None => {
            info!("No updates available - system is already up to date");
            return Ok(());
        }
    };

    // Download update
    info!("Downloading kernel update...");
    let downloaded_path = downloader
        .download_with_retries(&metadata, None)
        .await
        .context("Failed to download kernel")?;
    info!("Download completed: {}", downloaded_path);

    // Install update
    info!("Installing kernel update...");
    let mut installer = Installer::new(config).context("Failed to initialize installer")?;
    installer
        .install_kernel(&downloaded_path, &metadata, None)
        .await
        .context("Failed to install kernel")?;

    // Cleanup
    if let Err(e) = fs::remove_file(&downloaded_path).await {
        warn!("Failed to cleanup downloaded file: {}", e);
    }

    info!("✅ Update installed successfully!");
    info!("System reboot may be required to activate the new kernel.");

    Ok(())
}

/// Show current system status
async fn run_status(config_path: &str) -> Result<()> {
    ensure_config_exists(config_path).await?;

    // Try to get daemon status if it's running
    // For now, we'll show basic configuration info
    let config = load_config(config_path).await?;

    info!("=== OTA Client Status ===");
    info!("Configuration file: {}", config_path);
    info!("Check interval: {} minutes", config.check_interval_minutes);
    info!("Download path: {}", config.download_path);
    info!("Kernel path: {}", config.kernel_path);
    info!("Backup path: {}", config.backup_path);
    info!("Download timeout: {} seconds", config.download_timeout_secs);

    // Check if history file exists
    let history_path = format!("{}/ota_update_history.json", config.download_path);
    match fs::read_to_string(&history_path).await {
        Ok(content) => match serde_json::from_str::<Vec<UpdateRecord>>(&content) {
            Ok(history) => {
                info!("Update history: {} records", history.len());
                if let Some(last_update) = history.last() {
                    info!(
                        "Last update: {} ({})",
                        last_update.version,
                        last_update.timestamp.format("%Y-%m-%d %H:%M:%S")
                    );
                    info!("Status: {:?}", last_update.status);
                }
            }
            Err(_) => {
                warn!("Failed to parse update history");
            }
        },
        Err(_) => {
            info!("No update history found");
        }
    }

    // Test server connectivity
    info!("Testing server connectivity...");
    let mut downloader = Downloader::new(config);
    match downloader.discover_server().await {
        Ok(server_info) => {
            info!(
                "✅ Server reachable: {} at {}",
                server_info.name, server_info.address
            );
        }
        Err(e) => {
            warn!("❌ Server not reachable: {}", e);
        }
    }

    Ok(())
}

/// Perform rollback to previous kernel
async fn run_rollback(config_path: &str) -> Result<()> {
    info!("Loading configuration and performing rollback");

    ensure_config_exists(config_path).await?;
    let config = load_config(config_path).await?;

    let installer = Installer::new(config).context("Failed to initialize installer")?;

    info!("Rolling back to previous kernel...");
    installer
        .rollback()
        .await
        .context("Failed to perform rollback")?;

    info!("✅ Rollback completed successfully!");
    info!("System reboot may be required to activate the previous kernel.");

    Ok(())
}

/// Ensure configuration file exists, create default if not
async fn ensure_config_exists(config_path: &str) -> Result<()> {
    if !fs::metadata(config_path).await.is_ok() {
        info!(
            "Configuration file not found, creating default: {}",
            config_path
        );

        // Create parent directory if needed
        if let Some(parent) = std::path::Path::new(config_path).parent() {
            fs::create_dir_all(parent)
                .await
                .context("Failed to create config directory")?;
        }

        // Create default config
        ota_client::config::create_default_config(config_path)
            .await
            .context("Failed to create default configuration")?;

        info!("Default configuration created. Please review and modify as needed.");
    }

    Ok(())
}
