#!/bin/bash

# WolfServe Service Installer
# Installs WolfServe as a systemd service on Linux

set -e

INSTALL_DIR="/opt/wolfserve"
SERVICE_NAME="wolfserve"
PHP_FPM_PORT="9993"

# Check if running as root
if [ "$(id -u)" -ne 0 ]; then
  echo "❌ Please run as root (sudo ./install_service.sh)"
  exit 1
fi

echo "🐺 Installing WolfServe as a service..."

# Fix PHP sessions directory permissions
echo "🔧 Fixing PHP sessions directory permissions..."
for sessions_dir in /var/lib/php/sessions /var/lib/php/session; do
    if [ -d "$sessions_dir" ]; then
        chmod 777 "$sessions_dir"
        echo "   ✅ Set permissions on $sessions_dir"
        break
    fi
done

# 1. Determine Web User
WEB_USER="www-data"
if id "apache" &>/dev/null; then
    WEB_USER="apache"
elif id "nginx" &>/dev/null; then
    WEB_USER="nginx"
elif id "http" &>/dev/null; then
    WEB_USER="http"
fi
if ! id "$WEB_USER" &>/dev/null; then
    echo "⚠️  Web user '$WEB_USER' not found. Creating system user 'wolfserve'..."
    useradd -r -s /bin/false wolfserve
    WEB_USER="wolfserve"
fi
echo "👤 Service will run as user: $WEB_USER"

# 2. Build Checks
if [ ! -f "target/release/wolfserve" ]; then
    echo "🔨 wolfserve binary not found. Building..."
    if command -v cargo &>/dev/null; then
        cargo build --release
    else 
        echo "❌ cargo not found. Please run ./install.sh first or install Rust."
        exit 1
    fi
fi

if [ ! -f "wolflib/target/release/libwolflib.so" ]; then
    echo "🔨 libwolflib.so not found. Building..."
    if [ -f "./build_lib.sh" ]; then
        sh ./build_lib.sh
    else
        cd wolflib && cargo build --release && cd ..
    fi
fi

# 3. Create Installation Directory
echo "📂 Creating install directory at $INSTALL_DIR..."
mkdir -p "$INSTALL_DIR"
mkdir -p "$INSTALL_DIR/public"

# 4. Copy Files
echo "Vk Copying files..."
cp target/release/wolfserve "$INSTALL_DIR/"
cp wolflib/target/release/libwolflib.so "$INSTALL_DIR/"

# Copy config if not exists, else keep existing
if [ ! -f "$INSTALL_DIR/wolfserve.toml" ]; then
    if [ -f "wolfserve.toml" ]; then
        cp wolfserve.toml "$INSTALL_DIR/"
    elif [ -f "wolfserve.toml.example" ]; then
        cp wolfserve.toml.example "$INSTALL_DIR/wolfserve.toml"
    else
        # minimal config
        echo "[server]" > "$INSTALL_DIR/wolfserve.toml"
        echo "host = \"0.0.0.0\"" >> "$INSTALL_DIR/wolfserve.toml"
        echo "port = 3000" >> "$INSTALL_DIR/wolfserve.toml"
        echo "" >> "$INSTALL_DIR/wolfserve.toml"
        echo "[php]" >> "$INSTALL_DIR/wolfserve.toml"
        echo "fpm_address = \"127.0.0.1:$PHP_FPM_PORT\"" >> "$INSTALL_DIR/wolfserve.toml"
        echo "mode = \"fpm\"" >> "$INSTALL_DIR/wolfserve.toml"
    fi
fi

# Copy public files
# Only copy if source public exists
if [ -d "public" ]; then
    cp -r public/* "$INSTALL_DIR/public/"
fi

# 5. Fix paths in public/rust.php if it exists
# We want to replace the hardcoded source path with the new install path
RUST_PHP="$INSTALL_DIR/public/rust.php"
if [ -f "$RUST_PHP" ]; then
    echo "🔧 Updating libwolflib.so path in $RUST_PHP..."
    # Escape slashes for sed
    ESCAPED_LIB_PATH=$(echo "$INSTALL_DIR/libwolflib.so" | sed 's/\//\\\//g')
    # Use generic regex to find $libPath = '...'; assignment
    sed -i "s/\$libPath = '.*';/\$libPath = '$ESCAPED_LIB_PATH';/" "$RUST_PHP"
fi

# Set permissions
chown -R $WEB_USER:$WEB_USER "$INSTALL_DIR"
chmod +x "$INSTALL_DIR/wolfserve"

# 6. Create Systemd Service
echo "⚙️  Creating systemd service /etc/systemd/system/$SERVICE_NAME.service..."
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
Environment=RUST_LOG=info
AmbientCapabilities=CAP_NET_BIND_SERVICE

[Install]
WantedBy=multi-user.target
EOF

# 7. Configure PHP-FPM Pool
echo "🏊 Configuring PHP-FPM pool for WolfServe..."
FPM_POOL_CONF=""
# Attempt to find pool directory
if [ -d "/etc/php" ]; then
    # Debian/Ubuntu style: /etc/php/X.X/fpm/pool.d/
    # Find latest version
    PHP_VER=$(ls /etc/php/ | sort -V | tail -n1)
    if [ -d "/etc/php/$PHP_VER/fpm/pool.d" ]; then
        FPM_POOL_CONF="/etc/php/$PHP_VER/fpm/pool.d/wolfserve.conf"
    fi
elif [ -d "/etc/php-fpm.d" ]; then
    # RHEL/Fedora style
    FPM_POOL_CONF="/etc/php-fpm.d/wolfserve.conf"
fi

if [ -n "$FPM_POOL_CONF" ]; then
    echo "   Writing pool config to $FPM_POOL_CONF..."
    cat > "$FPM_POOL_CONF" <<EOF
[wolfserve]
user = $WEB_USER
group = $WEB_USER
listen = 127.0.0.1:$PHP_FPM_PORT
listen.owner = $WEB_USER
listen.group = $WEB_USER
pm = dynamic
pm.max_children = 5
pm.start_servers = 2
pm.min_spare_servers = 1
pm.max_spare_servers = 3
EOF
    echo "   ✅ PHP-FPM pool configured."
else
    echo "⚠️  Could not locate PHP-FPM pool directory. Please ensure PHP-FPM listens on 127.0.0.1:$PHP_FPM_PORT manually."
fi

# 8. Reload and Start
echo "🚀 Reloading daemons and starting services..."
systemctl daemon-reload

# Restart PHP-FPM to pick up new pool (handles both Fedora/RHEL and Debian/Ubuntu)
PHP_FPM_SVC=""
# Fedora/RHEL uses 'php-fpm', Debian/Ubuntu uses 'phpX.Y-fpm'
for svc in php-fpm php8.3-fpm php8.2-fpm php8.1-fpm php8.0-fpm php7.4-fpm; do
    if systemctl list-unit-files 2>/dev/null | grep -q "^$svc\.service"; then
        PHP_FPM_SVC="$svc"
        break
    fi
done
if [ -n "$PHP_FPM_SVC" ]; then
    echo "   Restarting $PHP_FPM_SVC..."
    systemctl enable "$PHP_FPM_SVC" 2>/dev/null || true
    systemctl restart "$PHP_FPM_SVC"
else
    echo "⚠️  Could not find PHP-FPM service. Please restart php-fpm manually."
fi

# Enable and start WolfServe
systemctl enable $SERVICE_NAME
systemctl restart $SERVICE_NAME

echo "✅ Installation complete!"
echo "   Server running on http://$(grep -m 1 'host =' $INSTALL_DIR/wolfserve.toml | cut -d '"' -f 2):$(grep -m 1 'port =' $INSTALL_DIR/wolfserve.toml | cut -d '=' -f 2 | tr -d ' ')"
echo "   Manage with: systemctl [start|stop|restart|status] $SERVICE_NAME"
echo ""
echo "⚠️  Don't forget to stop Apache and disable it if running on port 80/443:"
echo "   systemctl stop apache2 httpd"
echo "   systemctl disable apache2 httpd"
