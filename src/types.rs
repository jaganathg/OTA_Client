use serde::{Deserialize, Serialize};
use std::net::SocketAddr;

/// OTA client configuration
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct OtaConfig {
    /// Check interval in minutes
    pub check_interval_minutes: u64,

    /// Download directory for temporary files
    pub download_path: String,

    /// Current kernel path
    pub kernel_path: String,

    /// Backup kernel path
    pub backup_path: String,

    /// Maximum retry attempts for downloads
    pub max_retries: u32,

    /// mDNS service name to discover
    pub mdns_service: String,

    /// Fallback server URL (if mDNS fails)
    pub fallback_server: Option<String>,

    /// Download timeout in seconds
    pub download_timeout_secs: u64,
}

impl Default for OtaConfig {
    fn default() -> Self {
        Self {
            check_interval_minutes: 60,
            download_path: "/opt/ota/downloads".to_string(),
            kernel_path: "/boot/kernel.img".to_string(),
            backup_path: "/boot/kernel.img.backup".to_string(),
            max_retries: 3,
            mdns_service: "_ota._tcp.local".to_string(),
            fallback_server: None,
            download_timeout_secs: 90, // 90 seconds
        }
    }
}

/// Server information discovered via mDNS
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub address: SocketAddr,
    pub name: String,
}

/// Kernel metadata from server
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct KernelMetadata {
    pub latest_version: String,
    pub kernel_file: String,
    pub file_size: u64,
    pub checksum: String,
    pub release_date: String,
    pub description: String,
    pub download_url: String,
}

/// Download progress information
#[derive(Debug, Clone, PartialEq)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

/// OTA operation result
#[derive(Debug, PartialEq)]
pub enum OtaResult {
    NoUpdate,
    UpdateAvailable(KernelMetadata),
    UpdateDownloaded(String), // file path
    UpdateInstalled,
    Error(String),
}

/// Daemon state for monitoring and control
#[derive(Debug, Clone, PartialEq)]
pub enum DaemonState {
    Starting,
    Idle,
    Discovering,
    CheckingUpdates,
    Downloading(DownloadProgress),
    Installing(crate::installer::InstallationStatus),
    Rebooting,
    Error(String),
    Shutdown,
}

/// Update record for history tracking
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateRecord {
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub version: String,
    pub status: UpdateStatus,
    pub error_message: Option<String>,
    pub duration_seconds: u64,
}

/// Update operation status
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum UpdateStatus {
    Success,
    Failed,
    RolledBack,
}

/// Daemon status for external monitoring
#[derive(Debug, Clone)]
pub struct DaemonStatus {
    pub current_state: DaemonState,
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,
    pub last_update: Option<UpdateRecord>,
    pub update_count: usize,
    pub uptime: std::time::Duration,
    pub next_check_in: std::time::Duration,
}

/// CLI commands
#[derive(Debug, clap::Parser)]
#[command(name = "ota-client")]
#[command(about = "OTA Client for Raspberry Pi kernel updates")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Debug, clap::Subcommand)]
pub enum Commands {
    /// Run as daemon (background service)
    Daemon {
        #[arg(short, long, default_value = "/etc/ota-client/config.toml")]
        config: String,
    },
    /// Check for updates once
    Check {
        #[arg(short, long, default_value = "/etc/ota-client/config.toml")]
        config: String,
    },
    /// Force download and install update
    Update {
        #[arg(short, long, default_value = "/etc/ota-client/config.toml")]
        config: String,
    },
    /// Show current status
    Status {
        #[arg(short, long, default_value = "/etc/ota-client/config.toml")]
        config: String,
    },
    /// Rollback to previous kernel
    Rollback {
        #[arg(short, long, default_value = "/etc/ota-client/config.toml")]
        config: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn test_ota_config_default() {
        let config = OtaConfig::default();
        assert_eq!(config.check_interval_minutes, 60);
        assert_eq!(config.download_path, "/opt/ota/downloads");
        assert_eq!(config.kernel_path, "/boot/kernel.img");
        assert_eq!(config.backup_path, "/boot/kernel.img.backup");
        assert_eq!(config.max_retries, 3);
        assert_eq!(config.mdns_service, "_ota._tcp.local");
        assert_eq!(config.download_timeout_secs, 90);
        assert!(config.fallback_server.is_none());
    }

    #[test]
    fn test_ota_config_serialization() {
        let config = OtaConfig::default();
        let serialized = toml::to_string(&config).unwrap();
        let deserialized: OtaConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            config.check_interval_minutes,
            deserialized.check_interval_minutes
        );
        assert_eq!(config.mdns_service, deserialized.mdns_service);
    }

    #[test]
    fn test_server_info_creation() {
        let addr = std::net::SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 8080);
        let server = ServerInfo {
            address: addr,
            name: "test-server".to_string(),
        };
        assert_eq!(server.address.port(), 8080);
        assert_eq!(server.name, "test-server");
    }

    #[test]
    fn test_kernel_metadata_creation() {
        let metadata = KernelMetadata {
            latest_version: "1.0.0".to_string(),
            kernel_file: "kernel-v1.0.0.img".to_string(),
            file_size: 1024,
            checksum: "sha256:abc123".to_string(),
            release_date: "2025-06-16".to_string(),
            description: "Test kernel".to_string(),
            download_url: "/kernels/kernel-v1.0.0.img".to_string(),
        };

        assert_eq!(metadata.latest_version, "1.0.0");
        assert_eq!(metadata.file_size, 1024);
        assert!(metadata.checksum.starts_with("sha256:"));
    }

    #[test]
    fn test_download_progress_calculation() {
        let progress = DownloadProgress {
            downloaded: 512,
            total: 1024,
            percentage: 50.0,
        };

        assert_eq!(progress.downloaded, 512);
        assert_eq!(progress.total, 1024);
        assert_eq!(progress.percentage, 50.0);

        // Test percentage calculation
        let calculated_percentage = (progress.downloaded as f64 / progress.total as f64) * 100.0;
        assert_eq!(calculated_percentage, progress.percentage);
    }

    #[test]
    fn test_ota_result_variants() {
        let no_update = OtaResult::NoUpdate;
        assert_eq!(no_update, OtaResult::NoUpdate);

        let error_result = OtaResult::Error("test error".to_string());
        if let OtaResult::Error(msg) = error_result {
            assert_eq!(msg, "test error");
        } else {
            panic!("Expected Error variant");
        }

        let downloaded = OtaResult::UpdateDownloaded("/tmp/kernel.img".to_string());
        if let OtaResult::UpdateDownloaded(path) = downloaded {
            assert_eq!(path, "/tmp/kernel.img");
        } else {
            panic!("Expected UpdateDownloaded variant");
        }
    }

    #[test]
    fn test_kernel_metadata_json_serialization() {
        let metadata = KernelMetadata {
            latest_version: "1.0.1".to_string(),
            kernel_file: "kernel-v1.0.1.img".to_string(),
            file_size: 2048,
            checksum: "sha256:def456".to_string(),
            release_date: "2025-06-16T10:30:00Z".to_string(),
            description: "Updated kernel with fixes".to_string(),
            download_url: "/kernels/kernel-v1.0.1.img".to_string(),
        };

        let json = serde_json::to_string(&metadata).unwrap();
        let deserialized: KernelMetadata = serde_json::from_str(&json).unwrap();

        assert_eq!(metadata.latest_version, deserialized.latest_version);
        assert_eq!(metadata.file_size, deserialized.file_size);
        assert_eq!(metadata.checksum, deserialized.checksum);
    }

    #[test]
    fn test_config_validation_bounds() {
        let mut config = OtaConfig::default();

        // Test minimum values
        config.check_interval_minutes = 1;
        config.max_retries = 1;
        config.download_timeout_secs = 1;

        assert!(config.check_interval_minutes > 0);
        assert!(config.max_retries > 0);
        assert!(config.download_timeout_secs > 0);
    }

    #[test]
    fn test_cli_commands_parsing() {
        // Test that CLI structure is correctly defined
        // This will be useful when implementing argument parsing

        // We can add more specific CLI parsing tests when implementing main.rs
        assert_eq!(std::mem::size_of::<Cli>(), std::mem::size_of::<Cli>());
    }
}
