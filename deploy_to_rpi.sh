#!/bin/bash

# OTA Client Deployment Script for Raspberry Pi
# Usage: ./deploy_to_rpi.sh [RPi_IP_ADDRESS] [USERNAME]
# Example: ./deploy_to_rpi.sh 192.168.1.100 pi

set -e

# Configuration
RPI_IP="${1:-192.168.1.100}"
RPI_USER="${2:-pi}"
BINARY_PATH="target/aarch64-unknown-linux-gnu/release/ota_client"
CONFIG_TEMPLATE="config/client.toml"
SERVICE_FILE="systemd/ota-client.service"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${BLUE}=== OTA Client Deployment to Raspberry Pi ===${NC}"
echo "Target: ${RPI_USER}@${RPI_IP}"
echo

# Check if binary exists
if [ ! -f "$BINARY_PATH" ]; then
    echo -e "${RED}Error: ARM64 binary not found at $BINARY_PATH${NC}"
    echo "Please run: cross build --target aarch64-unknown-linux-gnu --release"
    exit 1
fi

# Check if config template exists
if [ ! -f "$CONFIG_TEMPLATE" ]; then
    echo -e "${RED}Error: Config template not found at $CONFIG_TEMPLATE${NC}"
    exit 1
fi

# Check if service file exists
if [ ! -f "$SERVICE_FILE" ]; then
    echo -e "${RED}Error: Service file not found at $SERVICE_FILE${NC}"
    exit 1
fi

echo -e "${GREEN}✓ All required files found${NC}"

# Test SSH connection
echo -e "${BLUE}Testing SSH connection...${NC}"
if ! ssh -o ConnectTimeout=10 "${RPI_USER}@${RPI_IP}" exit 2>/dev/null; then
    echo -e "${RED}Error: Cannot connect to ${RPI_USER}@${RPI_IP}${NC}"
    echo "Please ensure:"
    echo "1. RPi is running and connected to network"
    echo "2. SSH is enabled on RPi"
    echo "3. SSH key authentication is set up"
    exit 1
fi
echo -e "${GREEN}✓ SSH connection successful${NC}"

# Create deployment directory
DEPLOY_DIR="/tmp/ota_client_deploy"
mkdir -p "$DEPLOY_DIR"

# Copy files to deployment directory
echo -e "${BLUE}Preparing deployment files...${NC}"
cp "$BINARY_PATH" "$DEPLOY_DIR/"
cp "$CONFIG_TEMPLATE" "$DEPLOY_DIR/config.toml"
cp "$SERVICE_FILE" "$DEPLOY_DIR/"

# Create installation script
cat > "$DEPLOY_DIR/install.sh" << 'EOF'
#!/bin/bash
set -e

echo "=== Installing OTA Client on Raspberry Pi ==="

# Stop existing service if running
if systemctl is-active --quiet ota-client 2>/dev/null; then
    echo "Stopping existing ota-client service..."
    sudo systemctl stop ota-client
fi

# Create directories
echo "Creating directories..."
sudo mkdir -p /etc/ota-client
sudo mkdir -p /opt/ota/downloads
sudo mkdir -p /var/log/ota-client

# Install binary
echo "Installing binary..."
sudo cp ota_client /usr/local/bin/ota-client
sudo chmod +x /usr/local/bin/ota-client

# Install configuration
echo "Installing configuration..."
if [ ! -f /etc/ota-client/config.toml ]; then
    sudo cp config.toml /etc/ota-client/
    echo "Default configuration installed to /etc/ota-client/config.toml"
    echo "Please review and modify as needed!"
else
    echo "Configuration already exists at /etc/ota-client/config.toml"
    echo "Backup created at /etc/ota-client/config.toml.backup"
    sudo cp /etc/ota-client/config.toml /etc/ota-client/config.toml.backup
    sudo cp config.toml /etc/ota-client/config.toml.new
    echo "New configuration saved as /etc/ota-client/config.toml.new"
fi

# Install systemd service
echo "Installing systemd service..."
sudo cp ota-client.service /etc/systemd/system/
sudo systemctl daemon-reload

# Set permissions
echo "Setting permissions..."
sudo chown -R root:root /opt/ota /etc/ota-client
sudo chmod -R 755 /opt/ota
sudo chmod -R 644 /etc/ota-client/*.toml

# Test binary
echo "Testing binary..."
if /usr/local/bin/ota-client --help > /dev/null; then
    echo "✓ Binary installation successful"
else
    echo "✗ Binary test failed"
    exit 1
fi

echo
echo "=== Installation Complete ==="
echo
echo "Next steps:"
echo "1. Review configuration: sudo nano /etc/ota-client/config.toml"
echo "2. Enable service: sudo systemctl enable ota-client"
echo "3. Start service: sudo systemctl start ota-client"
echo "4. Check status: sudo systemctl status ota-client"
echo "5. View logs: sudo journalctl -u ota-client -f"
echo
echo "Manual commands:"
echo "  Check for updates: sudo ota-client check"
echo "  Force update: sudo ota-client update"
echo "  Show status: sudo ota-client status"
echo "  Rollback: sudo ota-client rollback"
EOF

chmod +x "$DEPLOY_DIR/install.sh"

# Transfer files to RPi
echo -e "${BLUE}Transferring files to RPi...${NC}"
scp -r "$DEPLOY_DIR"/* "${RPI_USER}@${RPI_IP}:/tmp/"

# Run installation on RPi
echo -e "${BLUE}Running installation on RPi...${NC}"
ssh "${RPI_USER}@${RPI_IP}" "cd /tmp && chmod +x install.sh && ./install.sh"

# Cleanup
rm -rf "$DEPLOY_DIR"

echo
echo -e "${GREEN}=== Deployment Complete! ===${NC}"
echo
echo -e "${YELLOW}Next steps on your Raspberry Pi:${NC}"
echo "1. SSH to RPi: ssh ${RPI_USER}@${RPI_IP}"
echo "2. Review config: sudo nano /etc/ota-client/config.toml"
echo "3. Enable service: sudo systemctl enable ota-client"
echo "4. Start service: sudo systemctl start ota-client"
echo "5. Check status: sudo systemctl status ota-client"
echo
echo -e "${YELLOW}Manual testing:${NC}"
echo "  sudo ota-client check --config /etc/ota-client/config.toml"
echo "  sudo ota-client status --config /etc/ota-client/config.toml"
echo
echo -e "${GREEN}Happy updating! 🚀${NC}" 