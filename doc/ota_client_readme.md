# OTA Client Project Documentation

## Project Overview

**Goal**: Create a Rust-based OTA (Over-The-Air) client that runs on Raspberry Pi 4 to automatically fetch and install kernel updates from a MacBook server.

## Architecture & Design Decisions

### Core Requirements (from previous discussion):
1. **mDNS Discovery**: Auto-discover MacBook server using `_ota._tcp.local` service
2. **Update Frequency**: Every 1 hour (customizable in minutes)
3. **Installation**: Auto-installation of updates
4. **Backup Strategy**: Keep 1 previous kernel as backup for rollback
5. **Configuration**: `/etc/ota-client/config.toml`

### Cross-Compilation Setup:
- **Development**: MacBook (Intel/M1/M2)
- **Target**: Raspberry Pi 4 (ARM64)
- **Tool**: `cross` with Docker for zero-setup cross-compilation
- **Command**: `cross build --target aarch64-unknown-linux-gnu --release`

## Project Structure

```
ota_client/
├── Cargo.toml              ✅ Dependencies configured
├── src/
│   ├── lib.rs              ✅ Module organization (simple approach)
│   ├── main.rs             ✅ CLI entry point with tests
│   ├── types.rs            ✅ Data structures with tests
│   ├── config.rs           ✅ Configuration management with tests
│   ├── downloader.rs       ✅ HTTP client + mDNS discovery with tests
│   ├── installer.rs        ✅ Kernel installation logic with tests
│   └── daemon.rs           ✅ Background daemon service with tests
├── config/
│   └── client.toml         ✅ Runtime configuration template
└── systemd/
    └── ota-client.service  ✅ Systemd service file
```

## Implementation Status

### ✅ **Completed Modules:**

#### 1. **lib.rs**
- Simple module organization (following ota_server pattern)
- No unnecessary re-exports or complexity

#### 2. **types.rs** 
**Features:**
- `OtaConfig`: Configuration structure with defaults
- `ServerInfo`: mDNS server discovery results  
- `KernelMetadata`: Server response metadata
- `DownloadProgress`: Progress tracking during downloads
- `OtaResult`: Operation result enum
- `Cli` & `Commands`: Command-line interface structure

**Testing:** 10 comprehensive unit tests covering serialization, validation, and data structure creation

#### 3. **config.rs**
**Features:**
- `load_config()`: Load from TOML file or create default
- `create_default_config()`: Generate default configuration
- `validate_config()`: Validate configuration values
- `save_config()`: Save configuration changes

**Testing:** 8 unit tests covering file operations, validation, and roundtrip serialization

#### 4. **downloader.rs**
**Features:**
- mDNS service discovery for MacBook server
- Fallback to manual server configuration
- HTTP client with progress tracking
- SHA256 checksum verification
- Retry logic with exponential backoff
- Server connectivity testing

**Testing:** 10 unit tests covering discovery, download logic, and checksum verification

#### 5. **installer.rs** 
**Features:**
- Safe kernel backup with verification (copy current to `.backup`)
- Atomic kernel replacement using temporary files
- Rollback capability to previous kernel
- Comprehensive file system operations with error handling
- ARM64 kernel format validation
- File integrity verification (SHA256 checksums)
- Proper permission handling and privilege checking
- Multiple backup copies for extra safety

**Testing:** 10 unit tests covering backup creation, rollback, validation, and atomic operations

#### 6. **daemon.rs**
**Features:**
- Background service main loop with Arc<Self> pattern
- Periodic update checking (configurable interval)
- Integration of downloader + installer with retry logic
- Comprehensive error handling and logging
- Signal handling (SIGTERM for shutdown, SIGHUP for config reload)
- Update history tracking and persistence
- Timeout protection for download operations (90 seconds default)
- State management and status reporting
- Automatic rollback on installation failures

**Testing:** 8 unit tests covering daemon creation, state transitions, update history, config reload, shutdown, rollback decisions, and status reporting

#### 7. **main.rs**
**Features:**
- CLI command parsing using clap with structured help
- 5 subcommands: `daemon`, `check`, `update`, `status`, `rollback`
- Configuration file handling with auto-creation of defaults
- Structured logging setup with environment variable support
- Integration with all modules (daemon, downloader, installer)
- Error handling and user-friendly status messages
- Automatic config directory creation

**Testing:** Manual CLI testing completed - all commands working correctly

### ✅ **Supporting Files:**

#### 8. **config/client.toml**
**Features:**
- Comprehensive configuration template with detailed comments
- Development and production path examples
- Advanced configuration options (commented out)
- Raspberry Pi specific kernel paths
- Network timeout and retry settings
- mDNS service discovery configuration
- Optional fallback server configuration

#### 9. **systemd/ota-client.service**
**Features:**
- Production-ready systemd service definition
- Automatic restart and failure recovery
- Security settings and resource limits
- Proper logging to systemd journal
- Signal handling for graceful shutdown and config reload
- Complete installation and management instructions
- Network dependency handling

## Current Issues & Fixes Applied

### Configuration Path Strategy:
- **Development/Testing**: Use local paths (`./downloads`, `/tmp/ota_test_*`)
- **Production**: System paths (`/etc/ota-client/`, `/opt/ota/`)
- **Tests**: Temporary paths to avoid permission issues

### Testing Strategy:
- Each module contains comprehensive unit tests
- Uses `#[tokio::test]` for async functions
- Manual cleanup in `/tmp/` instead of external test dependencies
- Follows ota_server project pattern

## Dependencies

```toml
[dependencies]
tokio = { version = "1.0", features = ["full"] }
reqwest = { version = "0.11", features = ["json", "stream"] }
serde = { version = "1.0", features = ["derive"] }
serde_json = "1.0"
toml = "0.8"
mdns = "3.0"
sha2 = "0.10"
clap = { version = "4.0", features = ["derive"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
anyhow = "1.0"
tokio-util = { version = "0.7", features = ["codec"] }
tokio-stream = "0.1"
chrono = { version = "0.4", features = ["serde"] }
```

## Build & Test Commands

### Development Testing:
```bash
# Test specific module
cargo test types
cargo test config
cargo test downloader

# Test all modules
cargo test

# Check compilation without running tests
cargo check
```

### Cross-Compilation for RPi:
```bash
# Install cross tool (one-time setup)
cargo install cross

# Set Docker platform for Apple Silicon Macs
export DOCKER_DEFAULT_PLATFORM=linux/amd64

# Cross-compile for Raspberry Pi ARM64
cross build --target aarch64-unknown-linux-gnu --release

# Output binary location
ls target/aarch64-unknown-linux-gnu/release/ota_client
```

## Next Implementation Steps

1. **Integration testing** - End-to-end workflow testing (Current Priority)
2. **Cross-compilation testing** - Verify ARM binaries work on RPi
3. **Documentation updates** - Usage examples and troubleshooting
4. **Performance optimization** - Memory usage and error handling improvements

## Deployment Workflow

### Quick Deployment (Automated)
```bash
# 1. Cross-compile for RPi
cross build --target aarch64-unknown-linux-gnu --release

# 2. Deploy to RPi (automated script)
./doc/deploy_to_rpi.sh 192.168.1.100 pi

# 3. Configure and start on RPi
ssh pi@192.168.1.100
sudo nano /etc/ota-client/config.toml
sudo systemctl enable ota-client
sudo systemctl start ota-client
```

### Detailed Steps
1. **Development**: `cargo test` on MacBook
2. **Cross-compile**: `cross build --target aarch64-unknown-linux-gnu --release`
3. **Deploy**: Use `./doc/deploy_to_rpi.sh` or manual transfer
4. **Install**: Automated via deployment script
5. **Configure**: Edit `/etc/ota-client/config.toml` on RPi
6. **Run**: `sudo systemctl start ota-client`

**📖 See [DEPLOYMENT.md](doc/DEPLOYMENT.md) for comprehensive deployment guide**

## Usage (Planned CLI Interface)

```bash
# Run as background daemon
ota-client daemon --config /etc/ota-client/config.toml

# Check for updates once
ota-client check

# Force update download and installation
ota-client update

# Show current status
ota-client status

# Rollback to previous kernel
ota-client rollback
```

## Integration with OTA Server

This client is designed to work with the companion `ota_server` project:

- **Server**: Runs on MacBook, serves kernel files via HTTP
- **Client**: Runs on RPi, discovers server via mDNS and fetches updates
- **Protocol**: HTTP with SHA256 checksum verification
- **Discovery**: mDNS service `_ota._tcp.local` with fallback to manual IP

## Security Features

- **Checksum Verification**: SHA256 validation of downloaded kernels
- **Atomic Updates**: Safe kernel replacement with backup
- **Rollback Capability**: Automatic fallback to previous working kernel
- **Local Network Only**: mDNS discovery restricts to local network
- **File Permissions**: Proper handling of system file permissions

The project follows the same modular, well-tested approach as the ota_server implementation.