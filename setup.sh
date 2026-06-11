#!/bin/bash
#
# WolfServe installer — downloads the latest precompiled binary from
# GitHub releases, installs PHP-FPM with a dedicated pool, and sets up
# the systemd service. Mirrors the wolfproxy setup.sh pattern: no Rust
# toolchain needed on the operator's box.
#
# (C) 2026 Wolf Software Systems Ltd - http://wolf.uk.com
#
# Usage:
#   curl -sL https://raw.githubusercontent.com/wolfsoftwaresystemsltd/wolfserve/main/setup.sh | sudo bash
#
# Environment overrides (non-interactive):
#   WOLFSERVE_FPM_PORT     - PHP-FPM port (default: 9993)
#   WOLFSERVE_SESSION_PATH - PHP session save path (default: /var/lib/php/wolfserve-sessions)
#

set -euo pipefail

RED='\033[31m'
GREEN='\033[32m'
YELLOW='\033[33m'
BLUE='\033[34m'
CYAN='\033[36m'
RESET='\033[0m'
BOLD='\033[1m'

REPO="wolfsoftwaresystemsltd/wolfserve"
INSTALL_DIR="/opt/wolfserve"
BIN_PATH="/usr/local/bin/wolfserve"
PHP_FPM_PORT="${WOLFSERVE_FPM_PORT:-9993}"
SESSION_SAVE_PATH="${WOLFSERVE_SESSION_PATH:-/var/lib/php/wolfserve-sessions}"

banner() {
    echo ""
    echo -e "${CYAN} __          ______  _      ______  _____  ______  _____ __      __ ______ ${RESET}"
    echo -e "${CYAN} \\ \\        / / __ \\| |    |  ____|/ ____||  ____||  __ \\\\ \\    / /|  ____|${RESET}"
    echo -e "${CYAN}  \\ \\  /\\  / / |  | | |    | |__  | (___  | |__   | |__) |\\ \\  / / | |__   ${RESET}"
    echo -e "${CYAN}   \\ \\/  \\/ /| |  | | |    |  __|  \\___ \\ |  __|  |  _  /  \\ \\/ /  |  __|  ${RESET}"
    echo -e "${CYAN}    \\  /\\  / | |__| | |____| |     ____) || |____ | | \\ \\   \\  /   | |____ ${RESET}"
    echo -e "${CYAN}     \\/  \\/   \\____/|______|_|    |_____/ |______||_|  \\_\\   \\/    |______|${RESET}"
    echo ""
    echo -e "${BOLD} (C) 2026 Wolf Software Systems Ltd - http://wolf.uk.com${RESET}"
    echo ""
}

info()    { echo -e "${BLUE}[INFO]${RESET} $1"; }
success() { echo -e "${GREEN}[OK]${RESET} $1"; }
warn()    { echo -e "${YELLOW}[WARN]${RESET} $1"; }
error()   { echo -e "${RED}[ERROR]${RESET} $1"; exit 1; }

command_exists() { command -v "$1" &>/dev/null; }

# ─── Pre-flight ─────────────────────────────────────────────────────────

banner

if [ "$(id -u)" -ne 0 ]; then
    error "This script must be run as root. Use: curl ... | sudo bash"
fi

ARCH_RAW="$(uname -m)"
case "$ARCH_RAW" in
    x86_64|amd64)  ARCH="x86_64"  ;;
    aarch64|arm64) ARCH="aarch64" ;;
    *) error "Unsupported architecture '$ARCH_RAW' — precompiled binaries ship for x86_64 and aarch64 only. Build from source: https://github.com/${REPO}" ;;
esac
info "Detected architecture: $ARCH"

if ! command_exists curl; then
    error "curl is required. Install it with your package manager and re-run."
fi

# ─── PHP-FPM (wolfserve executes PHP through FastCGI) ───────────────────

if ! command_exists php-fpm && ! ls /usr/sbin/php-fpm* /usr/sbin/php*-fpm &>/dev/null; then
    info "Installing PHP-FPM…"
    if command_exists apt-get; then
        apt-get update -qq && apt-get install -y -qq php-fpm php-mysql php-xml
    elif command_exists dnf; then
        dnf install -y -q php-fpm php-mysqlnd php-xml
    elif command_exists pacman; then
        pacman -S --noconfirm --needed php-fpm
    elif command_exists zypper; then
        zypper --non-interactive install php-fpm php-mysql
    elif command_exists apk; then
        apk add --no-cache php83-fpm php83-mysqli php83-xml 2>/dev/null || apk add --no-cache php82-fpm php82-mysqli php82-xml
    else
        warn "Unknown package manager — install PHP-FPM yourself and point it at 127.0.0.1:${PHP_FPM_PORT}."
    fi
else
    info "PHP-FPM already present."
fi

# ─── Web user ───────────────────────────────────────────────────────────

WEB_USER="www-data"
if id "apache" &>/dev/null; then WEB_USER="apache";
elif id "nginx" &>/dev/null && ! id "www-data" &>/dev/null; then WEB_USER="nginx";
elif id "http" &>/dev/null && ! id "www-data" &>/dev/null; then WEB_USER="http"; fi
if ! id "$WEB_USER" &>/dev/null; then
    useradd --system --no-create-home --shell /usr/sbin/nologin wolfserve
    WEB_USER="wolfserve"
fi
info "Service user: $WEB_USER"

# ─── Stop any running instance before swapping the binary ──────────────

if systemctl is-active --quiet wolfserve 2>/dev/null; then
    info "Stopping running WolfServe instance…"
    systemctl stop wolfserve
fi

# ─── Download latest release ───────────────────────────────────────────

info "Resolving latest release from GitHub…"
ASSET_URL="https://github.com/${REPO}/releases/latest/download/wolfserve-${ARCH}"
SUMS_URL="https://github.com/${REPO}/releases/latest/download/SHA256SUMS"

TMPDIR="$(mktemp -d)"
trap 'rm -rf "$TMPDIR"' EXIT

info "Downloading wolfserve-${ARCH}…"
if ! curl -fsSL -o "$TMPDIR/wolfserve-${ARCH}" "$ASSET_URL"; then
    error "Download failed: $ASSET_URL — check network access and that the release exists."
fi

info "Verifying SHA-256 checksum…"
if curl -fsSL -o "$TMPDIR/SHA256SUMS" "$SUMS_URL"; then
    cd "$TMPDIR"
    if grep -F "  wolfserve-${ARCH}" SHA256SUMS | sha256sum -c - >/dev/null 2>&1; then
        success "Checksum verified."
    else
        error "Checksum mismatch — refusing to install."
    fi
    cd - >/dev/null
else
    warn "No SHA256SUMS for this release — skipping checksum verification."
fi

install -m 755 "$TMPDIR/wolfserve-${ARCH}" "$BIN_PATH"
mkdir -p "$INSTALL_DIR/public"
success "Installed $BIN_PATH"

# ─── Config ─────────────────────────────────────────────────────────────

if [ ! -f "$INSTALL_DIR/wolfserve.toml" ]; then
    cat > "$INSTALL_DIR/wolfserve.toml" <<EOF
[server]
host = "0.0.0.0"
port = 3000

[php]
fpm_address = "127.0.0.1:${PHP_FPM_PORT}"

[apache]
config_dir = "/etc/apache2"
EOF
    success "Default configuration written to $INSTALL_DIR/wolfserve.toml"
fi

# ─── PHP-FPM pool on the port wolfserve expects ─────────────────────────

FPM_POOL_CONF=""
FPM_SERVICE="php-fpm"
if [ -d "/etc/php" ]; then
    PHP_VER=$(ls /etc/php/ | sort -V | tail -n1)
    if [ -d "/etc/php/$PHP_VER/fpm/pool.d" ]; then
        FPM_POOL_CONF="/etc/php/$PHP_VER/fpm/pool.d/wolfserve.conf"
        FPM_SERVICE="php${PHP_VER}-fpm"
    fi
elif [ -d "/etc/php-fpm.d" ]; then
    FPM_POOL_CONF="/etc/php-fpm.d/wolfserve.conf"
fi

if [ -n "$FPM_POOL_CONF" ]; then
    mkdir -p "$SESSION_SAVE_PATH"
    chown "$WEB_USER:$WEB_USER" "$SESSION_SAVE_PATH"
    chmod 1733 "$SESSION_SAVE_PATH"
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
php_admin_value[session.save_path] = $SESSION_SAVE_PATH
EOF
    systemctl restart "$FPM_SERVICE" 2>/dev/null || systemctl restart php-fpm 2>/dev/null || warn "Could not restart PHP-FPM — restart it manually."
    success "PHP-FPM pool 'wolfserve' on 127.0.0.1:$PHP_FPM_PORT"
else
    warn "Could not locate a PHP-FPM pool directory — configure PHP-FPM to listen on 127.0.0.1:$PHP_FPM_PORT manually."
fi

# ─── systemd unit ───────────────────────────────────────────────────────

chown -R "$WEB_USER:$WEB_USER" "$INSTALL_DIR"
cat > /etc/systemd/system/wolfserve.service <<EOF
[Unit]
Description=WolfServe High Performance Rust PHP Server
After=network.target

[Service]
Type=simple
User=$WEB_USER
Group=$WEB_USER
WorkingDirectory=$INSTALL_DIR
ExecStart=$BIN_PATH
Restart=always
RestartSec=5
Environment=RUST_LOG=info
AmbientCapabilities=CAP_NET_BIND_SERVICE
LimitNOFILE=65536

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable wolfserve 2>/dev/null || true
systemctl restart wolfserve

sleep 2
if ! systemctl is-active --quiet wolfserve; then
    error "WolfServe failed to start. Inspect: journalctl -u wolfserve -n 50"
fi

VERSION_LINE="$("$BIN_PATH" --version 2>/dev/null || echo "wolfserve installed")"

echo ""
echo -e "${GREEN}${BOLD}═══════════════════════════════════════════════════════════════${RESET}"
echo -e "${GREEN}${BOLD}  ${VERSION_LINE} — installed and running.${RESET}"
echo -e "${GREEN}${BOLD}═══════════════════════════════════════════════════════════════${RESET}"
echo ""
echo -e "  Binary:        ${CYAN}${BIN_PATH}${RESET}"
echo -e "  Config:        ${CYAN}${INSTALL_DIR}/wolfserve.toml${RESET}"
echo -e "  Vhosts:        ${CYAN}Apache-style configs from /etc/apache2 (see config)${RESET}"
echo -e "  Service:       ${CYAN}systemctl status wolfserve${RESET}"
echo -e "  Logs:          ${CYAN}journalctl -u wolfserve -f${RESET}"
echo -e "  Default port:  ${CYAN}http://<this-host>:3000/${RESET}"
echo ""
