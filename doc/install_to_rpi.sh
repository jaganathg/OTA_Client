#!/bin/bash

# Script to install custom kernel on Raspberry Pi
# This will install the kernel with "-v8_Jagan_Build" suffix

set -e  # Exit on error

echo "=== Installing custom kernel with -v8_Jagan_Build suffix ==="

# 1. Backup original kernel
echo "Backing up original kernel..."
sudo cp /boot/firmware/kernel8.img /boot/firmware/kernel8.img.original

# 2. Install custom kernel
echo "Installing custom kernel..."
sudo cp kernel_files/boot/kernel8.img /boot/firmware/kernel8.img

# 3. Copy DTB files
echo "Copying device tree files..."
sudo cp kernel_files/boot/bcm2711-rpi-4-b.dtb /boot/firmware/
sudo cp kernel_files/boot/bcm2711-rpi-400.dtb /boot/firmware/
sudo cp kernel_files/boot/bcm2711-rpi-cm4.dtb /boot/firmware/

# 4. Copy overlay files
echo "Copying overlay files..."
sudo cp -r kernel_files/boot/overlays/* /boot/firmware/overlays/

# 5. Install kernel modules
echo "Installing kernel modules..."
sudo mkdir -p /lib/modules/6.12.29-v8_Jagan_Build+
sudo cp -r kernel_files/lib/modules/6.12.29-v8_Jagan_Build+/* /lib/modules/6.12.29-v8_Jagan_Build+/

# 5.5. Clean up macOS metadata files (fixes depmod errors)
echo "Cleaning up macOS metadata files..."
sudo find /lib/modules/6.12.29-v8_Jagan_Build+ -name '._*' -delete

# 6. Update module dependencies
echo "Updating module dependencies..."
sudo depmod -a 6.12.29-v8_Jagan_Build+

# 7. Update cmdline.txt to make sure it references the right modules
echo "Creating backup of cmdline.txt..."
sudo cp /boot/firmware/cmdline.txt /boot/firmware/cmdline.txt.backup

# 8. Create backup of config.txt
echo "Creating backup of config.txt..."
sudo cp /boot/firmware/config.txt /boot/firmware/config.txt.backup

# 9. Update config.txt to ensure it loads the right kernel
echo "Updating config.txt to use custom kernel..."
if ! grep -q "^kernel=" /boot/firmware/config.txt; then
  # If kernel= line doesn't exist, add it
  echo "kernel=kernel8.img" | sudo tee -a /boot/firmware/config.txt
fi

echo ""
echo "===== Installation complete! ====="
echo "Your custom kernel has been installed."
echo "Original kernel is backed up at /boot/firmware/kernel8.img.original"
echo "Original cmdline.txt is backed up at /boot/firmware/cmdline.txt.backup"
echo "Original config.txt is backed up at /boot/firmware/config.txt.backup"
echo ""
echo "To reboot and use the new kernel, run: sudo reboot"
echo "After reboot, verify with: uname -a"
echo "You should see '6.12.29-v8_Jagan_Build+' in the output"
echo ""
echo "If the kernel fails to boot, you can restore the original by:"
echo "1. Shutting down and removing the SD card"
echo "2. On another computer, mount the boot partition and rename kernel8.img.original back to kernel8.img"
echo "3. Reinsert the SD card and boot again" 