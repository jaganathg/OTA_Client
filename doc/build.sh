#!/bin/bash

# Exit on error
set -e

# Make sure we're in the right directory
cd "$(dirname "$0")"

# Set up environment variables
export KERNEL=kernel8
export ARCH=arm64
export CROSS_COMPILE=aarch64-linux-gnu-

# Check for clean build flag
CLEAN_BUILD=0
if [ "$1" == "--clean" ]; then
    CLEAN_BUILD=1
    shift
fi

# Clean build environment if requested
if [ $CLEAN_BUILD -eq 1 ]; then
    echo "Performing clean build..."
    echo "Cleaning build environment..."
    make mrproper
    rm -f .config
    rm -rf modules_install
    mkdir -p modules_install
fi

# Apply configuration if .config doesn't exist
if [ ! -f .config ]; then
    echo "No .config found, applying configuration..."
    # Apply configuration fixes (which also applies the bcm2711_defconfig)
    ./config_fix.sh
else
    echo "Using existing .config file"
    # Just ensure the config is up to date
    make olddefconfig
fi

# Build the kernel
echo "Building kernel..."
make -j$(nproc) Image modules dtbs

# Create the destination directories
echo "Creating destination directories..."
mkdir -p modules_install/lib/modules
mkdir -p modules_install/boot
mkdir -p modules_install/boot/overlays

# Install kernel modules
echo "Installing kernel modules..."
make INSTALL_MOD_PATH=./modules_install modules_install

# Copy kernel and device tree files
echo "Copying kernel image and device tree files..."
cp arch/arm64/boot/Image modules_install/boot/kernel8.img
cp arch/arm64/boot/dts/broadcom/bcm2711-rpi-4-b.dtb modules_install/boot/
cp arch/arm64/boot/dts/broadcom/bcm2711-rpi-400.dtb modules_install/boot/
cp arch/arm64/boot/dts/broadcom/bcm2711-rpi-cm4.dtb modules_install/boot/
cp arch/arm64/boot/dts/overlays/*.dtbo modules_install/boot/overlays/
cp arch/arm64/boot/dts/overlays/README modules_install/boot/overlays/

# Create firmware directory
echo "Creating firmware directory..."
mkdir -p modules_install/lib/firmware/brcm

# Install WiFi firmware blobs
echo "Installing WiFi firmware blobs..."
# Check if local firmware files exist first
if [ -d "firmware/brcm" ] && [ -f "firmware/brcm/brcmfmac43455-sdio.bin" ]; then
    echo "Using local firmware files..."
    cp firmware/brcm/brcmfmac43455-sdio.* modules_install/lib/firmware/brcm/
else
    echo "Downloading firmware files..."
    # Check if wget is available
    if command -v wget &> /dev/null; then
        wget -O modules_install/lib/firmware/brcm/brcmfmac43455-sdio.bin https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.bin
        wget -O modules_install/lib/firmware/brcm/brcmfmac43455-sdio.clm_blob https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.clm_blob
        wget -O modules_install/lib/firmware/brcm/brcmfmac43455-sdio.txt https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.txt
    elif command -v curl &> /dev/null; then
        echo "Using curl instead of wget..."
        curl -o modules_install/lib/firmware/brcm/brcmfmac43455-sdio.bin https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.bin
        curl -o modules_install/lib/firmware/brcm/brcmfmac43455-sdio.clm_blob https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.clm_blob
        curl -o modules_install/lib/firmware/brcm/brcmfmac43455-sdio.txt https://raw.githubusercontent.com/RPi-Distro/firmware-nonfree/buster/brcm/brcmfmac43455-sdio.txt
    else
        echo "ERROR: Neither wget nor curl is available. Cannot download firmware files."
        echo "Please install wget or curl, or manually download the firmware files to firmware/brcm/"
        echo "Build completed, but WiFi firmware is missing and WiFi will not work."
    fi
fi

# Display kernel version information
echo ""
echo "=============================================="
echo "Kernel Version Information:"
echo "=============================================="
VERSION=$(make kernelrelease)
echo "Full Kernel Version: $VERSION"
LOCALVERSION=$(grep CONFIG_LOCALVERSION .config | cut -d '=' -f 2 | tr -d '"')
echo "Local Version String: $LOCALVERSION"
echo "=============================================="
echo ""

echo "Build complete!"
echo "The kernel files are in the modules_install directory."
echo "Copy these files to your Raspberry Pi to install the new kernel."
echo ""
echo "To install the kernel to an SD card, run:"
echo "  sudo ./install_kernel.sh /path/to/boot/partition"
echo ""
echo "For a clean build next time, run:"
echo "  ./build.sh --clean" 