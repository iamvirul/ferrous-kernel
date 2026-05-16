#!/usr/bin/env bash
#
# make-image.sh — Build a bootable UEFI disk image for Ferrous Kernel
#
# Creates a 64 MiB FAT32 disk image containing the EFI binary at
# EFI/BOOT/BOOTX64.EFI. The image can be used with:
#   - UTM (macOS) as a bootable USB/disk drive
#   - QEMU directly: -drive format=raw,file=ferrous-boot.img
#   - Any UEFI-capable VM (VMware, VirtualBox, Parallels)
#
# Usage:
#   ./scripts/make-image.sh [--release] [--output <path>]
#
# Options:
#   --release          Build in release mode (default: debug)
#   --output <path>    Output image path (default: ferrous-boot.img)
#
# Requirements (macOS):
#   - Xcode Command Line Tools (hdiutil, newfs_msdos — included in macOS)
#   - Rust nightly with x86_64-unknown-uefi target
#
# Requirements (Linux):
#   - mtools: apt install mtools  |  dnf install mtools
#   - dosfstools: apt install dosfstools

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"

# ---------------------------------------------------------------------------
# Defaults
# ---------------------------------------------------------------------------

BUILD_MODE="debug"
OUTPUT="$PROJECT_ROOT/ferrous-boot.img"
IMAGE_SIZE_MB=64

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------

while [[ $# -gt 0 ]]; do
    case "$1" in
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --output)
            OUTPUT="${2:?--output requires a path}"
            shift 2
            ;;
        *)
            echo "Unknown argument: $1" >&2
            exit 1
            ;;
    esac
done

# ---------------------------------------------------------------------------
# Colours
# ---------------------------------------------------------------------------

GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m'

info() { echo -e "${GREEN}[INFO]${NC} $1"; }
fail() { echo -e "${RED}[FAIL]${NC} $1"; exit 1; }
warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }

# ---------------------------------------------------------------------------
# Detect OS
# ---------------------------------------------------------------------------

OS="$(uname)"
if [[ "$OS" != "Darwin" && "$OS" != "Linux" ]]; then
    fail "Unsupported OS: $OS. Only macOS and Linux are supported."
fi

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------

info "Building Ferrous bootloader (${BUILD_MODE})..."
cd "$PROJECT_ROOT/boot"
if [[ "$BUILD_MODE" == "release" ]]; then
    cargo build --release 2>&1
else
    cargo build 2>&1
fi
cd "$PROJECT_ROOT"

EFI_BINARY="$PROJECT_ROOT/target/x86_64-unknown-uefi/${BUILD_MODE}/ferrous-boot.efi"
if [[ ! -f "$EFI_BINARY" ]]; then
    fail "EFI binary not found at $EFI_BINARY"
fi

EFI_SIZE=$(wc -c < "$EFI_BINARY")
info "EFI binary: $EFI_BINARY (${EFI_SIZE} bytes)"

# ---------------------------------------------------------------------------
# Create disk image
# ---------------------------------------------------------------------------

info "Creating ${IMAGE_SIZE_MB} MiB FAT32 disk image: $OUTPUT"

if [[ "$OS" == "Darwin" ]]; then
    # -----------------------------------------------------------------------
    # macOS: use hdiutil + newfs_msdos (no extra tools required)
    # -----------------------------------------------------------------------

    # 1. Create a zero-filled raw image.
    dd if=/dev/zero of="$OUTPUT" bs=1m count="$IMAGE_SIZE_MB" 2>/dev/null

    # 2. Attach the raw image as a block device (no auto-mount).
    DEVICE=$(hdiutil attach -nomount "$OUTPUT" | awk '{print $1}' | head -1)
    info "Attached as $DEVICE"

    # 3. Format as FAT32 with label FERROUS.
    newfs_msdos -F 32 -v FERROUS "$DEVICE" >/dev/null

    # 4. Mount at a temporary path.
    MOUNT_POINT="$(mktemp -d /tmp/ferrous-efi.XXXXXX)"
    mount -t msdos "$DEVICE" "$MOUNT_POINT"

    # 5. Populate EFI directory structure.
    mkdir -p "$MOUNT_POINT/EFI/BOOT"
    cp "$EFI_BINARY" "$MOUNT_POINT/EFI/BOOT/BOOTX64.EFI"
    info "Copied BOOTX64.EFI → EFI/BOOT/BOOTX64.EFI"

    # 6. Sync, unmount, detach.
    sync
    umount "$MOUNT_POINT" || diskutil unmount "$MOUNT_POINT"
    rmdir "$MOUNT_POINT"
    hdiutil detach "$DEVICE" >/dev/null
    info "Detached $DEVICE"

else
    # -----------------------------------------------------------------------
    # Linux: use mtools (no root/loop required)
    # -----------------------------------------------------------------------

    if ! command -v mformat &>/dev/null; then
        fail "mtools not found. Install with: apt install mtools  |  dnf install mtools"
    fi

    # 1. Create zero-filled image.
    dd if=/dev/zero of="$OUTPUT" bs=1M count="$IMAGE_SIZE_MB" 2>/dev/null

    # 2. Format as FAT32.
    mformat -i "$OUTPUT" -F -v FERROUS ::

    # 3. Create directory structure and copy EFI binary.
    mmd -i "$OUTPUT" ::/EFI
    mmd -i "$OUTPUT" ::/EFI/BOOT
    mcopy -i "$OUTPUT" "$EFI_BINARY" ::/EFI/BOOT/BOOTX64.EFI
    info "Copied BOOTX64.EFI → EFI/BOOT/BOOTX64.EFI"
fi

# ---------------------------------------------------------------------------
# Verify
# ---------------------------------------------------------------------------

FINAL_SIZE=$(wc -c < "$OUTPUT")
info "Image created: $OUTPUT ($(( FINAL_SIZE / 1024 / 1024 )) MiB)"

if [[ "$OS" == "Darwin" ]]; then
    # Quick sanity check: list EFI/BOOT inside the image using hdiutil.
    VERIFY_MOUNT="$(mktemp -d /tmp/ferrous-verify.XXXXXX)"
    VERIFY_DEV=$(hdiutil attach -nomount "$OUTPUT" | awk '{print $1}' | head -1)
    mount -t msdos "$VERIFY_DEV" "$VERIFY_MOUNT"
    if [[ -f "$VERIFY_MOUNT/EFI/BOOT/BOOTX64.EFI" ]]; then
        info "Verified: EFI/BOOT/BOOTX64.EFI present in image"
    else
        umount "$VERIFY_MOUNT" || true
        hdiutil detach "$VERIFY_DEV" >/dev/null || true
        rmdir "$VERIFY_MOUNT"
        fail "EFI binary not found in image — something went wrong"
    fi
    umount "$VERIFY_MOUNT" || diskutil unmount "$VERIFY_MOUNT"
    hdiutil detach "$VERIFY_DEV" >/dev/null
    rmdir "$VERIFY_MOUNT"
else
    if mdir -i "$OUTPUT" ::/EFI/BOOT/ 2>/dev/null | grep -qi "BOOTX64"; then
        info "Verified: EFI/BOOT/BOOTX64.EFI present in image"
    else
        fail "EFI binary not found in image — something went wrong"
    fi
fi

# ---------------------------------------------------------------------------
# Usage hint
# ---------------------------------------------------------------------------

echo ""
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo " Ferrous Kernel — Bootable Image Ready"
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
echo ""
echo " Image: $OUTPUT"
echo ""
echo " UTM (macOS):"
echo "   1. New VM → Emulate → Other"
echo "   2. Architecture: x86_64, Machine: q35, UEFI: ON"
echo "   3. Skip ISO — go straight to Summary"
echo "   4. VM settings → Drives → Add Drive → import $OUTPUT"
echo "      Interface: VirtIO, set as bootable"
echo "   5. VM settings → Serial → Enable serial port"
echo "      Mode: Builtin Terminal (serial output appears here)"
echo "   6. Display: can be disabled — no display output yet"
echo "   7. Boot — watch the serial console window"
echo ""
echo " QEMU (direct):"
echo "   qemu-system-x86_64 \\"
echo "     -machine q35 \\"
echo "     -drive if=pflash,format=raw,readonly=on,file=\$(brew --prefix qemu)/share/qemu/edk2-x86_64-code.fd \\"
echo "     -drive format=raw,file=$OUTPUT \\"
echo "     -m 256M -serial stdio -display none -no-reboot"
echo ""
echo " What you'll see: all kernel boot messages on serial, then halt."
echo " No display output until Phase 4 (framebuffer) is implemented."
echo "━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━"
