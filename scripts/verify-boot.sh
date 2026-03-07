#!/usr/bin/env bash
#
# verify-boot.sh — Automated boot verification for Ferrous Kernel
#
# Builds the bootloader, runs it in QEMU with a timeout, captures serial
# output, and checks that the expected boot strings are present. Exits 0
# on success, 1 on failure. Suitable for CI pipelines.
#
# Usage:
#   ./scripts/verify-boot.sh [--release] [--timeout <seconds>]
#
# Options:
#   --release        Build in release mode (default: debug)
#   --timeout <s>    Seconds to wait for boot output (default: 30)
#
# Expected output (any missing string = failure):
#   "=== Ferrous Kernel ==="
#   "kernel_entry: BootInfo validated"
#   "Kernel entered successfully!"
#   "Hello from Ferrous!"

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

BUILD_MODE="debug"
TIMEOUT=30

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --timeout)
            TIMEOUT="${2:?--timeout requires a value}"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

pass() { echo -e "${GREEN}[PASS]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; }
info() { echo -e "${GREEN}[INFO]${NC} $1"; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }

# ---------------------------------------------------------------------------
# Requirements check
# ---------------------------------------------------------------------------

check_requirements() {
    if ! command -v cargo &>/dev/null; then
        fail "cargo not found. Install Rust from https://rustup.rs/"
        exit 1
    fi

    if ! command -v qemu-system-x86_64 &>/dev/null; then
        fail "qemu-system-x86_64 not found. Install QEMU."
        exit 1
    fi

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
        fail "OVMF UEFI firmware not found."
        fail "macOS: brew install qemu | Ubuntu: apt install ovmf | Fedora: dnf install edk2-ovmf"
        exit 1
    fi
}

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

build() {
    info "Building bootloader (${BUILD_MODE})..."
    cd "$PROJECT_ROOT/boot"
    if [[ "$BUILD_MODE" == "release" ]]; then
        cargo build --release 2>&1
    else
        cargo build 2>&1
    fi
    cd "$PROJECT_ROOT"
}

# ---------------------------------------------------------------------------
# Boot disk
# ---------------------------------------------------------------------------

create_boot_disk() {
    BOOT_DISK="$PROJECT_ROOT/target/boot-disk"
    EFI_DIR="$BOOT_DISK/EFI/BOOT"
    mkdir -p "$EFI_DIR"

    BOOTLOADER="$PROJECT_ROOT/target/x86_64-unknown-uefi/${BUILD_MODE}/ferrous-boot.efi"
    if [[ ! -f "$BOOTLOADER" ]]; then
        fail "Bootloader binary not found at $BOOTLOADER"
        exit 1
    fi
    cp "$BOOTLOADER" "$EFI_DIR/BOOTX64.EFI"
}

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

# Strings that must appear in serial output for the boot to be considered
# successful. Order does not matter — all must be present.
EXPECTED_STRINGS=(
    "=== Ferrous Kernel ==="
    "kernel_entry: BootInfo validated"
    "Kernel entered successfully!"
    "Hello from Ferrous!"
)

run_and_verify() {
    SERIAL_LOG="$PROJECT_ROOT/target/serial-verify.log"
    : > "$SERIAL_LOG"  # truncate

    info "Starting QEMU (timeout: ${TIMEOUT}s)..."
    BOOT_DISK="$PROJECT_ROOT/target/boot-disk"

    KVM_FLAG=""
    if [[ "$(uname)" == "Linux" ]] && [[ -w /dev/kvm ]]; then
        KVM_FLAG="-enable-kvm"
    fi

    # Run QEMU in the background, writing serial to a log file.
    qemu-system-x86_64 \
        $KVM_FLAG \
        -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
        -drive format=raw,file=fat:rw:"$BOOT_DISK" \
        -m 256M \
        -serial "file:${SERIAL_LOG}" \
        -no-reboot \
        -display none &

    QEMU_PID=$!

    # Poll the log file until all expected strings appear or timeout expires.
    ELAPSED=0
    FOUND=0

    while [[ $ELAPSED -lt $TIMEOUT ]]; do
        sleep 1
        ELAPSED=$((ELAPSED + 1))

        # Check that QEMU is still alive.
        if ! kill -0 "$QEMU_PID" 2>/dev/null; then
            break
        fi

        # Check if all expected strings are present.
        ALL_FOUND=1
        for s in "${EXPECTED_STRINGS[@]}"; do
            if ! grep -qF "$s" "$SERIAL_LOG" 2>/dev/null; then
                ALL_FOUND=0
                break
            fi
        done

        if [[ $ALL_FOUND -eq 1 ]]; then
            FOUND=1
            break
        fi
    done

    # Kill QEMU regardless of outcome.
    kill "$QEMU_PID" 2>/dev/null || true
    wait "$QEMU_PID" 2>/dev/null || true

    return $((1 - FOUND))
}

# ---------------------------------------------------------------------------
# Report
# ---------------------------------------------------------------------------

report_results() {
    local serial_log="$PROJECT_ROOT/target/serial-verify.log"
    local all_passed=0

    echo ""
    echo "Boot verification results:"
    echo "--------------------------"

    for s in "${EXPECTED_STRINGS[@]}"; do
        if grep -qF "$s" "$serial_log" 2>/dev/null; then
            pass "\"$s\""
        else
            fail "\"$s\" — NOT FOUND"
            all_passed=1
        fi
    done

    echo ""
    echo "Full serial output:"
    echo "-------------------"
    if [[ -f "$serial_log" ]]; then
        cat "$serial_log"
    else
        warn "(no serial log)"
    fi

    return $all_passed
}

# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

main() {
    info "Ferrous Kernel — Boot Verification"
    info "==================================="
    echo ""

    check_requirements
    build
    create_boot_disk

    if run_and_verify; then
        report_results
        echo ""
        pass "Boot verification PASSED"
        exit 0
    else
        report_results
        echo ""
        fail "Boot verification FAILED"
        exit 1
    fi
}

main "$@"
