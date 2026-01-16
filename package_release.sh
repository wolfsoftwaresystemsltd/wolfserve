#!/bin/bash

# WolfServe Package Creator
# Creates a distributable package with precompiled binaries
# Usage: ./package_release.sh [output_dir]

set -e

OUTPUT_DIR="${1:-./release-package}"
PACKAGE_NAME="wolfserve-$(uname -m)-$(date +%Y%m%d)"

echo "🐺 WolfServe Release Packager"
echo "=============================="

# Check if binaries exist
if [ ! -f "target/release/wolfserve" ]; then
    echo "❌ wolfserve binary not found. Run 'cargo build --release' first."
    exit 1
fi

if [ ! -f "wolflib/target/release/libwolflib.so" ]; then
    echo "⚠️  libwolflib.so not found. Run './build_lib.sh' for PHP FFI support."
fi

# Create package directory
PACKAGE_DIR="$OUTPUT_DIR/$PACKAGE_NAME"
mkdir -p "$PACKAGE_DIR/public"

echo "📦 Creating package at: $PACKAGE_DIR"

# Copy binaries
echo "   Copying wolfserve binary..."
cp target/release/wolfserve "$PACKAGE_DIR/"

if [ -f "wolflib/target/release/libwolflib.so" ]; then
    echo "   Copying libwolflib.so..."
    cp wolflib/target/release/libwolflib.so "$PACKAGE_DIR/"
fi

# Copy config and installer
echo "   Copying configuration files..."
[ -f "wolfserve.toml.example" ] && cp wolfserve.toml.example "$PACKAGE_DIR/"
[ -f "wolfserve.toml" ] && cp wolfserve.toml "$PACKAGE_DIR/"
cp install_precompiled.sh "$PACKAGE_DIR/"
chmod +x "$PACKAGE_DIR/install_precompiled.sh"

# Copy public files
if [ -d "public" ]; then
    echo "   Copying public web files..."
    cp -r public/* "$PACKAGE_DIR/public/"
fi

# Create README for the package
cat > "$PACKAGE_DIR/README.txt" <<EOF
WolfServe Precompiled Release
==============================

This package contains precompiled WolfServe binaries.

Contents:
- wolfserve           : Main server binary
- libwolflib.so       : PHP FFI extension library (optional)
- wolfserve.toml      : Configuration file
- install_precompiled.sh : Installation script
- public/             : Sample web files

Quick Install:
--------------
1. Extract this package to any directory
2. Run: sudo ./install_precompiled.sh

This will:
- Install WolfServe to /opt/wolfserve
- Create a systemd service
- Configure PHP-FPM
- Start the server automatically

Manual Install:
---------------
1. Copy 'wolfserve' to your desired location
2. Copy 'wolfserve.toml' to the same directory
3. Create a 'public' folder for web files
4. Run: ./wolfserve

Requirements:
- Linux (systemd)
- PHP-FPM (will be installed automatically)
- Port 3000 (configurable in wolfserve.toml)
- Port 9993 for PHP-FPM

Supported Distributions:
- Debian/Ubuntu (apt)
- Fedora/RHEL/CentOS/Rocky (dnf)
- Arch Linux (pacman)
- openSUSE (zypper)

Built on: $(date)
Architecture: $(uname -m)
EOF

# Create tarball
echo ""
echo "📦 Creating tarball..."
cd "$OUTPUT_DIR"
tar -czvf "$PACKAGE_NAME.tar.gz" "$PACKAGE_NAME"
cd - > /dev/null

echo ""
echo "✅ Package created successfully!"
echo ""
echo "📁 Package directory: $PACKAGE_DIR"
echo "📦 Tarball: $OUTPUT_DIR/$PACKAGE_NAME.tar.gz"
echo ""
echo "To deploy on a target server:"
echo "   scp $OUTPUT_DIR/$PACKAGE_NAME.tar.gz user@server:/tmp/"
echo "   ssh user@server 'cd /tmp && tar -xzf $PACKAGE_NAME.tar.gz && cd $PACKAGE_NAME && sudo ./install_precompiled.sh'"
