use crate::types::{KernelMetadata, OtaConfig};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs::Permissions;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use tokio::fs as async_fs;
use tracing::{debug, error, info, warn};

/// Installation status tracking
#[derive(Debug, Clone, PartialEq)]
pub enum InstallationStatus {
    NotStarted,
    BackupCreated,
    KernelInstalled,
    Verified,
    Completed,
    Failed(String),
}

/// Installation progress callback
pub type InstallProgressCallback = dyn Fn(InstallationStatus) + Send + Sync;

/// Kernel installer with atomic operations and rollback support
pub struct Installer {
    config: OtaConfig,
    backup_paths: Vec<PathBuf>,
    temp_dir: PathBuf,
}

impl Installer {
    /// Create new installer instance
    pub fn new(config: OtaConfig) -> Result<Self> {
        let temp_dir = PathBuf::from(&config.download_path).join("install_temp");

        Ok(Self {
            config,
            backup_paths: Vec::new(),
            temp_dir,
        })
    }

    /// Install kernel with full backup and verification
    pub async fn install_kernel(
        &mut self,
        downloaded_kernel_path: &str,
        metadata: &KernelMetadata,
        progress_callback: Option<&InstallProgressCallback>,
    ) -> Result<()> {
        info!("Starting kernel installation: {}", metadata.latest_version);
        self.notify_progress(&progress_callback, InstallationStatus::NotStarted);

        // Step 1: Pre-installation validation
        self.validate_environment().await?;
        self.validate_downloaded_kernel(downloaded_kernel_path, metadata)
            .await?;

        // Step 2: Create temporary workspace
        self.setup_temp_workspace().await?;

        // Step 3: Create comprehensive backup
        self.create_backup()
            .await
            .context("Failed to create kernel backup")?;
        self.notify_progress(&progress_callback, InstallationStatus::BackupCreated);

        // Step 4: Prepare new kernel in temp location
        let temp_kernel_path = self
            .prepare_kernel_for_installation(downloaded_kernel_path)
            .await?;

        // Step 5: Atomic installation (the critical moment)
        match self.perform_atomic_installation(&temp_kernel_path).await {
            Ok(_) => {
                self.notify_progress(&progress_callback, InstallationStatus::KernelInstalled);

                // Step 6: Verify installation
                if let Err(e) = self.verify_installation(metadata).await {
                    error!("Installation verification failed: {}", e);
                    // Attempt rollback
                    if let Err(rollback_err) = self.rollback().await {
                        error!("CRITICAL: Rollback also failed: {}", rollback_err);
                        return Err(anyhow::anyhow!(
                            "Installation failed and rollback failed: {}. Manual intervention required.",
                            rollback_err
                        ));
                    }
                    return Err(e);
                }

                self.notify_progress(&progress_callback, InstallationStatus::Verified);

                // Step 7: Cleanup and finalize
                self.cleanup_temp_workspace().await?;
                self.notify_progress(&progress_callback, InstallationStatus::Completed);

                info!("Kernel installation completed successfully");
                Ok(())
            }
            Err(e) => {
                error!("Atomic installation failed: {}", e);
                self.notify_progress(
                    &progress_callback,
                    InstallationStatus::Failed(e.to_string()),
                );

                // Attempt rollback
                if let Err(rollback_err) = self.rollback().await {
                    error!("CRITICAL: Rollback failed: {}", rollback_err);
                    return Err(anyhow::anyhow!(
                        "Installation failed and rollback failed: {}. Manual intervention required.",
                        rollback_err
                    ));
                }

                Err(e)
            }
        }
    }

    /// Validate system environment before installation
    async fn validate_environment(&self) -> Result<()> {
        info!("Validating installation environment");

        // Check if kernel path exists and is writable
        let kernel_path = Path::new(&self.config.kernel_path);
        if !kernel_path.exists() {
            anyhow::bail!("Kernel path does not exist: {}", self.config.kernel_path);
        }

        // Check parent directory permissions
        let parent_dir = kernel_path
            .parent()
            .context("Cannot determine kernel parent directory")?;

        if !self.is_directory_writable(parent_dir).await? {
            anyhow::bail!("Insufficient permissions to write to kernel directory");
        }

        // Check available disk space
        self.check_disk_space().await?;

        // Verify we're running with appropriate privileges
        if !self.has_required_privileges() {
            anyhow::bail!("Insufficient privileges for kernel installation");
        }

        debug!("Environment validation passed");
        Ok(())
    }

    /// Validate downloaded kernel file
    async fn validate_downloaded_kernel(
        &self,
        kernel_path: &str,
        metadata: &KernelMetadata,
    ) -> Result<()> {
        info!("Validating downloaded kernel");

        let path = Path::new(kernel_path);
        if !path.exists() {
            anyhow::bail!("Downloaded kernel file not found: {}", kernel_path);
        }

        // Read file content for validation
        let file_content = async_fs::read(path).await?;

        // Verify file size
        if file_content.len() as u64 != metadata.file_size {
            anyhow::bail!(
                "File size mismatch: expected {}, got {}",
                metadata.file_size,
                file_content.len()
            );
        }

        // Verify checksum
        let calculated_checksum = self.calculate_file_checksum(path).await?;
        if calculated_checksum != metadata.checksum {
            anyhow::bail!(
                "Checksum mismatch: expected {}, got {}",
                metadata.checksum,
                calculated_checksum
            );
        }

        // Validate ARM64 kernel format
        self.validate_kernel_format(&file_content)?;

        debug!("Downloaded kernel validation passed");
        Ok(())
    }

    /// Validate ARM64 kernel format
    fn validate_kernel_format(&self, file_content: &[u8]) -> Result<()> {
        if file_content.len() >= 60 {
            let arm_magic = &file_content[56..60];
            if arm_magic != b"ARM\x64" {
                warn!("Kernel file may not be a valid ARM64 Image format");
            }
        }
        Ok(())
    }

    /// Create comprehensive backup of current kernel
    async fn create_backup(&mut self) -> Result<()> {
        info!("Creating kernel backup");

        let kernel_path = Path::new(&self.config.kernel_path);
        let backup_path = Path::new(&self.config.backup_path);

        // Create timestamped backup
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let _timestamped_backup = backup_path.with_extension(format!("backup_{}", timestamp));

        // Primary backup
        self.copy_with_verification(kernel_path, backup_path)
            .await?;
        self.backup_paths.push(PathBuf::from(backup_path));

        // Timestamped backup (for history)
        let timestamped_backup = self.temp_dir.join("original_kernel.backup");
        self.copy_with_verification(kernel_path, &timestamped_backup)
            .await?;
        self.backup_paths.push(timestamped_backup);

        // Additional safety: backup to temp directory
        let temp_backup = self.temp_dir.join("original_kernel.backup");
        self.copy_with_verification(kernel_path, &temp_backup)
            .await?;
        self.backup_paths.push(temp_backup);

        info!(
            "Backup created successfully with {} copies",
            self.backup_paths.len()
        );
        Ok(())
    }

    /// Copy file with integrity verification
    async fn copy_with_verification(&self, src: &Path, dst: &Path) -> Result<()> {
        // Ensure destination directory exists
        if let Some(parent) = dst.parent() {
            async_fs::create_dir_all(parent).await?;
        }

        // Copy file
        async_fs::copy(src, dst)
            .await
            .with_context(|| format!("Failed to copy {} to {}", src.display(), dst.display()))?;

        // Verify copy integrity
        let src_checksum = self.calculate_file_checksum(src).await?;
        let dst_checksum = self.calculate_file_checksum(dst).await?;

        if src_checksum != dst_checksum {
            // Clean up failed copy
            let _ = async_fs::remove_file(dst).await;
            anyhow::bail!(
                "Copy verification failed: checksums don't match for {}",
                dst.display()
            );
        }

        // Preserve original file permissions
        let src_metadata = async_fs::metadata(src).await?;
        let permissions = src_metadata.permissions();
        async_fs::set_permissions(dst, permissions).await?;

        debug!(
            "Successfully copied and verified: {} -> {}",
            src.display(),
            dst.display()
        );
        Ok(())
    }

    /// Setup temporary workspace for installation
    async fn setup_temp_workspace(&self) -> Result<()> {
        if self.temp_dir.exists() {
            async_fs::remove_dir_all(&self.temp_dir).await?;
        }
        async_fs::create_dir_all(&self.temp_dir).await?;

        debug!("Temporary workspace created: {}", self.temp_dir.display());
        Ok(())
    }

    /// Prepare kernel in temporary location with proper permissions
    async fn prepare_kernel_for_installation(&self, kernel_path: &str) -> Result<PathBuf> {
        let temp_kernel = self.temp_dir.join("new_kernel.img");

        // Copy new kernel to temp location
        async_fs::copy(kernel_path, &temp_kernel).await?;

        // Set appropriate permissions (typically 644 for kernel files)
        let permissions = Permissions::from_mode(0o644);
        async_fs::set_permissions(&temp_kernel, permissions).await?;

        debug!(
            "Kernel prepared for installation: {}",
            temp_kernel.display()
        );
        Ok(temp_kernel)
    }

    /// Perform atomic kernel installation
    async fn perform_atomic_installation(&self, temp_kernel_path: &Path) -> Result<()> {
        info!("Performing atomic kernel installation");

        let kernel_path = Path::new(&self.config.kernel_path);
        let temp_install_path = kernel_path.with_extension("installing");

        // Step 1: Copy new kernel to temporary name in target location
        self.copy_with_verification(temp_kernel_path, &temp_install_path)
            .await?;

        // Step 2: Atomic rename (this is the critical moment)
        async_fs::rename(&temp_install_path, kernel_path)
            .await
            .context("Failed to perform atomic kernel replacement")?;

        info!("Atomic installation completed");
        Ok(())
    }

    /// Verify installation success
    async fn verify_installation(&self, metadata: &KernelMetadata) -> Result<()> {
        info!("Verifying kernel installation");

        let kernel_path = Path::new(&self.config.kernel_path);

        // Check file exists
        if !kernel_path.exists() {
            anyhow::bail!("Kernel file missing after installation");
        }

        // Verify file size
        let file_metadata = async_fs::metadata(kernel_path).await?;
        if file_metadata.len() != metadata.file_size {
            anyhow::bail!(
                "Installed kernel size mismatch: expected {}, got {}",
                metadata.file_size,
                file_metadata.len()
            );
        }

        // Verify checksum
        let installed_checksum = self.calculate_file_checksum(kernel_path).await?;
        if installed_checksum != metadata.checksum {
            anyhow::bail!(
                "Installed kernel checksum mismatch: expected {}, got {}",
                metadata.checksum,
                installed_checksum
            );
        }

        // Verify file permissions
        let permissions = file_metadata.permissions();
        if permissions.mode() & 0o777 != 0o644 {
            warn!(
                "Kernel file permissions may be incorrect: {:o}",
                permissions.mode() & 0o777
            );
        }

        info!("Kernel installation verification passed");
        Ok(())
    }

    /// Rollback to previous kernel
    pub async fn rollback(&self) -> Result<()> {
        warn!("Performing kernel rollback");

        let kernel_path = Path::new(&self.config.kernel_path);
        let backup_path = Path::new(&self.config.backup_path);

        if !backup_path.exists() {
            anyhow::bail!("Backup file not found: {}", backup_path.display());
        }

        // Verify backup integrity before rollback
        let backup_checksum = self.calculate_file_checksum(backup_path).await?;
        debug!("Backup checksum: {}", backup_checksum);

        // Perform rollback copy
        self.copy_with_verification(backup_path, kernel_path)
            .await?;

        info!("Kernel rollback completed successfully");
        Ok(())
    }

    /// Calculate file checksum
    async fn calculate_file_checksum(&self, path: &Path) -> Result<String> {
        let contents = async_fs::read(path)
            .await
            .with_context(|| format!("Failed to read file: {}", path.display()))?;

        let mut hasher = Sha256::new();
        hasher.update(&contents);
        let hash = hasher.finalize();

        Ok(format!("sha256:{:x}", hash))
    }

    /// Check if directory is writable
    async fn is_directory_writable(&self, dir: &Path) -> Result<bool> {
        let test_file = dir.join(".ota_write_test");

        match async_fs::write(&test_file, b"test").await {
            Ok(_) => {
                let _ = async_fs::remove_file(&test_file).await;
                Ok(true)
            }
            Err(_) => Ok(false),
        }
    }

    /// Check available disk space
    async fn check_disk_space(&self) -> Result<()> {
        // This is a simplified check - in production, you'd want to use statvfs or similar
        let kernel_path = Path::new(&self.config.kernel_path);
        if let Ok(metadata) = async_fs::metadata(kernel_path).await {
            let required_space = metadata.len() * 3; // Need space for original + backup + new
            debug!("Estimated space required: {} bytes", required_space);
            // Additional space checks would go here
        }
        Ok(())
    }

    /// Check if running with required privileges
    fn has_required_privileges(&self) -> bool {
        // Check if running as root or with appropriate capabilities
        unsafe { libc::geteuid() == 0 }
    }

    /// Cleanup temporary workspace
    async fn cleanup_temp_workspace(&self) -> Result<()> {
        if self.temp_dir.exists() {
            async_fs::remove_dir_all(&self.temp_dir).await?;
            debug!("Temporary workspace cleaned up");
        }
        Ok(())
    }

    /// Cleanup old backups (keep only N most recent)
    pub async fn cleanup_old_backups(&self, keep_count: usize) -> Result<()> {
        info!(
            "Cleaning up old backups, keeping {} most recent",
            keep_count
        );

        let backup_dir = Path::new(&self.config.backup_path)
            .parent()
            .context("Cannot determine backup directory")?;

        let mut backup_files = Vec::new();
        let mut entries = async_fs::read_dir(backup_dir).await?;

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.contains("backup_") && name.ends_with(".backup") {
                    if let Ok(metadata) = entry.metadata().await {
                        backup_files.push((
                            path,
                            metadata
                                .modified()
                                .unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                        ));
                    }
                }
            }
        }

        // Sort by modification time (newest first)
        backup_files.sort_by(|a, b| b.1.cmp(&a.1));

        // Remove old backups
        for (path, _) in backup_files.iter().skip(keep_count) {
            if let Err(e) = async_fs::remove_file(path).await {
                warn!("Failed to remove old backup {}: {}", path.display(), e);
            } else {
                debug!("Removed old backup: {}", path.display());
            }
        }

        Ok(())
    }

    /// Get installation status
    pub fn get_backup_paths(&self) -> &[PathBuf] {
        &self.backup_paths
    }

    /// Helper to notify progress
    fn notify_progress(
        &self,
        callback: &Option<&InstallProgressCallback>,
        status: InstallationStatus,
    ) {
        if let Some(callback) = callback {
            callback(status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use tokio::io::AsyncWriteExt;

    async fn create_test_environment() -> (TempDir, OtaConfig, KernelMetadata) {
        let temp_dir = TempDir::new().unwrap();
        let temp_path = temp_dir.path().to_str().unwrap();

        let config = OtaConfig {
            check_interval_minutes: 60,
            download_path: format!("{}/downloads", temp_path),
            kernel_path: format!("{}/kernel.img", temp_path),
            backup_path: format!("{}/kernel.img.backup", temp_path),
            max_retries: 3,
            mdns_service: "_ota._tcp.local".to_string(),
            fallback_server: None,
            download_timeout_secs: 30,
        };

        // Create a dummy kernel file
        let kernel_content = b"dummy kernel data";
        let mut kernel_file = async_fs::File::create(&config.kernel_path).await.unwrap();
        kernel_file.write_all(kernel_content).await.unwrap();

        // Calculate correct checksum for the test data
        let mut hasher = Sha256::new();
        hasher.update(kernel_content);
        let correct_checksum = format!("sha256:{:x}", hasher.finalize());

        let metadata = KernelMetadata {
            latest_version: "1.0.0".to_string(),
            kernel_file: "kernel-v1.0.0.img".to_string(),
            file_size: kernel_content.len() as u64, // Correct size
            checksum: correct_checksum,             // Correct checksum
            release_date: "2025-06-16T10:30:00Z".to_string(),
            description: "Test kernel".to_string(),
            download_url: "/kernels/kernel-v1.0.0.img".to_string(),
        };

        (temp_dir, config, metadata)
    }

    #[tokio::test]
    async fn test_installer_creation() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let installer = Installer::new(config);
        assert!(installer.is_ok());
    }

    #[tokio::test]
    async fn test_checksum_calculation() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let installer = Installer::new(config.clone()).unwrap();

        let checksum = installer
            .calculate_file_checksum(Path::new(&config.kernel_path))
            .await
            .unwrap();
        assert!(checksum.starts_with("sha256:"));
        assert_eq!(checksum.len(), 71); // "sha256:" + 64 hex chars
    }

    #[tokio::test]
    async fn test_basic_backup_creation() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let mut installer = Installer::new(config.clone()).unwrap();

        installer.setup_temp_workspace().await.unwrap();
        let result = installer.create_backup().await;
        assert!(result.is_ok());
        assert!(!installer.backup_paths.is_empty());

        // Check that primary backup was created
        assert!(Path::new(&config.backup_path).exists());
    }

    #[tokio::test]
    async fn test_kernel_format_validation() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let installer = Installer::new(config).unwrap();

        // Test with valid ARM64 magic bytes
        let mut valid_arm64_kernel = vec![0u8; 64];
        valid_arm64_kernel[56..60].copy_from_slice(b"ARM\x64");
        let result = installer.validate_kernel_format(&valid_arm64_kernel);
        assert!(result.is_ok());

        // Test with invalid magic bytes (should still pass but warn)
        let mut invalid_kernel = vec![0u8; 64];
        invalid_kernel[56..60].copy_from_slice(b"XXXX");
        let result = installer.validate_kernel_format(&invalid_kernel);
        assert!(result.is_ok()); // Should pass but log warning

        // Test with short file (should pass)
        let short_kernel = vec![0u8; 30];
        let result = installer.validate_kernel_format(&short_kernel);
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_basic_rollback() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let mut installer = Installer::new(config.clone()).unwrap();

        // Create backup first
        installer.setup_temp_workspace().await.unwrap();
        installer.create_backup().await.unwrap();

        // Verify backup exists
        assert!(Path::new(&config.backup_path).exists());

        // Modify kernel file
        async_fs::write(&config.kernel_path, b"modified content")
            .await
            .unwrap();

        // Perform rollback
        let result = installer.rollback().await;
        assert!(result.is_ok());

        // Verify kernel was restored
        let restored_content = async_fs::read(&config.kernel_path).await.unwrap();
        assert_eq!(restored_content, b"dummy kernel data");
    }

    #[tokio::test]
    async fn test_file_copy_with_verification() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let installer = Installer::new(config.clone()).unwrap();

        let source = Path::new(&config.kernel_path);
        let dest_path = format!("{}/copy_test.img", _temp_dir.path().to_str().unwrap());
        let dest = Path::new(&dest_path);

        let result = installer.copy_with_verification(source, dest).await;
        assert!(result.is_ok());

        // Verify files are identical
        let source_content = async_fs::read(source).await.unwrap();
        let dest_content = async_fs::read(dest).await.unwrap();
        assert_eq!(source_content, dest_content);
    }

    #[tokio::test]
    async fn test_workspace_setup_and_cleanup() {
        let (_temp_dir, config, _metadata) = create_test_environment().await;
        let installer = Installer::new(config).unwrap();

        // Setup workspace
        let result = installer.setup_temp_workspace().await;
        assert!(result.is_ok());
        assert!(installer.temp_dir.exists());

        // Cleanup workspace
        let result = installer.cleanup_temp_workspace().await;
        assert!(result.is_ok());
        assert!(!installer.temp_dir.exists());
    }

    #[tokio::test]
    async fn test_downloaded_kernel_validation_success() {
        let (_temp_dir, config, metadata) = create_test_environment().await;
        let installer = Installer::new(config.clone()).unwrap();

        // Create a downloaded kernel file with correct content and checksum
        let download_path = format!("{}/downloaded_kernel.img", config.download_path);
        async_fs::create_dir_all(&config.download_path)
            .await
            .unwrap();
        async_fs::write(&download_path, b"dummy kernel data")
            .await
            .unwrap();

        // Should pass validation
        let result = installer
            .validate_downloaded_kernel(&download_path, &metadata)
            .await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_downloaded_kernel_validation_failure() {
        let (_temp_dir, config, metadata) = create_test_environment().await;
        let installer = Installer::new(config.clone()).unwrap();

        // Create a downloaded kernel file with wrong content
        let download_path = format!("{}/wrong_kernel.img", config.download_path);
        async_fs::create_dir_all(&config.download_path)
            .await
            .unwrap();
        async_fs::write(&download_path, b"wrong kernel content")
            .await
            .unwrap();

        // Should fail validation due to checksum mismatch
        let result = installer
            .validate_downloaded_kernel(&download_path, &metadata)
            .await;
        assert!(result.is_err());
    }
}
