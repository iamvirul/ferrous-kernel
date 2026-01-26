#!/usr/bin/env bash
#
# Run Ferrous Kernel in QEMU with UEFI
#
# This script builds the bootloader and runs it in QEMU with OVMF (UEFI firmware).
#
# Prerequisites:
#   - Rust nightly toolchain with x86_64-unknown-uefi target
#   - QEMU with x86_64 support
#   - OVMF UEFI firmware (usually provided by your package manager)
#
# Usage:
#   ./scripts/run-qemu.sh [--release]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# Build mode
BUILD_MODE="debug"
if [[ "${1:-}" == "--release" ]]; then
    BUILD_MODE="release"
fi

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

error() {
    echo -e "${RED}[ERROR]${NC} $1"
    exit 1
}

# Check for required tools
check_requirements() {
    if ! command -v cargo &> /dev/null; then
        error "cargo not found. Please install Rust."
    fi

    if ! command -v qemu-system-x86_64 &> /dev/null; then
        error "qemu-system-x86_64 not found. Please install QEMU."
    fi

    # Check for OVMF
    OVMF_PATHS=(
        "/usr/share/OVMF/OVMF_CODE.fd"
        "/usr/share/edk2-ovmf/x64/OVMF_CODE.fd"
        "/usr/share/edk2/x64/OVMF_CODE.fd"
        "/opt/homebrew/share/qemu/edk2-x86_64-code.fd"
        "/usr/local/share/qemu/edk2-x86_64-code.fd"
    )

    OVMF_CODE=""
    for path in "${OVMF_PATHS[@]}"; do
        if [[ -f "$path" ]]; then
            OVMF_CODE="$path"
            break
        fi
    done

    if [[ -z "$OVMF_CODE" ]]; then
        warn "OVMF UEFI firmware not found in common locations."
        warn "Please install OVMF/EDK2 or set OVMF_CODE environment variable."
        warn "On macOS: brew install qemu (includes OVMF)"
        warn "On Ubuntu: apt install ovmf"
        warn "On Fedora: dnf install edk2-ovmf"
        error "Cannot continue without UEFI firmware."
    fi

    info "Using OVMF: $OVMF_CODE"
}

# Build the bootloader
build_bootloader() {
    info "Building bootloader (${BUILD_MODE})..."

    cd "$PROJECT_ROOT/boot"

    if [[ "$BUILD_MODE" == "release" ]]; then
        cargo build --release
    else
        cargo build
    fi

    cd "$PROJECT_ROOT"
}

# Create the EFI boot disk structure
create_boot_disk() {
    info "Creating boot disk structure..."

    BOOT_DISK="$PROJECT_ROOT/target/boot-disk"
    EFI_DIR="$BOOT_DISK/EFI/BOOT"

    mkdir -p "$EFI_DIR"

    # Copy the bootloader to the correct EFI path
    BOOTLOADER_PATH="$PROJECT_ROOT/target/x86_64-unknown-uefi/${BUILD_MODE}/ferrous-boot.efi"

    if [[ ! -f "$BOOTLOADER_PATH" ]]; then
        error "Bootloader not found at $BOOTLOADER_PATH"
    fi

    cp "$BOOTLOADER_PATH" "$EFI_DIR/BOOTX64.EFI"

    info "Boot disk created at $BOOT_DISK"
}

# Run QEMU
run_qemu() {
    info "Starting QEMU..."

    BOOT_DISK="$PROJECT_ROOT/target/boot-disk"

    qemu-system-x86_64 \
        -enable-kvm 2>/dev/null || true \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
        -drive format=raw,file=fat:rw:"$BOOT_DISK" \
        -m 256M \
        -serial stdio \
        -no-reboot \
        -display none
}

# Alternative run without KVM (for macOS)
run_qemu_no_kvm() {
    info "Starting QEMU (without KVM)..."

    BOOT_DISK="$PROJECT_ROOT/target/boot-disk"

    qemu-system-x86_64 \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
        -drive format=raw,file=fat:rw:"$BOOT_DISK" \
        -m 256M \
        -serial stdio \
        -no-reboot \
        -display none
}

main() {
    info "Ferrous Kernel QEMU Runner"
    info "=========================="

    check_requirements
    build_bootloader
    create_boot_disk

    # Try with KVM first, fall back to without
    if [[ "$(uname)" == "Linux" ]]; then
        run_qemu
    else
        run_qemu_no_kvm
    fi
}

main "$@"
