# Development Environment Setup Guide

**Version:** 0.1  
**Date:** 2026-01-04  
**Status:** Phase 0

---

## Overview

This guide walks you through setting up a development environment for Ferrous Kernel. The kernel is written in Rust and targets x86_64 architecture.

---

## Prerequisites

### Required Software

1. **Rust Toolchain**
   - Version: Latest nightly (required for `no_std` kernel development)
   - Install from: https://rustup.rs/
   - Verify: `rustc --version` (should show nightly)

2. **QEMU**
   - Version: 6.0+ recommended
   - Purpose: Emulation and testing
   - Installation:
     - **macOS**: `brew install qemu`
     - **Linux (Ubuntu/Debian)**: `sudo apt-get install qemu-system-x86`
     - **Linux (Fedora)**: `sudo dnf install qemu-system-x86`
     - **Windows**: Download from https://www.qemu.org/download/

3. **Cross-Compilation Tools**
   - **LLVM/Clang** (for linking, optional but recommended)
   - **GCC** (fallback linker)
   - Installation:
     - **macOS**: `brew install llvm`
     - **Linux**: Usually pre-installed or `sudo apt-get install gcc clang`
     - **Windows**: Install via MSYS2 or WSL

4. **Git**
   - Version: 2.0+
   - Installation: https://git-scm.com/downloads

5. **Build Tools**
   - **make** (for build scripts, optional)
   - **bash** (for scripts, macOS/Linux/WSL)

### Optional but Recommended

- **rust-analyzer** (IDE support) - Install via your IDE/editor
- **GDB** or **LLDB** (debugging) - `brew install gdb` or `sudo apt-get install gdb`
- **hexdump** or **xxd** (binary inspection)

---

## Setup Steps

### 1. Install Rust Toolchain

```bash
# Install rustup (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Install nightly toolchain
rustup toolchain install nightly

# Set nightly as default (recommended for kernel development)
rustup default nightly

# Add rust-src component (needed for some tools)
rustup component add rust-src

# Verify installation
rustc --version
cargo --version
```

### 2. Clone the Repository

```bash
# Clone the repository
git clone https://github.com/iamvirul/ferrous-kernel.git
cd ferrous-kernel

# Verify you're on the correct branch
git branch
```

### 3. Verify Build System

```bash
# Check that Cargo workspace is set up correctly
cargo check --workspace

# If this succeeds, the build system is configured correctly
```

**Note:** The build system is in Phase 0 and may not compile yet. This is expected. The workspace structure should be valid.

### 4. Install Additional Rust Tools

```bash
# Install rustfmt (code formatting)
rustup component add rustfmt

# Install clippy (linter)
rustup component add clippy

# Verify tools
cargo fmt --version
cargo clippy --version
```

### 5. Verify QEMU Installation

```bash
# Check QEMU version
qemu-system-x86_64 --version

# Expected output: QEMU emulator version 6.0.0 or later
```

---

## Platform-Specific Setup

### macOS

```bash
# Install dependencies via Homebrew
brew install qemu llvm

# Set up environment (add to ~/.zshrc or ~/.bash_profile)
export PATH="/usr/local/opt/llvm/bin:$PATH"
```

### Linux (Ubuntu/Debian)

```bash
# Install dependencies
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    qemu-system-x86 \
    gdb \
    clang \
    llvm \
    git
```

### Linux (Fedora)

```bash
# Install dependencies
sudo dnf install -y \
    gcc \
    gcc-c++ \
    make \
    qemu-system-x86 \
    gdb \
    clang \
    llvm \
    git
```

### Windows (WSL2 Recommended)

1. Install WSL2 with Ubuntu
2. Follow Linux (Ubuntu) instructions above
3. Install QEMU in WSL:
   ```bash
   sudo apt-get install qemu-system-x86
   ```

---

## Verifying Your Setup

Run these commands to verify your environment is set up correctly:

```bash
# Check Rust version (should show nightly)
rustc --version

# Check Cargo version
cargo --version

# Check QEMU installation
qemu-system-x86_64 --version

# Check workspace structure
cargo check --workspace 2>&1 | head -20

# Check formatting tool
cargo fmt --version

# Check linter
cargo clippy --version
```

All commands should execute without errors (though `cargo check` may show compilation errors since code isn't implemented yet).

---

## Development Workflow

### Building

```bash
# Build the entire workspace
cargo build

# Build in release mode
cargo build --release

# Build specific crate
cargo build -p ferrous-kernel

# Build with verbose output
cargo build --verbose
```

### Formatting Code

```bash
# Format all code
cargo fmt

# Check formatting (CI)
cargo fmt --check
```

### Linting

```bash
# Run clippy on all crates
cargo clippy --workspace --all-targets

# Run clippy with warnings as errors (CI)
cargo clippy --workspace --all-targets -- -D warnings
```

### Testing

```bash
# Run all tests (when implemented)
cargo test

# Run tests for specific crate
cargo test -p ferrous-core

# Run tests with output
cargo test -- --nocapture
```

---

## IDE Setup

### VS Code

1. Install the "rust-analyzer" extension
2. Open the repository folder
3. rust-analyzer should automatically detect the workspace

### CLion / IntelliJ IDEA

1. Install the Rust plugin
2. Open the repository as a Cargo project
3. Configure Rust toolchain in settings

### Vim/Neovim

1. Install rust-analyzer LSP client
2. Configure LSP for Rust files
3. Use `:LspInfo` to verify rust-analyzer is connected

---

## Troubleshooting

### Rust Toolchain Issues

**Problem:** `rustc: command not found`

**Solution:**
```bash
# Add Cargo to PATH
source $HOME/.cargo/env

# Or add to your shell profile (~/.zshrc, ~/.bashrc)
export PATH="$HOME/.cargo/bin:$PATH"
```

**Problem:** Wrong Rust version (stable instead of nightly)

**Solution:**
```bash
# Set nightly as default
rustup default nightly

# Verify
rustc --version
```

### QEMU Issues

**Problem:** `qemu-system-x86_64: command not found`

**Solution:**
- Verify QEMU is installed: `which qemu-system-x86_64`
- Add QEMU to PATH if installed in non-standard location
- Reinstall QEMU if necessary

**Problem:** QEMU version too old

**Solution:**
- Update QEMU to version 6.0 or later
- On macOS: `brew upgrade qemu`
- On Linux: Update via package manager

### Build System Issues

**Problem:** `cargo check` fails with workspace errors

**Solution:**
- Verify all `Cargo.toml` files are present
- Check workspace members in root `Cargo.toml`
- Ensure all crate directories exist

**Problem:** Linker errors

**Solution:**
- Install LLVM or GCC
- Verify linker is in PATH
- Check `.cargo/config.toml` for linker configuration (when added)

---

## Next Steps

Once your environment is set up:

1. Read the [CONTRIBUTING.md](CONTRIBUTING.md) guide
2. Review the [ARCHITECTURE.md](ARCHITECTURE.md) documents
3. Check [ROADMAP.md](ROADMAP.md) for current phase work
4. Look for `good-first-issue` labels in GitHub issues
5. Start contributing!

---

## Getting Help

If you encounter issues:

1. Check this guide for common problems
2. Review [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines
3. Open an issue on GitHub with:
   - Your operating system
   - Rust version (`rustc --version`)
   - Error messages
   - Steps to reproduce

---

## References

- [Rust Book](https://doc.rust-lang.org/book/) - Learn Rust
- [Embedded Rust Book](https://docs.rust-embedded.org/book/) - no_std Rust
- [QEMU Documentation](https://www.qemu.org/documentation/)
- [Ferrous Kernel CONTRIBUTING.md](CONTRIBUTING.md)
- [Ferrous Kernel ROADMAP.md](ROADMAP.md)

---

**Last Updated:** 2026-01-04

