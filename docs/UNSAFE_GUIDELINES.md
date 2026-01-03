# Ferrous Kernel - Unsafe Rust Guidelines

**Version:** 0.1  
**Date:** 2026-01-04  
**Status:** Active Policy

---

## Overview

This document defines the policy and guidelines for using `unsafe` Rust code in Ferrous Kernel. Unsafe code is a necessary tool for systems programming, but it must be used judiciously, documented thoroughly, and reviewed carefully.

**Goal:** Unsafe code represents **<5% of total codebase**.

**Related Documents:**
- [CHARTER.md](CHARTER.md) - Design principles (unsafe code must be explicit, reviewed, isolated)
- [CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines
- [ARCHITECTURE.md](ARCHITECTURE.md) - System architecture overview

---

## Core Principles

1. **Unsafe code is explicit, reviewed, and isolated**
2. **Every unsafe block requires a safety comment**
3. **Unsafe code is hidden behind safe abstractions**
4. **Public APIs are safe**
5. **Unsafe code gets additional review**

---

## When Unsafe Code is Acceptable

Unsafe code is acceptable (and often necessary) for the following use cases:

### 1. Architecture-Specific Operations

**Examples:**
- CPU register access (MSRs, control registers)
- Page table manipulation
- Interrupt descriptor table (IDT) setup
- Global descriptor table (GDT) setup
- CPU feature detection and configuration

**Rationale:** These operations require direct hardware access that cannot be expressed in safe Rust.

### 2. Memory-Mapped I/O (MMIO) and Port I/O (PIO)

**Examples:**
- Device register access
- Hardware control registers
- I/O port access (x86_64)

**Rationale:** Hardware registers are accessed via raw pointers or assembly instructions.

### 3. Physical Memory Management

**Examples:**
- Physical frame allocation/deallocation
- Direct physical memory access
- Memory-mapped device access

**Rationale:** Physical addresses cannot be validated by Rust's type system.

### 4. Inline Assembly

**Examples:**
- CPU-specific instructions (e.g., `INVLPG`, `HLT`)
- Atomic operations not available in standard library
- Context switching code

**Rationale:** Some operations require assembly that cannot be expressed in Rust.

### 5. Performance-Critical Paths with Proven Safety

**Examples:**
- Zero-copy operations with verified safety
- Lock-free data structures with proven correctness
- Hot path optimizations with safety proofs

**Rationale:** Performance-critical code may require unsafe optimizations, but only when safety can be proven.

---

## When Unsafe Code is Unacceptable

Unsafe code is **not acceptable** for the following reasons:

### [BAD] Avoid Borrow Checker Issues

**Never use unsafe to work around borrow checker limitations.**

**Why:** If the borrow checker complains, it's usually indicating a design issue. Fix the design instead of using unsafe.

**Example (BAD):**
```rust
// [BAD] Using unsafe to avoid borrow checker
unsafe {
    let ptr = self.data.as_mut_ptr();
    // manipulate data through raw pointer
}
```

**Example (GOOD):**
```rust
// [GOOD] Restructure code to satisfy borrow checker
// Use interior mutability, split borrows, or refactor design
```

### [BAD] Transmuting Without Justification

**Never use `std::mem::transmute` or similar without careful justification and documentation.**

**Why:** Transmutation is extremely dangerous and often indicates a design flaw.

**Acceptable only when:**
- Converting between compatible types with identical memory layout
- Well-documented and reviewed
- No safe alternative exists

### [BAD] Raw Pointer Manipulation Without Clear Ownership

**Never manipulate raw pointers without clear ownership semantics.**

**Why:** Raw pointers bypass Rust's ownership system. Without clear ownership, safety cannot be guaranteed.

**Example (BAD):**
```rust
// [BAD] Unclear ownership
unsafe {
    let ptr: *mut T = get_pointer_from_somewhere();
    *ptr = value; // Who owns this? When is it freed?
}
```

**Example (GOOD):**
```rust
// [GOOD] Clear ownership via wrapper type
pub struct OwnedPointer<T> {
    ptr: *mut T,
    // ... ownership tracking ...
}

impl<T> OwnedPointer<T> {
    pub fn new(ptr: *mut T) -> Self {
        // Safety: caller guarantees ownership
        Self { ptr }
    }
}
```

### [BAD] Avoiding Proper Error Handling

**Never use unsafe to avoid proper error handling or validation.**

**Why:** Safety requires validation. Using unsafe to skip validation defeats the purpose.

---

## Safety Comment Requirements

**Every `unsafe` block, function, or trait implementation MUST have a safety comment.**

### Required Elements

A safety comment must explain:

1. **What invariants must hold** - What conditions must be true for this code to be safe
2. **Why they are guaranteed to hold** - How the caller/context ensures these invariants
3. **What could go wrong if violated** - Consequences of violating the invariants

### Safety Comment Format

```rust
/// Brief description of the unsafe operation.
///
/// # Safety
///
/// This function is unsafe because:
/// - Invariant 1: [Description of required condition]
///   - Guaranteed by: [How it's guaranteed]
///   - If violated: [Consequence]
/// - Invariant 2: [Description of required condition]
///   - Guaranteed by: [How it's guaranteed]
///   - If violated: [Consequence]
pub unsafe fn unsafe_function(...) { ... }
```

### Example: Safe Safety Comment

```rust
/// Map a virtual page to a physical frame in the page table.
///
/// # Safety
///
/// This function is unsafe because it directly manipulates page table entries.
///
/// **Required invariants:**
/// - `virt_addr` must be a valid virtual address in canonical form (x86_64)
///   - Guaranteed by: `VirtualAddress::new()` validates canonical form
///   - If violated: May cause page table corruption or undefined behavior
///
/// - `phys_frame` must be a valid, allocated physical frame
///   - Guaranteed by: `PhysicalFrame` type ensures validity
///   - If violated: May map to invalid or reused physical memory
///
/// - The virtual address must not already be mapped
///   - Guaranteed by: Caller must check `translate()` before calling
///   - If violated: Overwrites existing mapping, may leak frames
///
/// - The page table structure must be valid and properly initialized
///   - Guaranteed by: `PageTable` constructor ensures valid structure
///   - If violated: May cause page faults or memory corruption
pub unsafe fn map_page(
    &mut self,
    virt_addr: VirtualAddress,
    phys_frame: PhysicalFrame,
    flags: PageFlags,
) -> Result<(), MemoryError> {
    // Implementation...
}
```

### Example: Unsafe Block with Safety Comment

```rust
// Safety: We've verified that:
// 1. `ptr` is valid (checked bounds and alignment)
// 2. `ptr` points to initialized memory (checked initialization flag)
// 3. No other references exist (single mutable borrow of container)
// 4. The lifetime is valid (data lives as long as container)
//
// If any of these are violated, we get undefined behavior (use-after-free,
// double-free, or data race).
unsafe {
    let value = *ptr;
    // ... use value ...
}
```

---

## Safe Abstractions

**All unsafe code must be hidden behind safe public APIs.**

### Principle

- **Public APIs are safe** - Users of your API should not need to think about unsafe code
- **Unsafe is an implementation detail** - Unsafe code is internal to your module
- **Type system enforces safety** - Use Rust's type system to prevent misuse

### Example: Safe Wrapper Around Unsafe Code

```rust
// [BAD] Exposing unsafe API
pub unsafe fn map_page_raw(
    page_table: *mut PageTable,
    virt_addr: u64,
    phys_addr: u64,
) { ... }

// [GOOD] Safe wrapper
pub struct AddressSpace {
    page_table: PageTable,
    // ... other fields ...
}

impl AddressSpace {
    /// Map a virtual region to physical frames.
    ///
    /// Returns an error if:
    /// - The virtual address range is invalid
    /// - The region overlaps with existing mappings
    /// - Physical memory cannot be allocated
    pub fn map_region(
        &mut self,
        virt_start: VirtualAddress,
        size: usize,
        flags: RegionFlags,
    ) -> Result<VirtualRegion, MemoryError> {
        // Validate inputs (safe)
        self.validate_region(virt_start, size)?;
        
        // Allocate physical frames (safe API, unsafe internals)
        let frames = self.allocator.allocate_frames(size)?;
        
        // Map pages (safe wrapper around unsafe operation)
        unsafe {
            self.map_pages_internal(virt_start, frames, flags)?;
        }
        
        Ok(VirtualRegion::new(virt_start, size, frames))
    }
    
    // Internal unsafe function (not public)
    unsafe fn map_pages_internal(
        &mut self,
        virt_start: VirtualAddress,
        frames: Vec<PhysicalFrame>,
        flags: RegionFlags,
    ) -> Result<(), MemoryError> {
        // Safety: Caller guarantees valid inputs (checked in map_region)
        // Safety: Frames are valid (guaranteed by allocator)
        // Safety: Page table is valid (guaranteed by AddressSpace)
        
        // ... unsafe page table manipulation ...
    }
}
```

---

## Code Review Requirements

**All unsafe code requires additional review beyond standard code review.**

### Review Checklist for Unsafe Code

When reviewing unsafe code, verify:

- [ ] Safety comment is present and complete
- [ ] All required invariants are documented
- [ ] Invariants are guaranteed by the caller/context
- [ ] Consequences of violation are documented
- [ ] Unsafe code is hidden behind safe API (if public)
- [ ] Type system is used to prevent misuse
- [ ] No unsafe shortcuts (borrow checker, transmute, etc.)
- [ ] Error handling is appropriate
- [ ] Tests cover the unsafe code paths
- [ ] Edge cases are handled

### Review Process

1. **Initial Review**: Standard code review process
2. **Unsafe Code Review**: Additional review focused on safety
3. **Safety Verification**: Verify invariants are guaranteed
4. **Approval**: At least one reviewer must explicitly approve unsafe code

### Reviewers

- All reviewers can review unsafe code
- Unsafe code should be reviewed by someone familiar with the area
- Complex unsafe code may require multiple reviewers

---

## Testing Unsafe Code

**Unsafe code must be thoroughly tested.**

### Testing Requirements

1. **Unit Tests**: Test the unsafe code directly
2. **Integration Tests**: Test the safe API that wraps unsafe code
3. **Edge Cases**: Test boundary conditions and error cases
4. **Fuzzing** (future): Fuzz unsafe code paths when possible

### Example: Testing Unsafe Code

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_map_page_valid() {
        let mut page_table = PageTable::new().unwrap();
        let frame = PhysicalFrame::allocate().unwrap();
        let virt_addr = VirtualAddress::new(0x1000).unwrap();
        
        // Test safe API
        unsafe {
            page_table.map_page(virt_addr, frame, PageFlags::WRITABLE).unwrap();
        }
        
        // Verify mapping
        assert_eq!(
            page_table.translate(virt_addr),
            Some(frame.start_address())
        );
    }
    
    #[test]
    #[should_panic(expected = "already mapped")]
    fn test_map_page_duplicate() {
        // Test that mapping same address twice fails
        // ...
    }
    
    #[test]
    fn test_map_page_invalid_address() {
        // Test that invalid addresses are rejected
        // ...
    }
}
```

---

## Common Unsafe Patterns in Kernel Development

### 1. Raw Pointers for Hardware Access

```rust
/// MMIO register access
pub struct MmioRegister {
    ptr: *mut u32,
}

impl MmioRegister {
    pub fn new(addr: usize) -> Self {
        // Safety: Caller guarantees addr is valid MMIO address
        Self { ptr: addr as *mut u32 }
    }
    
    pub fn read(&self) -> u32 {
        // Safety: ptr is valid MMIO address (guaranteed by constructor)
        unsafe { ptr::read_volatile(self.ptr) }
    }
    
    pub fn write(&self, value: u32) {
        // Safety: ptr is valid MMIO address (guaranteed by constructor)
        unsafe { ptr::write_volatile(self.ptr, value) }
    }
}
```

### 2. Static Mutable State

```rust
use core::sync::atomic::{AtomicBool, Ordering};

static INITIALIZED: AtomicBool = AtomicBool::new(false);

pub fn initialize() {
    // Safety: Only called once during boot, before multi-threading
    if INITIALIZED.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok() {
        unsafe {
            // Initialization code
        }
    }
}
```

### 3. Unsafe Traits (GlobalAlloc)

```rust
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // Safety: Layout is valid (guaranteed by allocator API)
        // Safety: Returned pointer is valid for layout.size() bytes
        // Safety: Memory is uninitialized (caller must initialize)
        self.inner_alloc(layout)
    }
    
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // Safety: ptr was allocated by alloc() with same layout
        // Safety: ptr has not been deallocated already
        self.inner_dealloc(ptr, layout)
    }
}
```

---

## Metrics and Monitoring

### Tracking Unsafe Code

- **Goal**: Unsafe code <5% of total codebase
- **Measurement**: Count lines of code in `unsafe` blocks vs total lines
- **Review**: Regular audits of unsafe code usage

### Tools

- `cargo geiger` - Scans for unsafe code usage
- Manual review - Regular code reviews
- Metrics tracking - Track unsafe code percentage over time

---

## Security Considerations

### Unsafe Code and Security

Unsafe code can introduce security vulnerabilities:

- **Memory safety violations** - Use-after-free, double-free, buffer overflows
- **Information leaks** - Uninitialized memory, pointer leaks
- **Control flow hijacking** - Stack/heap corruption

### Mitigation Strategies

1. **Minimize unsafe code** - Use safe Rust when possible
2. **Isolate unsafe code** - Keep unsafe code in small, isolated modules
3. **Document thoroughly** - Safety comments prevent misuse
4. **Review carefully** - Multiple reviewers for unsafe code
5. **Test extensively** - Cover all unsafe code paths
6. **Audit regularly** - Regular security audits of unsafe code

---

## Examples: Good vs Bad

### Example 1: Page Table Access

```rust
// [BAD] Unsafe public API, no safety comment
pub unsafe fn set_page_entry(table: *mut PageTable, index: usize, entry: u64) {
    (*table).entries[index] = entry;
}

// [GOOD] Safe API with unsafe internals, documented safety
pub struct PageTable { /* ... */ }

impl PageTable {
    /// Set a page table entry.
    ///
    /// # Safety
    ///
    /// - `index` must be valid (0-511 for x86_64)
    ///   - Guaranteed by: Bounds checking in this function
    /// - `entry` must be a valid page table entry
    ///   - Guaranteed by: `PageTableEntry` type ensures validity
    pub fn set_entry(&mut self, index: usize, entry: PageTableEntry) -> Result<(), MemoryError> {
        if index >= self.entries.len() {
            return Err(MemoryError::InvalidIndex);
        }
        
        unsafe {
            // Safety: index is bounds-checked above
            // Safety: entry is valid (PageTableEntry type)
            self.entries[index] = entry.into();
        }
        
        Ok(())
    }
}
```

### Example 2: Memory Allocation

```rust
// [BAD] Using unsafe to avoid proper error handling
pub fn allocate(size: usize) -> *mut u8 {
    unsafe {
        // No error handling, no validation
        libc::malloc(size) as *mut u8
    }
}

// [GOOD] Safe wrapper with error handling
pub fn allocate(size: usize) -> Result<NonNull<u8>, AllocationError> {
    if size == 0 {
        return Err(AllocationError::ZeroSized);
    }
    
    unsafe {
        let ptr = libc::malloc(size);
        if ptr.is_null() {
            Err(AllocationError::OutOfMemory)
        } else {
            Ok(NonNull::new_unchecked(ptr))
        }
    }
}
```

---

## References

- [Rust Unsafe Code Guidelines](https://rust-lang.github.io/unsafe-code-guidelines/)
- [The Rustonomicon](https://doc.rust-lang.org/nomicon/) - The Dark Arts of Advanced and Unsafe Rust
- [Rust API Guidelines](https://rust-lang.github.io/api-guidelines/) - Section on safety
- [Ferrous Kernel CHARTER.md](CHARTER.md) - Design principles
- [Ferrous Kernel CONTRIBUTING.md](CONTRIBUTING.md) - Contribution guidelines

---

## Summary

**Key Takeaways:**

1. **Goal**: Unsafe code <5% of codebase
2. **Requirement**: Every unsafe block needs a safety comment
3. **Principle**: Hide unsafe code behind safe APIs
4. **Process**: Additional review for all unsafe code
5. **Testing**: Thoroughly test unsafe code paths
6. **Acceptable uses**: Hardware access, memory management, performance-critical paths
7. **Unacceptable uses**: Avoiding borrow checker, shortcuts, unclear ownership

**Remember:** Unsafe code is a tool, not a solution. Use it when necessary, document it thoroughly, and isolate it carefully.

---

**Document Status**: This is an active policy document. As we gain implementation experience, these guidelines will be refined and updated.

