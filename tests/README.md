# Tests

This directory contains integration and unit tests for the Ferrous Kernel project.

## Structure

```
tests/
├── README.md          # This file
└── boot_tests.rs      # Bootloader integration tests
```

## Test Categories

### Bootloader Tests (`boot_tests.rs`)

Tests for the UEFI bootloader that verify:

- **Memory region type handling**: Documents and validates the expected UEFI memory types (Conventional, BootServicesCode/Data, RuntimeServices, ACPI, MMIO, etc.)
- **Boot sequence documentation**: Validates the expected boot stages from UEFI firmware to kernel handoff

These tests serve as living documentation for the boot process. The actual runtime behavior is tested via QEMU in CI.

## Running Tests

```bash
# Run all tests
cargo test

# Run specific test file
cargo test --test boot_tests

# Run with output
cargo test -- --nocapture
```

## CI Integration

Tests are automatically run in the GitHub Actions CI pipeline. The bootloader build verification happens in the `bootloader.yml` workflow, which:

1. Builds the bootloader for `x86_64-unknown-uefi` target
2. Runs unit tests
3. (Future) Runs QEMU-based integration tests

## Adding New Tests

When adding tests:

1. Place unit tests in the relevant module's source file
2. Place integration tests in this `tests/` directory
3. Follow the naming convention: `<component>_tests.rs`
4. Document the test purpose with comments

## Test Philosophy

From the project charter:

- Debug builds are paranoid (extensive assertions)
- Errors are explicit, never silently ignored
- Fault injection testing is a first-class concern (Phase 3+)
- Integration tests run in QEMU
