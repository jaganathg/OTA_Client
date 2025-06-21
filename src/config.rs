use crate::types::OtaConfig;
use anyhow::{Context, Result};
use std::path::Path;
use tracing::{info, warn};

/// Load configuration from file or create default
pub async fn load_config(config_path: &str) -> Result<OtaConfig> {
    let path = Path::new(config_path);

    if path.exists() {
        info!("Loading config from {}", config_path);
        let content = tokio::fs::read_to_string(path)
            .await
            .with_context(|| format!("Failed to read config file: {}", config_path))?;

        let config: OtaConfig = toml::from_str(&content)
            .with_context(|| format!("Failed to parse config file: {}", config_path))?;

        validate_config(&config).await?;
        Ok(config)
    } else {
        warn!("Config file not found, creating default: {}", config_path);
        create_default_config(config_path).await
    }
}

/// Create default configuration file
pub async fn create_default_config(config_path: &str) -> Result<OtaConfig> {
    let config = OtaConfig::default();

    // Create directory if it doesn't exist
    if let Some(parent) = Path::new(config_path).parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("Failed to create config directory: {:?}", parent))?;
    }

    // Write default config
    let toml_content =
        toml::to_string_pretty(&config).context("Failed to serialize default config")?;

    tokio::fs::write(config_path, toml_content)
        .await
        .with_context(|| format!("Failed to write config file: {}", config_path))?;

    info!("Created default config at {}", config_path);
    Ok(config)
}

/// Validate configuration values
async fn validate_config(config: &OtaConfig) -> Result<()> {
    if config.check_interval_minutes == 0 {
        anyhow::bail!("check_interval_minutes must be greater than 0");
    }

    if config.max_retries == 0 {
        anyhow::bail!("max_retries must be greater than 0");
    }

    if config.download_timeout_secs == 0 {
        anyhow::bail!("download_timeout_secs must be greater than 0");
    }

    // Validate paths exist or can be created
    let download_path = Path::new(&config.download_path);
    if let Some(parent) = download_path.parent() {
        if !parent.exists() {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("Cannot create download directory: {:?}", parent))?;
        }
    }

    Ok(())
}

/// Update configuration file with new values
pub async fn save_config(config: &OtaConfig, config_path: &str) -> Result<()> {
    let toml_content = toml::to_string_pretty(config).context("Failed to serialize config")?;

    tokio::fs::write(config_path, toml_content)
        .await
        .with_context(|| format!("Failed to write config file: {}", config_path))?;

    info!("Saved config to {}", config_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[tokio::test]
    async fn test_create_default_config() {
        let config_path = "/tmp/test_ota_config.toml";

        // Clean up before test
        let _ = fs::remove_file(config_path);

        let config = create_default_config(config_path).await.unwrap();

        assert_eq!(config.check_interval_minutes, 60);
        assert_eq!(config.max_retries, 3);
        assert!(std::path::Path::new(config_path).exists());

        // Clean up after test
        let _ = fs::remove_file(config_path);
    }

    #[tokio::test]
    async fn test_load_existing_config() {
        let config_path = "/tmp/test_ota_load_config.toml";

        // Create a test config file
        let test_config = r#"
check_interval_minutes = 30
download_path = "/tmp/ota"
kernel_path = "/boot/test_kernel.img"
backup_path = "/boot/test_kernel.img.backup"
max_retries = 5
mdns_service = "_test_ota._tcp.local"
download_timeout_secs = 600
"#;

        fs::write(config_path, test_config).unwrap();

        let config = load_config(config_path).await.unwrap();

        assert_eq!(config.check_interval_minutes, 30);
        assert_eq!(config.download_path, "/tmp/ota");
        assert_eq!(config.max_retries, 5);
        assert_eq!(config.mdns_service, "_test_ota._tcp.local");
        assert_eq!(config.download_timeout_secs, 600);

        // Clean up
        let _ = fs::remove_file(config_path);
    }

    #[tokio::test]
    async fn test_validate_config_success() {
        // Use a config with download path that doesn't require creating system directories
        let mut config = OtaConfig::default();
        config.download_path = "/tmp".to_string(); // Use existing /tmp directory

        let result = validate_config(&config).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn test_validate_config_failure() {
        let mut config = OtaConfig::default();

        // Test zero check interval
        config.check_interval_minutes = 0;
        assert!(validate_config(&config).await.is_err());

        // Reset and test zero retries
        config = OtaConfig::default();
        config.max_retries = 0;
        assert!(validate_config(&config).await.is_err());

        // Reset and test zero timeout
        config = OtaConfig::default();
        config.download_timeout_secs = 0;
        assert!(validate_config(&config).await.is_err());
    }

    #[tokio::test]
    async fn test_nonexistent_config_creates_default() {
        let config_path = "/tmp/test_ota_nonexistent_config.toml";

        // Ensure file doesn't exist
        let _ = fs::remove_file(config_path);

        let config = load_config(config_path).await.unwrap();

        // Should have created default config
        assert_eq!(config.check_interval_minutes, 60);
        assert!(std::path::Path::new(config_path).exists());

        // Clean up
        let _ = fs::remove_file(config_path);
    }
}
