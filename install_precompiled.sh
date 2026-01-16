#!/bin/bash

# WolfServe Precompiled Installer
# Installs WolfServe from precompiled binaries without requiring source code or Rust
# Usage: sudo ./install_precompiled.sh [binary_dir]
#   binary_dir: Optional path containing precompiled binaries (default: current directory)

set -e

INSTALL_DIR="/opt/wolfserve"
SERVICE_NAME="wolfserve"
PHP_FPM_PORT="9993"
BINARY_DIR="${1:-.}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo_info() { echo -e "${GREEN}✅ $1${NC}"; }
echo_warn() { echo -e "${YELLOW}⚠️  $1${NC}"; }
echo_error() { echo -e "${RED}❌ $1${NC}"; }

# Check if running as root
if [ "$(id -u)" -ne 0 ]; then
    echo_error "Please run as root (sudo ./install_precompiled.sh)"
    exit 1
fi

echo "🐺 WolfServe Precompiled Binary Installer"
echo "=========================================="

# 1. Check for required precompiled files
echo ""
echo "📦 Checking for precompiled binaries in: $BINARY_DIR"

WOLFSERVE_BIN=""
WOLFLIB_SO=""

# Look for wolfserve binary
if [ -f "$BINARY_DIR/wolfserve" ]; then
    WOLFSERVE_BIN="$BINARY_DIR/wolfserve"
elif [ -f "$BINARY_DIR/target/release/wolfserve" ]; then
    WOLFSERVE_BIN="$BINARY_DIR/target/release/wolfserve"
else
    echo_error "wolfserve binary not found!"
    echo "   Expected at: $BINARY_DIR/wolfserve or $BINARY_DIR/target/release/wolfserve"
    exit 1
fi
echo_info "Found wolfserve: $WOLFSERVE_BIN"

# Look for libwolflib.so
if [ -f "$BINARY_DIR/libwolflib.so" ]; then
    WOLFLIB_SO="$BINARY_DIR/libwolflib.so"
elif [ -f "$BINARY_DIR/wolflib/target/release/libwolflib.so" ]; then
    WOLFLIB_SO="$BINARY_DIR/wolflib/target/release/libwolflib.so"
else
    echo_warn "libwolflib.so not found - PHP FFI extension will not be available"
fi
[ -n "$WOLFLIB_SO" ] && echo_info "Found libwolflib.so: $WOLFLIB_SO"

# Look for config file
CONFIG_FILE=""
if [ -f "$BINARY_DIR/wolfserve.toml" ]; then
    CONFIG_FILE="$BINARY_DIR/wolfserve.toml"
elif [ -f "$BINARY_DIR/wolfserve.toml.example" ]; then
    CONFIG_FILE="$BINARY_DIR/wolfserve.toml.example"
fi
[ -n "$CONFIG_FILE" ] && echo_info "Found config: $CONFIG_FILE"

# 2. Detect OS and install PHP dependencies
echo ""
echo "📋 Checking system dependencies..."

if [ -f /etc/os-release ]; then
    . /etc/os-release
    OS=$NAME
    echo "   Detected OS: $OS"
fi

install_php() {
    if command -v dnf >/dev/null 2>&1; then
        echo "   Installing PHP and extensions via DNF (Fedora/RHEL)..."
        dnf install -y php php-fpm php-ffi php-pdo php-mysqlnd php-mbstring php-xml php-json 2>/dev/null || \
        dnf install -y php php-fpm php-pdo php-mysqlnd php-mbstring php-xml 2>/dev/null
        # Enable PHP-FPM service on Fedora/RHEL
        systemctl enable php-fpm 2>/dev/null || true
        systemctl start php-fpm 2>/dev/null || true
        # Handle SELinux if enabled
        if command -v getenforce >/dev/null && [ "$(getenforce)" != "Disabled" ]; then
            echo "   Configuring SELinux..."
            setsebool -P httpd_can_network_connect 1 2>/dev/null || true
        fi
    elif command -v apt-get >/dev/null 2>&1; then
        echo "   Installing PHP and extensions via APT (Debian/Ubuntu)..."
        apt-get update -qq
        apt-get install -y php-fpm php-mysql php-xml curl >/dev/null 2>&1
    elif command -v pacman >/dev/null 2>&1; then
        echo "   Installing PHP and extensions via Pacman (Arch)..."
        pacman -S --noconfirm php php-fpm >/dev/null 2>&1
    elif command -v zypper >/dev/null 2>&1; then
        echo "   Installing PHP and extensions via Zypper (openSUSE)..."
        zypper install -y php php-fpm php-mysql php-xml >/dev/null 2>&1
    else
        echo_warn "Could not detect package manager. Please install PHP and PHP-FPM manually."
    fi
}

# Check if PHP-FPM is installed
if ! command -v php-fpm >/dev/null 2>&1 && ! [ -f /usr/sbin/php-fpm ]; then
    echo "   PHP-FPM not found, installing..."
    install_php
else
    echo_info "PHP-FPM is already installed"
fi

# Fix PHP sessions directory permissions
echo "   Fixing PHP sessions directory permissions..."
for sessions_dir in /var/lib/php/sessions /var/lib/php/session; do
    if [ -d "$sessions_dir" ]; then
        chmod 777 "$sessions_dir"
        echo_info "Set permissions on $sessions_dir"
        break
    fi
done

# 3. Determine Web User
echo ""
echo "👤 Determining service user..."
WEB_USER="www-data"
if id "apache" &>/dev/null; then
    WEB_USER="apache"
elif id "nginx" &>/dev/null; then
    WEB_USER="nginx"
elif id "http" &>/dev/null; then
    WEB_USER="http"
fi
if ! id "$WEB_USER" &>/dev/null; then
    echo "   Creating system user 'wolfserve'..."
    useradd -r -s /bin/false wolfserve 2>/dev/null || true
    WEB_USER="wolfserve"
fi
echo_info "Service will run as user: $WEB_USER"

# 4. Create Installation Directory
echo ""
echo "📂 Setting up installation directory..."
mkdir -p "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR/public"

# 5. Copy Binaries
echo ""
echo "📋 Installing binaries..."
cp "$WOLFSERVE_BIN" "$INSTALL_DIR/wolfserve"
chmod +x "$INSTALL_DIR/wolfserve"
echo_info "Installed wolfserve to $INSTALL_DIR/wolfserve"

if [ -n "$WOLFLIB_SO" ]; then
    cp "$WOLFLIB_SO" "$INSTALL_DIR/libwolflib.so"
    chmod 644 "$INSTALL_DIR/libwolflib.so"
    echo_info "Installed libwolflib.so to $INSTALL_DIR/libwolflib.so"
fi

# 6. Install Configuration
echo ""
echo "⚙️  Setting up configuration..."
if [ ! -f "$INSTALL_DIR/wolfserve.toml" ]; then
    if [ -n "$CONFIG_FILE" ]; then
        cp "$CONFIG_FILE" "$INSTALL_DIR/wolfserve.toml"
        echo_info "Copied configuration from $CONFIG_FILE"
    else
        # Create minimal default config
        cat > "$INSTALL_DIR/wolfserve.toml" <<TOML
# WolfServe Configuration

[server]
host = "0.0.0.0"
port = 3000

[php]
fpm_address = "127.0.0.1:$PHP_FPM_PORT"

[apache]
config_dir = "/etc/apache2"
TOML
        echo_info "Created default configuration"
    fi
else
    echo_info "Keeping existing configuration at $INSTALL_DIR/wolfserve.toml"
fi

# 7. Copy public files if available
if [ -d "$BINARY_DIR/public" ]; then
    echo ""
    echo "📄 Copying public web files..."
    cp -r "$BINARY_DIR/public/"* "$INSTALL_DIR/public/" 2>/dev/null || true
    
    # Fix libwolflib.so path in rust.php if it exists
    RUST_PHP="$INSTALL_DIR/public/rust.php"
    if [ -f "$RUST_PHP" ]; then
        ESCAPED_LIB_PATH=$(echo "$INSTALL_DIR/libwolflib.so" | sed 's/\//\\\//g')
        sed -i "s/\$libPath = '.*';/\$libPath = '$ESCAPED_LIB_PATH';/" "$RUST_PHP"
        echo_info "Updated libwolflib.so path in rust.php"
    fi
fi

# 8. Set Permissions
echo ""
echo "🔒 Setting permissions..."
chown -R "$WEB_USER:$WEB_USER" "$INSTALL_DIR"
echo_info "Set ownership to $WEB_USER"

# 9. Create Systemd Service
echo ""
echo "🔧 Creating systemd service..."
cat > /etc/systemd/system/$SERVICE_NAME.service <<EOF
[Unit]
Description=WolfServe High Performance Rust PHP Server
After=network.target php-fpm.service

[Service]
Type=simple
User=$WEB_USER
Group=$WEB_USER
WorkingDirectory=$INSTALL_DIR
ExecStart=$INSTALL_DIR/wolfserve
Restart=always
RestartSec=5
Environment=RUST_LOG=info
AmbientCapabilities=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
EOF
echo_info "Created /etc/systemd/system/$SERVICE_NAME.service"

# 10. Configure PHP-FPM Pool
echo ""
echo "🏊 Configuring PHP-FPM pool..."
FPM_POOL_CONF=""
if [ -d "/etc/php" ]; then
    # Debian/Ubuntu style
    PHP_VER=$(ls /etc/php/ 2>/dev/null | sort -V | tail -n1)
    if [ -n "$PHP_VER" ] && [ -d "/etc/php/$PHP_VER/fpm/pool.d" ]; then
        FPM_POOL_CONF="/etc/php/$PHP_VER/fpm/pool.d/wolfserve.conf"
    fi
elif [ -d "/etc/php-fpm.d" ]; then
    # RHEL/Fedora style
    FPM_POOL_CONF="/etc/php-fpm.d/wolfserve.conf"
fi

if [ -n "$FPM_POOL_CONF" ]; then
    cat > "$FPM_POOL_CONF" <<EOF
[wolfserve]
user = $WEB_USER
group = $WEB_USER
listen = 127.0.0.1:$PHP_FPM_PORT
listen.owner = $WEB_USER
listen.group = $WEB_USER
pm = dynamic
pm.max_children = 10
pm.start_servers = 2
pm.min_spare_servers = 1
pm.max_spare_servers = 5
php_admin_value[error_log] = /var/log/wolfserve-php-fpm.log
php_admin_flag[log_errors] = on
EOF
    echo_info "PHP-FPM pool configured at $FPM_POOL_CONF"
else
    echo_warn "Could not locate PHP-FPM pool directory."
    echo "   Please ensure PHP-FPM listens on 127.0.0.1:$PHP_FPM_PORT manually."
fi

# 11. Enable and Start Services
echo ""
echo "🚀 Starting services..."
systemctl daemon-reload

# Find and restart PHP-FPM (handles both Fedora/RHEL and Debian/Ubuntu naming)
PHP_FPM_SVC=""
# Fedora/RHEL uses 'php-fpm', Debian/Ubuntu uses 'phpX.Y-fpm'
for svc in php-fpm php8.3-fpm php8.2-fpm php8.1-fpm php8.0-fpm php7.4-fpm; do
    if systemctl list-unit-files 2>/dev/null | grep -q "^$svc\.service"; then
        PHP_FPM_SVC="$svc"
        break
    fi
done
# Fallback: check if php-fpm is available as a service
if [ -z "$PHP_FPM_SVC" ] && systemctl cat php-fpm >/dev/null 2>&1; then
    PHP_FPM_SVC="php-fpm"
fi

if [ -n "$PHP_FPM_SVC" ]; then
    echo "   Restarting $PHP_FPM_SVC..."
    systemctl enable "$PHP_FPM_SVC" >/dev/null 2>&1 || true
    systemctl restart "$PHP_FPM_SVC"
    echo_info "PHP-FPM restarted"
else
    echo_warn "Could not find PHP-FPM service. Please restart it manually."
fi

# Enable and start WolfServe
systemctl enable "$SERVICE_NAME" >/dev/null 2>&1
systemctl restart "$SERVICE_NAME"
echo_info "WolfServe service started"

# 12. Final Summary
echo ""
echo "=========================================="
echo "🎉 WolfServe Installation Complete!"
echo "=========================================="
echo ""
echo "📁 Installation directory: $INSTALL_DIR"
echo "🌐 Server running on: http://0.0.0.0:$(grep -m 1 'port' $INSTALL_DIR/wolfserve.toml 2>/dev/null | grep -o '[0-9]*' || echo '3000')"
echo ""
echo "📋 Useful commands:"
echo "   systemctl status $SERVICE_NAME    - Check service status"
echo "   systemctl restart $SERVICE_NAME   - Restart the service"
echo "   systemctl stop $SERVICE_NAME      - Stop the service"
echo "   journalctl -u $SERVICE_NAME -f    - View live logs"
echo ""
echo "📝 Configuration file: $INSTALL_DIR/wolfserve.toml"
echo ""

# Quick status check
if systemctl is-active --quiet "$SERVICE_NAME"; then
    echo_info "WolfServe is running!"
else
    echo_warn "WolfServe may not have started correctly. Check logs with:"
    echo "   journalctl -u $SERVICE_NAME -n 50"
fi
