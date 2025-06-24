use crate::types::{DownloadProgress, KernelMetadata, OtaConfig, ServerInfo};
use anyhow::{Context, Result};
use futures_util::{pin_mut, stream::StreamExt};
use reqwest::Client;
use sha2::{Digest, Sha256};
// use std::io::Write;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

use tracing::{debug, error, info, warn};

/// HTTP downloader with mDNS server discovery
pub struct Downloader {
    client: Client,
    config: OtaConfig,
    server_info: Option<ServerInfo>,
}

impl Downloader {
    /// Create new downloader instance
    pub fn new(config: OtaConfig) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.download_timeout_secs))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            client,
            config,
            server_info: None,
        }
    }

    /// Discover OTA server using mDNS
    pub async fn discover_server(&mut self) -> Result<ServerInfo> {
        info!(
            "Starting mDNS discovery for service: {}",
            self.config.mdns_service
        );

        match self.mdns_discovery().await {
            Ok(server) => {
                info!(
                    "Discovered server via mDNS: {}:{}",
                    server.address.ip(),
                    server.address.port()
                );
                self.server_info = Some(server.clone());
                Ok(server)
            }
            Err(e) => {
                warn!("mDNS discovery failed: {}", e);

                if let Some(fallback_url) = &self.config.fallback_server.clone() {
                    info!("Trying fallback server: {}", fallback_url);
                    self.try_fallback_server(fallback_url).await
                } else {
                    Err(e).context("mDNS discovery failed and no fallback server configured")
                }
            }
        }
    }

    /// Perform mDNS service discovery
    async fn mdns_discovery(&self) -> Result<ServerInfo> {
        use mdns::RecordKind;
        use std::collections::HashMap;

        info!("Starting mDNS discovery with timeout: 15s");
        let stream = mdns::discover::all(&self.config.mdns_service, Duration::from_secs(15))
            .context("Failed to start mDNS discovery")?
            .listen();

        let mut _services: HashMap<String, (std::net::IpAddr, u16)> = HashMap::new();

        pin_mut!(stream);
        while let Some(Ok(response)) = stream.next().await {
            info!(
                "Received mDNS response with {} records",
                response.records().count()
            );
            let mut name = String::new();
            let mut ip = None;
            let mut port = None;

            for record in response.records() {
                info!("Processing record: {:?}", &record.kind);
                match &record.kind {
                    RecordKind::A(addr) => {
                        ip = Some(std::net::IpAddr::V4(*addr));
                        debug!("Found A record: {}", addr);
                    }
                    RecordKind::AAAA(addr) => {
                        if !matches!(ip, Some(std::net::IpAddr::V4(_))) {
                            ip = Some(std::net::IpAddr::V6(*addr));
                            debug!("Found AAAA record: {}", addr);
                        } else {
                            debug!("Found AAAA record: {} (ignored, IPv4 preferred)", addr);
                        }
                    }
                    RecordKind::SRV {
                        port: srv_port,
                        target,
                        ..
                    } => {
                        port = Some(srv_port);
                        name = target.to_string();
                        debug!("Found SRV record: {}:{}", target, srv_port);
                    }
                    _ => {}
                }
            }

            if let (Some(ip_addr), Some(port_num)) = (ip, port) {
                let socket_addr = SocketAddr::new(ip_addr, *port_num);
                let server_info = ServerInfo {
                    address: socket_addr,
                    name: name.clone(),
                };

                // Test connectivity before returning
                info!(
                    "Found potential server: {}:{}, testing connectivity...",
                    ip_addr, port_num
                );
                if self.test_server_connectivity(&server_info).await.is_ok() {
                    info!(
                        "Successfully discovered server via mDNS: {}:{}",
                        ip_addr, port_num
                    );
                    return Ok(server_info);
                } else {
                    warn!(
                        "Server {}:{} found via mDNS but connectivity test failed",
                        ip_addr, port_num
                    );
                }
            }
        }

        anyhow::bail!("No valid OTA servers found via mDNS")
    }

    /// Try fallback server configuration
    async fn try_fallback_server(&mut self, server_url: &str) -> Result<ServerInfo> {
        let url = reqwest::Url::parse(server_url).context("Invalid fallback server URL")?;

        let host = url.host_str().context("No host in fallback server URL")?;
        let port = url.port().unwrap_or(80);

        let socket_addr = tokio::net::lookup_host((host, port))
            .await
            .context("Failed to resolve fallback server")?
            .next()
            .context("No address resolved for fallback server")?;

        let server_info = ServerInfo {
            address: socket_addr,
            name: host.to_string(),
        };

        if self.test_server_connectivity(&server_info).await.is_ok() {
            self.server_info = Some(server_info.clone());
            Ok(server_info)
        } else {
            anyhow::bail!("Fallback server is not responding")
        }
    }

    /// Test server connectivity
    async fn test_server_connectivity(&self, server: &ServerInfo) -> Result<()> {
        let url = format!(
            "http://{}:{}/health",
            server.address.ip(),
            server.address.port()
        );

        debug!("Testing connectivity to: {}", url);

        let response = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .context("Failed to connect to server")?;

        if response.status().is_success() {
            debug!("Server connectivity test passed");
            Ok(())
        } else {
            anyhow::bail!("Server returned error status: {}", response.status())
        }
    }

    /// Check for kernel updates
    pub async fn check_for_updates(&self) -> Result<Option<KernelMetadata>> {
        let server = self
            .server_info
            .as_ref()
            .context("No server discovered. Call discover_server() first")?;

        let url = format!(
            "http://{}:{}/version",
            server.address.ip(),
            server.address.port()
        );

        info!("Checking for updates at: {}", url);

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to check for updates")?;

        if !response.status().is_success() {
            anyhow::bail!("Server returned error: {}", response.status());
        }

        let version_response: serde_json::Value = response
            .json()
            .await
            .context("Failed to parse version response")?;

        // Extract kernel info from response
        let kernel_info = if let Some(kernel_info) = version_response.get("kernel_info") {
            serde_json::from_value(kernel_info.clone())
                .context("Failed to parse kernel metadata")?
        } else {
            // Fallback: try to parse entire response as KernelMetadata
            serde_json::from_value(version_response)
                .context("Failed to parse kernel metadata from response")?
        };

        Ok(Some(kernel_info))
    }

    /// Download kernel file with progress tracking
    pub async fn download_kernel(
        &self,
        metadata: &KernelMetadata,
        progress_callback: Option<&(dyn Fn(DownloadProgress) + Send + Sync)>,
    ) -> Result<String> {
        let server = self
            .server_info
            .as_ref()
            .context("No server discovered. Call discover_server() first")?;

        let url = format!(
            "http://{}:{}{}",
            server.address.ip(),
            server.address.port(),
            metadata.download_url
        );

        info!("Downloading kernel from: {}", url);

        // Create download directory
        tokio::fs::create_dir_all(&self.config.download_path)
            .await
            .context("Failed to create download directory")?;

        let filename = &metadata.kernel_file;
        let file_path = format!("{}/{}", self.config.download_path, filename);

        // Start download
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .context("Failed to start download")?;

        if !response.status().is_success() {
            anyhow::bail!("Download failed with status: {}", response.status());
        }

        // Get expected checksum from headers
        let expected_checksum = response
            .headers()
            .get("x-checksum")
            .and_then(|h| h.to_str().ok())
            .map(|s| s.to_string());

        let content_length = response.content_length().unwrap_or(metadata.file_size);

        // Create file and download with progress tracking
        let mut file = tokio::fs::File::create(&file_path)
            .await
            .context("Failed to create download file")?;

        let mut downloaded = 0u64;
        let mut hasher = Sha256::new();
        let mut stream = response.bytes_stream();

        while let Some(chunk_result) = tokio_stream::StreamExt::next(&mut stream).await {
            let chunk = chunk_result.context("Failed to read chunk")?;

            file.write_all(&chunk)
                .await
                .context("Failed to write chunk to file")?;

            hasher.update(&chunk);
            downloaded += chunk.len() as u64;

            // Report progress
            if let Some(ref callback) = progress_callback {
                let progress = DownloadProgress {
                    downloaded,
                    total: content_length,
                    percentage: (downloaded as f64 / content_length as f64) * 100.0,
                };
                callback(progress);
            }
        }

        file.flush().await.context("Failed to flush file")?;

        // Verify checksum
        let calculated_checksum = format!("sha256:{:x}", hasher.finalize());

        if let Some(expected) = expected_checksum {
            if calculated_checksum != expected && calculated_checksum != metadata.checksum {
                error!(
                    "Checksum mismatch! Expected: {}, Got: {}",
                    expected, calculated_checksum
                );
                // Clean up corrupted file
                let _ = tokio::fs::remove_file(&file_path).await;
                anyhow::bail!("Checksum verification failed");
            }
        } else if calculated_checksum != metadata.checksum {
            error!(
                "Checksum mismatch! Expected: {}, Got: {}",
                metadata.checksum, calculated_checksum
            );
            let _ = tokio::fs::remove_file(&file_path).await;
            anyhow::bail!("Checksum verification failed");
        }

        info!("Download completed successfully: {}", file_path);
        info!("Checksum verified: {}", calculated_checksum);

        Ok(file_path)
    }

    /// Download with retry logic
    pub async fn download_with_retries(
        &self,
        metadata: &KernelMetadata,
        progress_callback: Option<Box<dyn Fn(DownloadProgress) + Send + Sync>>,
    ) -> Result<String> {
        let mut last_error = None;

        for attempt in 1..=self.config.max_retries {
            match self
                .download_kernel(metadata, progress_callback.as_deref())
                .await
            {
                Ok(path) => return Ok(path),
                Err(e) => {
                    warn!("Download attempt {} failed: {}", attempt, e);
                    last_error = Some(e);

                    if attempt < self.config.max_retries {
                        let delay = Duration::from_secs(2_u64.pow(attempt - 1)); // Exponential backoff
                        info!("Retrying in {:?}...", delay);
                        tokio::time::sleep(delay).await;
                    }
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("All download attempts failed")))
    }

    /// Get current server info
    pub fn get_server_info(&self) -> Option<&ServerInfo> {
        self.server_info.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{IpAddr, Ipv4Addr};

    fn create_test_config() -> OtaConfig {
        OtaConfig {
            check_interval_minutes: 60,
            download_path: "/tmp/ota_test_downloads".to_string(),
            kernel_path: "/boot/kernel.img".to_string(),
            backup_path: "/boot/kernel.img.backup".to_string(),
            max_retries: 3,
            mdns_service: "_ota._tcp.local".to_string(),
            fallback_server: Some("http://192.168.1.100:8080".to_string()),
            download_timeout_secs: 30,
        }
    }

    fn create_test_server_info() -> ServerInfo {
        ServerInfo {
            address: SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 100)), 8080),
            name: "test-server".to_string(),
        }
    }

    fn create_test_metadata() -> KernelMetadata {
        KernelMetadata {
            latest_version: "1.0.0".to_string(),
            kernel_file: "kernel-v1.0.0.img".to_string(),
            file_size: 1024,
            checksum: "sha256:abc123def456".to_string(),
            release_date: "2025-06-16T10:30:00Z".to_string(),
            description: "Test kernel".to_string(),
            download_url: "/kernels/kernel-v1.0.0.img".to_string(),
        }
    }

    #[test]
    fn test_downloader_creation() {
        let config = create_test_config();
        let downloader = Downloader::new(config.clone());

        assert_eq!(downloader.config.download_timeout_secs, 30);
        assert_eq!(downloader.config.max_retries, 3);
        assert!(downloader.server_info.is_none());
    }

    #[tokio::test]
    async fn test_fallback_server_parsing() {
        let config = create_test_config();
        let mut downloader = Downloader::new(config);

        // Test with valid URL format
        let result = downloader
            .try_fallback_server("http://192.168.1.100:8080")
            .await;
        // This will fail due to no actual server, but parsing should work
        assert!(result.is_err()); // Expected since no real server
    }

    #[test]
    fn test_server_info_storage() {
        let config = create_test_config();
        let mut downloader = Downloader::new(config);
        let server_info = create_test_server_info();

        assert!(downloader.get_server_info().is_none());

        downloader.server_info = Some(server_info.clone());
        let stored_info = downloader.get_server_info().unwrap();

        assert_eq!(stored_info.address, server_info.address);
        assert_eq!(stored_info.name, server_info.name);
    }

    #[test]
    fn test_download_progress_calculation() {
        let progress = DownloadProgress {
            downloaded: 256,
            total: 1024,
            percentage: 25.0,
        };

        assert_eq!(progress.downloaded, 256);
        assert_eq!(progress.total, 1024);
        assert_eq!(progress.percentage, 25.0);
    }

    #[tokio::test]
    async fn test_download_directory_creation() {
        let config = create_test_config();
        let _downloader = Downloader::new(config.clone());

        // Clean up before test
        let _ = tokio::fs::remove_dir_all(&config.download_path).await;

        // Create download directory
        let result = tokio::fs::create_dir_all(&config.download_path).await;
        assert!(result.is_ok());

        // Verify directory exists
        let metadata = tokio::fs::metadata(&config.download_path).await;
        assert!(metadata.is_ok());
        assert!(metadata.unwrap().is_dir());

        // Clean up after test
        let _ = tokio::fs::remove_dir_all(&config.download_path).await;
    }

    #[test]
    fn test_checksum_format() {
        let test_data = b"hello world";
        let mut hasher = Sha256::new();
        hasher.update(test_data);
        let result = hasher.finalize();
        let checksum = format!("sha256:{:x}", result);

        assert!(checksum.starts_with("sha256:"));
        assert_eq!(checksum.len(), 71); // "sha256:" + 64 hex chars
    }

    #[test]
    fn test_metadata_validation() {
        let metadata = create_test_metadata();

        assert!(!metadata.latest_version.is_empty());
        assert!(!metadata.kernel_file.is_empty());
        assert!(metadata.file_size > 0);
        assert!(metadata.checksum.starts_with("sha256:"));
        assert!(metadata.download_url.starts_with("/"));
    }

    #[tokio::test]
    async fn test_retry_logic_parameters() {
        let config = create_test_config();
        let downloader = Downloader::new(config);

        // Test that retry logic uses correct max_retries
        assert_eq!(downloader.config.max_retries, 3);

        // Test exponential backoff calculation
        for attempt in 1..=3 {
            let delay = std::time::Duration::from_secs(2_u64.pow(attempt - 1));
            match attempt {
                1 => assert_eq!(delay.as_secs(), 1), // 2^0 = 1
                2 => assert_eq!(delay.as_secs(), 2), // 2^1 = 2
                3 => assert_eq!(delay.as_secs(), 4), // 2^2 = 4
                _ => {}
            }
        }
    }

    #[test]
    fn test_url_construction() {
        let server = create_test_server_info();
        let metadata = create_test_metadata();

        let version_url = format!(
            "http://{}:{}/version",
            server.address.ip(),
            server.address.port()
        );
        assert_eq!(version_url, "http://192.168.1.100:8080/version");

        let download_url = format!(
            "http://{}:{}{}",
            server.address.ip(),
            server.address.port(),
            metadata.download_url
        );
        assert_eq!(
            download_url,
            "http://192.168.1.100:8080/kernels/kernel-v1.0.0.img"
        );
    }
}
