#!/bin/bash

# Add common sbin folders to PATH for PHP-FPM
export PATH="$PATH:/usr/sbin:/usr/local/sbin"

# Function to checking required commands
check_command() {
    if ! command -v "$1" > /dev/null 2>&1; then
        # Try sbin for PHP-FPM specifically
        if [ "$1" = "php-fpm" ]; then
            if [ -f "/usr/sbin/php-fpm" ] || [ -f "/usr/sbin/php-fpm8.3" ] || [ -f "/usr/sbin/php-fpm8.2" ] || [ -f "/usr/sbin/php-fpm8.1" ]; then
                return 0
            fi
        fi
        echo "Error: $1 is required but not installed."
        exit 1
    fi
}

check_command php-fpm

# Cleanup function to kill background processes on script exit
cleanup() {
    echo "Stopping servers..."
    # Only kill background jobs if we are still running
    # kill $(jobs -p) 2>/dev/null
}

# Set trap to call cleanup on Interrupt or Terminate
trap cleanup INT TERM

echo "Starting WolfServe Environment..."

# Read session_save_path from wolfserve.toml config (defaults to /tmp if not found)
SESSION_SAVE_PATH="/tmp"
if [ -f "wolfserve.toml" ]; then
    CONFIGURED_PATH=$(grep -E "^session_save_path\s*=" wolfserve.toml | sed 's/.*=\s*"\(.*\)"/\1/' | tr -d ' ')
    if [ -n "$CONFIGURED_PATH" ]; then
        SESSION_SAVE_PATH="$CONFIGURED_PATH"
        # Create session directory if it doesn't exist
        if [ ! -d "$SESSION_SAVE_PATH" ]; then
            echo "Creating session directory: $SESSION_SAVE_PATH"
            mkdir -p "$SESSION_SAVE_PATH"
            chmod 1733 "$SESSION_SAVE_PATH"
        fi
    fi
fi
echo "PHP session save path: $SESSION_SAVE_PATH"

# 1. Start PHP-FPM
# Check if port 9993 is already in use
if command -v lsof >/dev/null 2>&1 && lsof -i :9993 >/dev/null 2>&1; then
    echo "Port 9993 is already in use. Assuming PHP-FPM is running."
else
    echo "Starting PHP-FPM on 127.0.0.1:9993..."
    
    # Create a minimal config to avoid conflicts with system-wide FPM
    FPM_CONF="/tmp/wolfserve-fpm.conf"
    cat <<EOF > "$FPM_CONF"
[global]
error_log = /tmp/wolfserve-fpm.log
daemonize = yes

[www]
user = $(whoami)
group = $(id -gn)
listen = 127.0.0.1:9993
pm = static
pm.max_children = 5
php_admin_value[ffi.enable] = true
php_admin_value[file_uploads] = On
php_admin_value[upload_max_filesize] = 256M
php_admin_value[post_max_size] = 256M
php_admin_value[upload_tmp_dir] = /tmp
php_admin_value[max_file_uploads] = 20
php_admin_value[session.save_path] = ${SESSION_SAVE_PATH:-/tmp}
EOF

    # Find the fpm binary
    FPM_BIN=""
    if command -v php-fpm >/dev/null; then FPM_BIN="php-fpm";
    elif command -v php-fpm8.3 >/dev/null; then FPM_BIN="php-fpm8.3";
    elif command -v php-fpm8.2 >/dev/null; then FPM_BIN="php-fpm8.2";
    elif command -v php-fpm8.1 >/dev/null; then FPM_BIN="php-fpm8.1";
    fi

    if [ -n "$FPM_BIN" ]; then
        # Run with the custom config and ignore system php.ini to avoid conflicts
        if [ "$(id -u)" = "0" ]; then
            "$FPM_BIN" -n -y "$FPM_CONF" --allow-to-run-as-root
        else
            "$FPM_BIN" -n -y "$FPM_CONF"
        fi
    else
        echo "Warning: php-fpm not found. You may need to start it manually."
    fi
    # Wait for PHP to initialize
    sleep 1
fi

# 2. Start Rust Server
echo "Starting WolfServe (binary) in screen session 'wolfserve'..."

BINARY="./target/release/wolfserve"

if [ -f "$BINARY" ]; then
    # Check if a screen session named 'wolfserve' already exists
    if screen -list | grep -q "\.wolfserve"; then
        echo "A screen session named 'wolfserve' is already running. Re-attaching..."
        screen -dr wolfserve
    else
        # Run and capture potential immediate crash to a log
        screen -dmLS wolfserve bash -c "$BINARY 2>&1 | tee /tmp/wolfserve.crash.log"
        echo "WolfServe started in background screen session 'wolfserve'."
        echo "Use 'screen -r wolfserve' to view logs."
        echo "If it crashes immediately, check /tmp/wolfserve.crash.log"
    fi
else
    echo "Error: wolfserve binary not found at $BINARY"
    echo "Please compile the project first."
    exit 1
fi
