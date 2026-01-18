#!/bin/bash
#
# WolfServe Upgrade Script
# (C)2025 Wolf Software Systems Ltd
#
# Usage: ./upgrade.sh [new_binary_path]
#        ./upgrade.sh --rollback
#        ./upgrade.sh --build      (build and upgrade)
#        ./upgrade.sh              (auto-find or build binary)
#

set -e

# Configuration
INSTALL_DIR="${WOLFSERVE_DIR:-/opt/wolfserve}"
SERVICE_NAME="wolfserve"
BINARY_NAME="wolfserve"
BACKUP_DIR="${INSTALL_DIR}/backups"
MAX_BACKUPS=5
HEALTH_CHECK_URL="${HEALTH_CHECK_URL:-http://127.0.0.1:3000/}"
HEALTH_CHECK_TIMEOUT=30

# Try to find the source directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SOURCE_DIR="${WOLFSERVE_SOURCE:-${SCRIPT_DIR}}"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

log_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root (use sudo)"
        exit 1
    fi
}

get_current_version() {
    if [[ -x "${INSTALL_DIR}/${BINARY_NAME}" ]]; then
        "${INSTALL_DIR}/${BINARY_NAME}" --version 2>/dev/null || echo "unknown"
    else
        echo "not installed"
    fi
}

create_backup() {
    local timestamp=$(date +%Y%m%d_%H%M%S)
    local backup_path="${BACKUP_DIR}/${BINARY_NAME}_${timestamp}"
    
    mkdir -p "${BACKUP_DIR}"
    
    if [[ -f "${INSTALL_DIR}/${BINARY_NAME}" ]]; then
        cp "${INSTALL_DIR}/${BINARY_NAME}" "${backup_path}"
        log_info "Created backup: ${backup_path}"
        
        # Clean up old backups, keep only MAX_BACKUPS
        local backup_count=$(ls -1 "${BACKUP_DIR}/${BINARY_NAME}_"* 2>/dev/null | wc -l)
        if [[ $backup_count -gt $MAX_BACKUPS ]]; then
            ls -1t "${BACKUP_DIR}/${BINARY_NAME}_"* | tail -n +$((MAX_BACKUPS + 1)) | xargs rm -f
            log_info "Cleaned up old backups (keeping ${MAX_BACKUPS})"
        fi
        
        echo "${backup_path}"
    else
        echo ""
    fi
}

get_latest_backup() {
    ls -1t "${BACKUP_DIR}/${BINARY_NAME}_"* 2>/dev/null | head -1
}

# Find binary in common locations
find_binary() {
    local locations=(
        "${SOURCE_DIR}/target/release/${BINARY_NAME}"
        "${SOURCE_DIR}/target/debug/${BINARY_NAME}"
        "${SOURCE_DIR}/${BINARY_NAME}"
        "./target/release/${BINARY_NAME}"
        "./target/debug/${BINARY_NAME}"
        "./${BINARY_NAME}"
    )
    
    for loc in "${locations[@]}"; do
        if [[ -f "${loc}" && -x "${loc}" ]]; then
            echo "${loc}"
            return 0
        fi
    done
    
    return 1
}

# Build the binary from source
build_binary() {
    log_info "Building ${BINARY_NAME} from source..." >&2
    
    if [[ ! -f "${SOURCE_DIR}/Cargo.toml" ]]; then
        log_error "Cannot find Cargo.toml in ${SOURCE_DIR}" >&2
        log_error "Set WOLFSERVE_SOURCE to the source directory or run from source dir" >&2
        return 1
    fi
    
    cd "${SOURCE_DIR}"
    
    # Check if cargo is available
    if ! command -v cargo &>/dev/null; then
        log_error "Cargo not found. Please install Rust: https://rustup.rs" >&2
        return 1
    fi
    
    log_info "Compiling release build (this may take a few minutes)..." >&2
    if cargo build --release 2>&1; then
        log_info "Build completed successfully" >&2
        echo "${SOURCE_DIR}/target/release/${BINARY_NAME}"
        return 0
    else
        log_error "Build failed" >&2
        return 1
    fi
}

# Find or build the binary
find_or_build_binary() {
    local binary_path
    
    # First try to find an existing binary
    if binary_path=$(find_binary); then
        log_info "Found existing binary: ${binary_path}" >&2
        echo "${binary_path}"
        return 0
    fi
    
    # No binary found, try to build
    log_warn "No binary found, attempting to build..." >&2
    if binary_path=$(build_binary); then
        echo "${binary_path}"
        return 0
    fi
    
    return 1
}

stop_service() {
    log_info "Stopping ${SERVICE_NAME} service..."
    
    if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
        systemctl stop "${SERVICE_NAME}"
        
        # Wait for graceful shutdown
        local count=0
        while systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null && [[ $count -lt 30 ]]; do
            sleep 1
            ((count++))
        done
        
        if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
            log_warn "Service didn't stop gracefully, forcing..."
            systemctl kill "${SERVICE_NAME}"
        fi
        
        log_info "Service stopped"
    else
        log_info "Service was not running"
    fi
}

start_service() {
    log_info "Starting ${SERVICE_NAME} service..."
    systemctl start "${SERVICE_NAME}"
    
    # Wait a moment for startup
    sleep 2
    
    if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
        log_info "Service started successfully"
        return 0
    else
        log_error "Service failed to start"
        return 1
    fi
}

health_check() {
    log_info "Performing health check..."
    
    local count=0
    while [[ $count -lt $HEALTH_CHECK_TIMEOUT ]]; do
        if curl -sf -o /dev/null --max-time 5 "${HEALTH_CHECK_URL}" 2>/dev/null; then
            log_info "Health check passed"
            return 0
        fi
        sleep 1
        ((count++))
    done
    
    log_error "Health check failed after ${HEALTH_CHECK_TIMEOUT} seconds"
    return 1
}

install_binary() {
    local new_binary="$1"
    
    if [[ ! -f "${new_binary}" ]]; then
        log_error "New binary not found: ${new_binary}"
        exit 1
    fi
    
    if [[ ! -x "${new_binary}" ]]; then
        chmod +x "${new_binary}"
    fi
    
    # Remove existing binary before installing new one
    if [[ -f "${INSTALL_DIR}/${BINARY_NAME}" ]]; then
        rm -f "${INSTALL_DIR}/${BINARY_NAME}"
    fi
    
    cp "${new_binary}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    
    log_info "Installed new binary to ${INSTALL_DIR}/${BINARY_NAME}"
}

rollback() {
    local backup_path=$(get_latest_backup)
    
    if [[ -z "${backup_path}" || ! -f "${backup_path}" ]]; then
        log_error "No backup found to rollback to"
        exit 1
    fi
    
    log_warn "Rolling back to: ${backup_path}"
    
    stop_service
    
    cp "${backup_path}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"
    
    if start_service; then
        if health_check; then
            log_info "Rollback completed successfully"
            # Remove the used backup
            rm -f "${backup_path}"
        else
            log_error "Rollback completed but health check failed!"
            exit 1
        fi
    else
        log_error "Rollback failed - service won't start"
        exit 1
    fi
}

upgrade() {
    local new_binary="$1"
    
    log_info "=== WolfServe Upgrade ==="
    log_info "Current version: $(get_current_version)"
    log_info "New binary: ${new_binary}"
    
    # Create backup
    local backup_path=$(create_backup)
    
    # Stop service
    stop_service
    
    # Install new binary
    install_binary "${new_binary}"
    
    # Start service
    if ! start_service; then
        log_error "New version failed to start, rolling back..."
        if [[ -n "${backup_path}" ]]; then
            cp "${backup_path}" "${INSTALL_DIR}/${BINARY_NAME}"
            start_service || true
        fi
        exit 1
    fi
    
    # Health check
    if ! health_check; then
        log_error "Health check failed, rolling back..."
        stop_service
        if [[ -n "${backup_path}" ]]; then
            cp "${backup_path}" "${INSTALL_DIR}/${BINARY_NAME}"
            start_service || true
        fi
        exit 1
    fi
    
    log_info "=== Upgrade completed successfully ==="
    log_info "New version: $(get_current_version)"
}

show_status() {
    echo "=== WolfServe Status ==="
    echo "Install directory: ${INSTALL_DIR}"
    echo "Current version: $(get_current_version)"
    echo ""
    
    if systemctl is-active --quiet "${SERVICE_NAME}" 2>/dev/null; then
        echo -e "Service status: ${GREEN}running${NC}"
    else
        echo -e "Service status: ${RED}stopped${NC}"
    fi
    
    echo ""
    echo "Available backups:"
    if ls "${BACKUP_DIR}/${BINARY_NAME}_"* &>/dev/null; then
        ls -lh "${BACKUP_DIR}/${BINARY_NAME}_"* | awk '{print "  " $9 " (" $5 ")"}'
    else
        echo "  (none)"
    fi
}

show_usage() {
    echo "WolfServe Upgrade Script"
    echo ""
    echo "Usage:"
    echo "  $0                  Auto-find binary or build from source, then upgrade"
    echo "  $0 <binary_path>    Upgrade using specified binary"
    echo "  $0 --build          Force rebuild from source, then upgrade"
    echo "  $0 --rollback       Rollback to previous version"
    echo "  $0 --status         Show current status"
    echo "  $0 --help           Show this help"
    echo ""
    echo "Environment variables:"
    echo "  WOLFSERVE_DIR       Installation directory (default: /opt/wolfserve)"
    echo "  WOLFSERVE_SOURCE    Source directory for building (default: script location)"
    echo "  HEALTH_CHECK_URL    Health check URL (default: http://127.0.0.1:3000/)"
    echo ""
    echo "Examples:"
    echo "  sudo $0                               # Auto-find or build"
    echo "  sudo $0 --build                       # Force rebuild"
    echo "  sudo $0 ./target/release/wolfserve    # Use specific binary"
    echo "  sudo $0 --rollback                    # Rollback to previous"
}

# Main
case "${1:-}" in
    --rollback)
        check_root
        rollback
        ;;
    --status)
        show_status
        ;;
    --help|-h)
        show_usage
        ;;
    --build)
        check_root
        binary_path=$(build_binary) || exit 1
        upgrade "${binary_path}"
        ;;
    "")
        # No argument - auto-find or build
        check_root
        binary_path=$(find_or_build_binary) || {
            log_error "Could not find or build binary"
            log_error "Specify a binary path or run from source directory"
            exit 1
        }
        upgrade "${binary_path}"
        ;;
    -*)
        log_error "Unknown option: $1"
        show_usage
        exit 1
        ;;
    *)
        check_root
        upgrade "$1"
        ;;
esac
