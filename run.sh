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
check_command cargo

# Cleanup function to kill background processes on script exit
cleanup() {
    echo "Stopping servers..."
    kill $(jobs -p) 2>/dev/null
}

# Set trap to call cleanup on exit
trap cleanup EXIT INT TERM

echo "Starting WolfServe Environment..."

# Build WolfLib
echo "Building WolfLib..."
if ! (cd wolflib && cargo build --release); then
    echo "Error: Failed to build WolfLib"
    exit 1
fi

# 2. Start Rust Server
echo "Building and starting WolfServe..."
# Use cargo run --release for better performance, or just cargo run for dev
cargo run --bin wolfserve

# Script will stay here until cargo run exits
