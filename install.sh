#!/bin/bash

# Exit on error
set -e

# Check if running as root
if [ "$(id -u)" -ne 0 ]; then
    echo "❌ Please run as root (sudo ./install.sh)"
    exit 1
fi

echo "🐺 Initializing WolfServe Installation..."

# 1. Detect OS and install PHP + Dependencies
if [ -f /etc/os-release ]; then
    . /etc/os-release
    OS=$NAME
fi

echo "Detected OS: $OS"

if command -v dnf >/dev/null; then
    echo "Installing PHP and extensions via DNF (Fedora/RHEL)..."
    dnf install -y php php-fpm php-ffi php-pdo php-mysqlnd php-mbstring php-xml php-json \
        openssl-devel curl gcc make pkg-config
    # Enable and start PHP-FPM on Fedora/RHEL
    systemctl enable php-fpm 2>/dev/null || true
    systemctl start php-fpm 2>/dev/null || true
    # Handle SELinux if enabled
    if command -v getenforce >/dev/null && [ "$(getenforce)" != "Disabled" ]; then
        echo "Configuring SELinux for WolfServe..."
        setsebool -P httpd_can_network_connect 1 2>/dev/null || true
    fi
    # Fix PHP sessions directory permissions (Fedora/RHEL path)
    if [ -d "/var/lib/php/session" ]; then
        chmod 777 /var/lib/php/session
    fi
elif command -v apt-get >/dev/null; then
    echo "Installing PHP and extensions via APT (Debian/Ubuntu)..."
    apt-get update
    # Note: Removed version-conflicting php-sqlite3. Using generic php-fpm, php-mysql, php-xml.
    apt-get install -y php-fpm php-mysql php-xml curl libssl-dev pkg-config build-essential
    # Fix PHP sessions directory permissions (Debian/Ubuntu path)
    if [ -d "/var/lib/php/sessions" ]; then
        chmod 777 /var/lib/php/sessions
    fi
else
    echo "⚠️  Could not detect 'dnf' or 'apt-get'. Please install PHP and PHP-FPM manually."
fi

# 2. Check for Rust
if ! command -v cargo >/dev/null; then
    echo "❌ Rust/Cargo is not found."
    echo "Please install it by running:"
    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
    exit 1
else
    echo "✅ Rust is available."
fi

# 3. Build the Server
echo "🔨 Building WolfServe (Rust Server)..."
cargo build --release

# 4. Build the Library
echo "🔨 Building WolfLib (Rust PHP Extension)..."
if [ -f ./build_lib.sh ]; then
    chmod +x ./build_lib.sh
    sh ./build_lib.sh
else
    echo "⚠️  build_lib.sh not found, creating it..."
    echo '#!/bin/sh' > build_lib.sh
    echo 'cd wolflib && cargo build --release' >> build_lib.sh
    chmod +x build_lib.sh
    sh ./build_lib.sh
fi

# 5. Check if php-ffi is enabled
echo "🔍 Checking PHP FFI configuration..."
PHP_FFI_ENABLED=$(php -r "echo ini_get('ffi.enable');")
if [ "$PHP_FFI_ENABLED" != "1" ] && [ "$PHP_FFI_ENABLED" != "preload" ]; then
    echo "⚠️  PHP FFI is not enabled in your global php.ini."
    echo "   You may need to set 'ffi.enable=true' in your php.ini."
fi

echo "------------------------------------------------"
echo "🎉 Installation successfully completed!"
echo "   Run './run.sh' to start the server."
echo "------------------------------------------------"
