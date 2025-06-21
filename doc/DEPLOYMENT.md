# OTA Client Deployment Guide for Raspberry Pi

This guide covers deploying the OTA client to a Raspberry Pi 4 running Raspberry Pi OS.

## Prerequisites

### On MacBook (Development Machine)
- Rust toolchain installed
- `cross` crate for cross-compilation
- Docker (required by `cross`)
- SSH access to Raspberry Pi

### On Raspberry Pi
- Raspberry Pi OS (64-bit recommended)
- SSH enabled
- Network connectivity
- Root/sudo access

## Quick Deployment (Automated)

### 1. Cross-Compile the Binary
```bash
# Install cross if not already installed
cargo install cross

# For Apple Silicon Macs
export DOCKER_DEFAULT_PLATFORM=linux/amd64

# Build ARM64 binary
cross build --target aarch64-unknown-linux-gnu --release
```

### 2. Deploy to Raspberry Pi
```bash
# Run the automated deployment script
./doc/deploy_to_rpi.sh [RPi_IP] [USERNAME]

# Example:
./doc/deploy_to_rpi.sh 192.168.1.100 pi
```

The script will:
- ✅ Verify all files are present
- ✅ Test SSH connection
- ✅ Transfer binary and configuration files
- ✅ Install everything in correct locations
- ✅ Set up systemd service
- ✅ Configure permissions

### 3. Configure and Start Service
```bash
# SSH to your Raspberry Pi
ssh pi@192.168.1.100

# Review and edit configuration
sudo nano /etc/ota-client/config.toml

# Enable and start the service
sudo systemctl enable ota-client
sudo systemctl start ota-client

# Check status
sudo systemctl status ota-client
```

## Manual Deployment (Step-by-Step)

If you prefer manual deployment or need to troubleshoot:

### 1. Cross-Compilation
```bash
# Install cross
cargo install cross

# Set Docker platform (Apple Silicon Macs only)
export DOCKER_DEFAULT_PLATFORM=linux/amd64

# Build for ARM64
cross build --target aarch64-unknown-linux-gnu --release

# Verify binary
ls -la target/aarch64-unknown-linux-gnu/release/ota_client
```

### 2. Transfer Files to RPi
```bash
# Create temporary directory
mkdir /tmp/ota_deploy
cd /tmp/ota_deploy

# Copy files
cp /path/to/ota_client/target/aarch64-unknown-linux-gnu/release/ota_client .
cp /path/to/ota_client/config/client.toml config.toml
cp /path/to/ota_client/systemd/ota-client.service .

# Transfer to RPi
scp * pi@192.168.1.100:/tmp/
```

### 3. Install on Raspberry Pi
```bash
# SSH to RPi
ssh pi@192.168.1.100

# Create directories
sudo mkdir -p /etc/ota-client
sudo mkdir -p /opt/ota/downloads
sudo mkdir -p /usr/local/bin

# Install binary
sudo cp /tmp/ota_client /usr/local/bin/
sudo chmod +x /usr/local/bin/ota_client

# Install configuration
sudo cp /tmp/config.toml /etc/ota-client/
sudo chown root:root /etc/ota-client/config.toml
sudo chmod 644 /etc/ota-client/config.toml

# Install systemd service
sudo cp /tmp/ota-client.service /etc/systemd/system/
sudo systemctl daemon-reload

# Set permissions
sudo chown -R root:root /opt/ota /etc/ota-client
```

### 4. Configuration
```bash
# Edit configuration for your environment
sudo nano /etc/ota-client/config.toml
```

Key settings to review:
```toml
# Update frequency (adjust as needed)
check_interval_minutes = 60

# Use production paths
download_path = "/opt/ota/downloads"
kernel_path = "/boot/kernel8.img"  # or kernel.img for 32-bit
backup_path = "/boot/kernel8.img.backup"

# Network settings
download_timeout_secs = 90
mdns_service = "_ota._tcp.local"

# Optional: Fallback server
# fallback_server = "http://192.168.1.100:8080"
```

### 5. Start and Enable Service
```bash
# Enable service to start on boot
sudo systemctl enable ota-client

# Start the service
sudo systemctl start ota-client

# Check status
sudo systemctl status ota-client

# View logs
sudo journalctl -u ota-client -f
```

## Service Management

### Common Commands
```bash
# Start service
sudo systemctl start ota-client

# Stop service
sudo systemctl stop ota-client

# Restart service
sudo systemctl restart ota-client

# Check status
sudo systemctl status ota-client

# View logs (real-time)
sudo journalctl -u ota-client -f

# View logs (last 50 lines)
sudo journalctl -u ota-client -n 50

# Reload configuration (sends SIGHUP)
sudo systemctl reload ota-client
```

### Manual Testing
```bash
# Check for updates
sudo ota_client check --config /etc/ota-client/config.toml

# Show status
sudo ota_client status --config /etc/ota-client/config.toml

# Force update (if available)
sudo ota_client update --config /etc/ota-client/config.toml

# Rollback to previous kernel
sudo ota_client rollback --config /etc/ota-client/config.toml
```

## Troubleshooting

### Binary Issues
```bash
# Check if binary is working
/usr/local/bin/ota_client --help

# Check binary architecture
file /usr/local/bin/ota_client
# Should show: ARM aarch64, dynamically linked
```

### Service Issues
```bash
# Check service status
sudo systemctl status ota-client

# Check service logs
sudo journalctl -u ota-client --no-pager

# Check configuration syntax
sudo ota_client check --config /etc/ota-client/config.toml
```

### Network Issues
```bash
# Test mDNS discovery manually
avahi-browse -r _ota._tcp

# Check if server is reachable
ping 192.168.1.100

# Test HTTP connectivity
curl http://192.168.1.100:8080/health
```

### Permission Issues
```bash
# Check file permissions
ls -la /etc/ota-client/
ls -la /opt/ota/
ls -la /usr/local/bin/ota_client

# Fix permissions if needed
sudo chown -R root:root /etc/ota-client /opt/ota
sudo chmod -R 755 /opt/ota
sudo chmod 644 /etc/ota-client/config.toml
sudo chmod +x /usr/local/bin/ota_client
```

## File Locations

After deployment, files are located at:

```
/usr/local/bin/ota_client          # Main binary
/etc/ota-client/config.toml        # Configuration file
/etc/systemd/system/ota-client.service  # Systemd service
/opt/ota/downloads/                # Download directory
/opt/ota/ota_update_history.json   # Update history log
/boot/kernel8.img                  # Current kernel (64-bit)
/boot/kernel8.img.backup           # Backup kernel
```

## Security Considerations

- Service runs as root (required for kernel updates)
- Network access restricted to local network via mDNS
- SHA256 checksums verify kernel integrity
- Automatic rollback on installation failures
- Backup kernels maintained for recovery

## Integration with OTA Server

Ensure your OTA server (running on MacBook) is:
1. Advertising `_ota._tcp.local` mDNS service
2. Serving on the correct port (default 8080)
3. Reachable from the RPi network

Test server connectivity:
```bash
# From RPi - test mDNS discovery
avahi-browse -r _ota._tcp

# Test HTTP endpoint
curl http://[SERVER_IP]:8080/health
```

This completes the deployment process. The OTA client will now automatically check for and install kernel updates according to your configuration! 