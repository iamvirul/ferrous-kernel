# QEMU Testing Guide

**Last Updated:** 2026-03-07
**Applies to:** Phase 1 (Proof of Life)

---

## Overview

Ferrous Kernel is developed and tested primarily on QEMU x86_64 with OVMF (Open Virtual Machine Firmware), which provides a UEFI environment matching the real hardware target.

This document covers:

- How to run the kernel in QEMU
- Expected output for each Phase 1 milestone
- Automated boot verification
- Troubleshooting common failures
- Hardware requirements for physical machine testing

---

## Prerequisites

| Tool | Minimum Version | Install |
|------|----------------|---------|
| Rust nightly | latest | `rustup toolchain install nightly` |
| QEMU | 6.0+ | see below |
| OVMF / EDK2 | any | bundled with QEMU on macOS |

### Install QEMU

**macOS**
```bash
brew install qemu
# OVMF is bundled — no extra step needed.
```

**Ubuntu / Debian**
```bash
sudo apt install qemu-system-x86 ovmf
```

**Fedora**
```bash
sudo dnf install qemu-system-x86 edk2-ovmf
```

**Arch Linux**
```bash
sudo pacman -S qemu-system-x86 edk2-ovmf
```

---

## Running in QEMU

### Quick start

```bash
./scripts/run-qemu.sh
```

This builds the bootloader in debug mode and launches QEMU. Serial output is written to stdout.

### Release build

```bash
./scripts/run-qemu.sh --release
```

### What the script does

1. Checks for `qemu-system-x86_64` and OVMF firmware
2. Builds `boot/` with `cargo build` (UEFI target)
3. Creates a minimal FAT boot disk at `target/boot-disk/EFI/BOOT/BOOTX64.EFI`
4. Launches QEMU with:
   - 256 MB RAM
   - No display (headless)
   - Serial output piped to stdout
   - KVM enabled on Linux if `/dev/kvm` is writable; software emulation on macOS

### Running QEMU manually

```bash
qemu-system-x86_64 \
    -drive if=pflash,format=raw,readonly=on,file=/opt/homebrew/share/qemu/edk2-x86_64-code.fd \
    -drive format=raw,file=fat:rw:target/boot-disk \
    -m 256M \
    -serial stdio \
    -no-reboot \
    -display none
```

Adjust the OVMF path for your system:

| OS | Default OVMF path |
|----|-------------------|
| macOS (Homebrew) | `/opt/homebrew/share/qemu/edk2-x86_64-code.fd` |
| Ubuntu | `/usr/share/OVMF/OVMF_CODE.fd` |
| Fedora | `/usr/share/edk2/x64/OVMF_CODE.fd` |
| Arch | `/usr/share/edk2-ovmf/x64/OVMF_CODE.fd` |

---

## Expected Output — Phase 1.1 (Bare Metal Boot)

A successful Phase 1.1 boot produces the following on the serial console. This output is the authoritative reference for the `verify-boot.sh` script.

```
========================================
  Ferrous Kernel UEFI Bootloader v0.1
========================================

[OK] UEFI boot services initialized
[INFO] Firmware: EDK II (rev 65536)
[INFO] UEFI Revision: 2.70
[...] Retrieving memory map
    Found 99 memory regions
[OK] Memory map retrieved

Memory Map Summary:
-------------------
  0x0000000000 - 0x00000a0000: Conventional (640 KB)
  ... (entries vary by QEMU version and RAM size)

[...] Looking for ACPI tables
[OK] ACPI RSDP found at: 0xf77e014
[...] Looking for GOP framebuffer
[OK] Framebuffer: 1280x800 @ 0x80000000

[INFO] Total memory:  12543 MB
[INFO] Usable memory: 249 MB

========================================
  Preparing for kernel handoff...
========================================

[OK] KernelBootInfo populated (magic=0xfe220b00cafe0001)

=== Ferrous Kernel ===
[OK] kernel_entry: BootInfo validated
[OK] Kernel entered successfully!
Hello from Ferrous!

Memory map entries: 99
[INFO] ACPI RSDP present
[INFO] Framebuffer present

Kernel halting. Phase 1.2 (runtime setup) not yet implemented.
```

### Verification checklist

After running, confirm these lines appear in the output:

- [ ] `=== Ferrous Kernel ===`
- [ ] `kernel_entry: BootInfo validated`
- [ ] `Kernel entered successfully!`
- [ ] `Hello from Ferrous!`

The boot info magic (`0xfe220b00cafe0001`) must match exactly — it validates the `KernelBootInfo` ABI between bootloader and kernel.

---

## Automated Boot Verification

`scripts/verify-boot.sh` runs the full build + boot cycle and checks for all required output strings. Exit code 0 = pass, 1 = fail.

```bash
# Standard verification
./scripts/verify-boot.sh

# Release build
./scripts/verify-boot.sh --release

# Custom timeout (default 30s)
./scripts/verify-boot.sh --timeout 60
```

### CI integration

Add this to your CI pipeline (GitHub Actions example):

```yaml
- name: Verify boot
  run: ./scripts/verify-boot.sh --timeout 60
```

The serial log is always saved to `target/serial-verify.log` for inspection on failure.

---

## QEMU Configuration Details

### Memory

Default: 256 MB (`-m 256M`). The bootloader detects and reports usable memory in the serial output. Changing this affects the memory map but not correctness.

```bash
# Test with different memory sizes
qemu-system-x86_64 ... -m 512M ...
qemu-system-x86_64 ... -m 1G ...
```

### KVM acceleration

On Linux with KVM available, the run script enables `-enable-kvm` automatically. KVM makes boot much faster but is not required for correctness. On macOS, QEMU uses its software TCG backend.

To force software emulation (useful for debugging):
```bash
# Remove -enable-kvm from the command
qemu-system-x86_64 ... -machine accel=tcg ...
```

### Serial output modes

```bash
# stdout (default in run-qemu.sh)
-serial stdio

# Log to file
-serial file:serial.log

# TCP socket (for remote inspection)
-serial tcp::4444,server,nowait
# Connect: nc localhost 4444
```

### Display options

```bash
# No display (headless, default)
-display none

# VGA display (useful if framebuffer driver is added later)
-display sdl
-vga std
```

---

## Troubleshooting

### QEMU exits immediately with no output

**Cause:** OVMF not found or the boot disk path is wrong.

**Fix:**
```bash
# Verify OVMF exists
ls /opt/homebrew/share/qemu/edk2-x86_64-code.fd

# Verify boot disk was created
ls target/boot-disk/EFI/BOOT/BOOTX64.EFI

# Run the build step manually
cd boot && cargo build && cd ..
```

### "FATAL: kernel_entry received null BootInfo pointer"

**Cause:** The `KernelBootInfo` static was not populated before the jump. This is a bug.

**Fix:** Check `boot/src/main.rs` — the `KERNEL_BOOT_INFO` write must happen before `exit_boot_services`.

### "FATAL: KernelBootInfo magic/version mismatch"

**Cause:** The bootloader and kernel are using different versions of the `ferrous-boot-info` crate, or the `KernelBootInfo` struct layout has changed without updating `BOOT_INFO_MAGIC`.

**Fix:**
```bash
# Clean and rebuild everything
cargo clean
cd boot && cargo build && cd ..
```

### "Hello from Ferrous!" does not appear

**Cause:** Either the UART init failed, or the kernel halted before reaching `kernel_main`.

**Debug steps:**
1. Check that "kernel_entry: BootInfo validated" appears — if not, the handoff failed
2. Check that the magic value printed matches `0xfe220b00cafe0001`
3. Inspect `target/serial-verify.log` for the full output

### QEMU hangs and never halts

**Expected behavior:** The kernel prints its output then executes `hlt` in a loop with interrupts disabled. QEMU will appear to hang — this is correct. The `run-qemu.sh` script will keep running until you press `Ctrl+C`. The `verify-boot.sh` script detects the output and kills QEMU automatically.

### KVM permission denied (Linux)

```bash
# Add yourself to the kvm group
sudo usermod -aG kvm $USER
# Log out and back in, then verify
ls -la /dev/kvm
```

### OVMF not found on macOS

```bash
brew install qemu
# OVMF is installed alongside QEMU
ls /opt/homebrew/share/qemu/edk2-x86_64-code.fd
```

---

## Hardware Requirements (Physical Machines)

Real hardware testing is optional during Phase 1 but confirms UEFI compatibility beyond QEMU's emulation.

### Minimum requirements

| Component | Requirement |
|-----------|-------------|
| Architecture | x86_64 (64-bit) |
| Firmware | UEFI 2.0 or later (Secure Boot must be **disabled**) |
| RAM | 256 MB minimum |
| Storage | USB drive or any FAT32-formatted storage for the boot disk |

### Booting from USB

```bash
# Build the bootloader
cd boot && cargo build && cd ..

# Create the EFI directory structure on a FAT32 USB drive
# (replace /dev/sdX with your USB device — double-check before running!)
sudo mkfs.fat -F32 /dev/sdX1
sudo mount /dev/sdX1 /mnt/usb
sudo mkdir -p /mnt/usb/EFI/BOOT
sudo cp target/x86_64-unknown-uefi/debug/ferrous-boot.efi /mnt/usb/EFI/BOOT/BOOTX64.EFI
sudo umount /mnt/usb
```

Then boot the machine from the USB drive. Connect a serial cable (or use a USB-to-serial adapter) to capture COM1 output at 115200 baud 8N1.

### Known UEFI compatibility notes

- **Secure Boot:** Must be disabled. Ferrous is not signed.
- **CSM (Compatibility Support Module):** Should be disabled. Ferrous requires native UEFI, not legacy BIOS emulation.
- **GOP framebuffer:** Tested with EDK2's emulated GOP. Real hardware may report different resolutions or pixel formats — these are passed through correctly via `KernelBootInfo`.

### Hardware not yet tested

Physical hardware testing has not been performed as of Phase 1.1. If you test on real hardware, please open an issue with:

- Machine make/model
- UEFI firmware version
- Whether it booted successfully
- Full serial console output

---

## Known Limitations (Phase 1.1)

| Limitation | Detail |
|------------|--------|
| No GDT/IDT | Using UEFI's descriptor tables. Any exception triple-faults. |
| No kernel heap | No memory allocation after handoff. |
| Bootstrap stack only | 16 KiB, no guard page. Stack overflow is undetected. |
| No interrupt handling | Hardware interrupts disabled (`cli` before jump). |
| Single core | Only the boot processor is active. |
| Halts immediately | After printing boot info, the kernel executes `hlt` forever. |

These are all tracked in Phase 1.2 and 1.3 issues.

---

## Related Documents

- [SETUP.md](SETUP.md) — Development environment setup
- [BOOT_ARCHITECTURE.md](BOOT_ARCHITECTURE.md) — Boot process design
- [ROADMAP.md](ROADMAP.md) — Phase 1 milestones and task tracking
- [adr/ADR-0001-kernel-entry-point-handoff.md](adr/ADR-0001-kernel-entry-point-handoff.md) — Kernel handoff design decisions
