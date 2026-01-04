# Ferrous Kernel - Coding Standards

**Version:** 0.1  
**Date:** 2026-01-04  
**Status:** Active Policy

---

## Overview

This document defines coding standards and style guidelines for Ferrous Kernel. These standards ensure code consistency, readability, and maintainability across the project.

**Related Documents:**
- [UNSAFE_GUIDELINES.md](UNSAFE_GUIDELINES.md) - Unsafe Rust policy
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines
- [CHARTER.md](CHARTER.md) - Design principles

---

## Rust Style Guide

### Code Formatting

**Use `rustfmt` for all code formatting.**

```bash
# Format code
cargo fmt

# Check formatting (for CI)
cargo fmt --check
```

**Default settings:**
- Use standard `rustfmt` configuration
- 4 spaces for indentation
- 100 character line width (default)
- No trailing whitespace

### Naming Conventions

Follow Rust's standard naming conventions:

**Types (Structs, Enums, Traits):**
- PascalCase: `PageTable`, `VirtualAddress`, `MemoryError`

**Functions and Variables:**
- snake_case: `map_page`, `virt_addr`, `page_table`

**Constants:**
- SCREAMING_SNAKE_CASE: `PAGE_SIZE`, `MAX_FRAMES`

**Modules:**
- snake_case: `memory`, `scheduling`, `ipc`

**Lifetimes:**
- Single lowercase letter: `'a`, `'static`

**Example:**
```rust
const PAGE_SIZE: usize = 4096;

struct PageTable {
    entries: [PageTableEntry; 512],
}

fn map_page(
    page_table: &mut PageTable,
    virt_addr: VirtualAddress,
    phys_frame: PhysicalFrame,
) -> Result<(), MemoryError> {
    // Implementation
}
```

---

## Code Organization

### Module Structure

**Organize code into logical modules:**

```rust
// Module with related functionality
mod memory {
    mod physical;
    mod virtual;
    mod allocator;
    
    pub use physical::PhysicalFrame;
    pub use virtual::VirtualAddress;
}
```

### File Organization

- One main type per file (when reasonable)
- Related types in the same module
- Public API in `mod.rs` or `lib.rs`
- Implementation details in submodules

### Import Organization

**Order imports logically:**

1. Standard library (`std` / `core`)
2. External crates
3. Internal crate imports
4. Current crate imports

**Example:**
```rust
// Standard library
use core::ptr;
use alloc::vec::Vec;

// External crates
use spin::Mutex;

// Internal crate
use crate::memory::VirtualAddress;
use crate::error::MemoryError;

// Current module
use super::PageFlags;
```

### Visibility

- Default to private (`pub` only when necessary)
- Use `pub(crate)` for internal API
- Use `pub` only for public API
- Document all public items

---

## Documentation Standards

### Public API Documentation

**All public items must be documented:**

```rust
/// Maps a virtual page to a physical frame.
///
/// This function creates a page table entry mapping the virtual address
/// to the physical frame with the specified flags.
///
/// # Arguments
///
/// * `virt_addr` - Virtual address to map
/// * `phys_frame` - Physical frame to map to
/// * `flags` - Page flags (read, write, execute, etc.)
///
/// # Returns
///
/// Returns `Ok(())` on success, or `Err(MemoryError)` on failure.
///
/// # Errors
///
/// - `MemoryError::InvalidAddress` - Invalid virtual address
/// - `MemoryError::AlreadyMapped` - Address already mapped
/// - `MemoryError::OutOfMemory` - Cannot allocate page table entries
///
/// # Safety
///
/// This function is unsafe because it directly manipulates page tables.
/// See function implementation for safety requirements.
pub unsafe fn map_page(
    virt_addr: VirtualAddress,
    phys_frame: PhysicalFrame,
    flags: PageFlags,
) -> Result<(), MemoryError> {
    // Implementation
}
```

### Code Comments

**Comments explain "why", not "what":**

```rust
// GOOD: Explains why
// We use a bitmap allocator here because it's simple and predictable,
// which is more important than raw performance for the kernel heap.

// BAD: Explains what (code is self-explanatory)
// Allocate a frame from the bitmap
let frame = bitmap.allocate();
```

### Documentation Format

- Use Markdown in doc comments
- Include examples for complex APIs
- Document all parameters and return values
- Document errors and safety requirements
- Link to related types/functions

---

## Error Handling

### Error Types

**Use structured error types:**

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryError {
    OutOfMemory,
    InvalidAddress,
    AlreadyMapped,
    NotMapped,
    InvalidFlags,
}
```

### Error Handling Philosophy

- **Explicit errors** - Return `Result<T, E>`, don't panic (unless kernel bug)
- **No silent failures** - All errors must be handled or propagated
- **Error context** - Include relevant information in errors
- **Error propagation** - Use `?` operator, add context with `.map_err()`

**Example:**
```rust
fn allocate_frames(count: usize) -> Result<Vec<PhysicalFrame>, MemoryError> {
    let mut frames = Vec::new();
    
    for _ in 0..count {
        let frame = self.inner_alloc()
            .ok_or(MemoryError::OutOfMemory)?;
        frames.push(frame);
    }
    
    Ok(frames)
}
```

### Panics

**Panic only for kernel bugs, not user errors:**

- Panic when invariants are violated (kernel bug)
- Return errors for invalid input (user error)
- Use `unwrap()` only when you can prove it won't panic
- Use `expect()` with descriptive messages

---

## Type Safety

### Newtype Patterns

**Use newtypes for type safety:**

```rust
// GOOD: Type-safe addresses
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct VirtualAddress(u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct PhysicalAddress(u64);

// BAD: Raw u64 (easy to mix up)
fn map_page(virt_addr: u64, phys_addr: u64) { ... }
```

### Avoid Primitive Obsession

- Use types instead of raw primitives
- Create types for domain concepts
- Leverage Rust's type system for safety

---

## Code Comments

### When to Comment

- **Complex algorithms** - Explain the approach
- **Non-obvious code** - Explain why, not what
- **Workarounds** - Document temporary solutions
- **Performance optimizations** - Explain tradeoffs
- **Safety invariants** - Document unsafe code (see UNSAFE_GUIDELINES.md)

### Comment Style

- Use `//` for single-line comments
- Use `///` for documentation comments
- Use `//!` for module-level documentation
- Comments should be clear and concise

---

## Testing Standards

### Unit Tests

**Test each module:**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_map_page_success() {
        // Arrange
        let mut page_table = PageTable::new().unwrap();
        let frame = PhysicalFrame::allocate().unwrap();
        
        // Act
        unsafe {
            page_table.map_page(VirtualAddress::new(0x1000).unwrap(), frame, PageFlags::WRITABLE).unwrap();
        }
        
        // Assert
        assert!(page_table.is_mapped(VirtualAddress::new(0x1000).unwrap()));
    }
}
```

### Integration Tests

- Place in `tests/` directory
- Test public API
- Test error cases
- Test edge cases

### Test Organization

- One test per behavior
- Descriptive test names
- Arrange-Act-Assert pattern
- Test both success and failure cases

---

## Commit Message Guidelines

### Commit Message Format

```
Short summary (50 chars or less)

More detailed explanation if needed. Wrap at 72 characters.
Explain the what and why, not the how.

- Bullet points are okay
- Reference issues: Fixes #123
- Reference ADRs: Implements ADR-0001
```

### Commit Message Rules

- First line: Brief summary (imperative mood)
- Body: Explain what and why (if needed)
- Reference issues/PRs: `Fixes #123`, `Closes #456`
- Sign commits (DCO - Developer Certificate of Origin)

**Examples:**

```
Add physical frame allocator

Implements bitmap-based frame allocator as specified in ADR-0001.

- Uses bitmap for simplicity (as per ADR)
- NUMA-aware allocation (future enhancement)
- Unit tests included

Fixes #42
```

```
Fix page table entry validation

Page table entries were not properly validated before use, causing
undefined behavior in some edge cases.

- Add bounds checking
- Validate entry format
- Add test cases

Fixes #78
```

---

## Linting and Static Analysis

### Clippy

**Run clippy on all code:**

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

**Common clippy rules:**
- Enable all lints
- Warnings are errors in CI
- Fix clippy suggestions (unless explicitly overridden)

### Disabling Lints

**Use sparingly and document why:**

```rust
#[allow(clippy::too_many_arguments)]
// Allow here because kernel APIs often have many parameters
// for explicit control (design decision, not oversight)
fn complex_syscall(a: u64, b: u64, c: u64, d: u64, e: u64, f: u64, g: u64) {
    // Implementation
}
```

---

## Performance Considerations

### Optimization Philosophy

1. **Correctness first** - Correct code is more important than fast code
2. **Measure before optimizing** - Profile to find bottlenecks
3. **Optimize hot paths** - Focus optimization efforts where they matter
4. **Avoid premature optimization** - Write clear code first

### Performance Guidelines

- Use appropriate data structures
- Avoid unnecessary allocations in hot paths
- Consider cache locality
- Profile before optimizing
- Document performance-critical code

---

## no_std Considerations

### Core Library Only

- Use `core` instead of `std`
- Use `alloc` for collections (when available)
- No standard library dependencies
- Custom implementations for missing functionality

### Common Patterns

```rust
// Use core:: instead of std::
use core::fmt;
use core::ptr;
use alloc::vec::Vec;  // When alloc feature is enabled
```

---

## Code Review Checklist

When reviewing code, check:

- [ ] Code follows formatting standards (`cargo fmt`)
- [ ] No clippy warnings (`cargo clippy`)
- [ ] Public APIs are documented
- [ ] Error handling is appropriate
- [ ] Tests are included for new functionality
- [ ] Commit messages follow guidelines
- [ ] Unsafe code has safety comments (see UNSAFE_GUIDELINES.md)
- [ ] Code is clear and readable
- [ ] No unnecessary complexity

---

## References

- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) - Official Rust API guidelines
- [The Rust Book](https://doc.rust-lang.org/book/) - Learn Rust
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/) - Rust examples
- [Ferrous Kernel UNSAFE_GUIDELINES.md](UNSAFE_GUIDELINES.md)
- [Ferrous Kernel CONTRIBUTING.md](CONTRIBUTING.md)

---

**Last Updated:** 2026-01-04

