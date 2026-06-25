#!/bin/bash
# WolfServe Installation Script
# (C) 2026 Wolf Software Systems Ltd - http://wolf.uk.com
#
# WolfServe is distributed as a PREBUILT BINARY built by GitHub CI — there is no
# need to compile it from source. This script used to run `cargo build
# --release`, which failed for anyone running it outside a source checkout
# ("could not find Cargo.toml"). It is kept only so older links and docs keep
# working: it now hands off to setup.sh, which downloads the right prebuilt
# binary for this machine's architecture, installs PHP/dependencies, and sets up
# the service — exactly like WolfStack's installer and the other Wolf components.

set -e

if [ "$(id -u)" -ne 0 ]; then
    echo "❌ Please run as root (e.g. sudo bash)"
    exit 1
fi

if ! command -v curl >/dev/null 2>&1; then
    echo "❌ curl is required. Install it (e.g. 'apt install curl', 'dnf install curl', 'pacman -S curl') and re-run." >&2
    exit 1
fi

echo "🐺 WolfServe now ships as a prebuilt binary — fetching the installer (setup.sh)..."
curl -fsSL https://raw.githubusercontent.com/wolfsoftwaresystemsltd/wolfserve/main/setup.sh | bash
