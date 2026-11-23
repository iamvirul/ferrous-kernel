//! Copying data across the user/kernel boundary (ADR-0007).
//!
//! The only audited path by which the kernel reads user memory. Every
//! access is preceded by a page-table walk proving that each page of the
//! range is present **and** user-accessible, so the kernel never faults on a
//! user pointer and never reads supervisor-only memory on a user's behalf.

use ferrous_syscall::{SyscallError, UserPtr};

use crate::memory::address_space::{active_pml4_phys, is_user_readable_in};
use crate::memory::frame_allocator::PAGE_SIZE;
use crate::memory::paging::VirtualAddress;

/// Copy `src.byte_len()` bytes from user memory into the front of `dst`.
///
/// # Errors
///
/// - [`SyscallError::Invalid`] if `dst` is shorter than the source range
///   (a kernel caller bug, reported instead of truncating silently).
/// - [`SyscallError::Fault`] if any page of the range is unmapped or not
///   user-accessible in the active address space. Nothing is copied.
///
/// # Safety
///
/// - CR3 must hold the calling process's address space (true for the whole
///   syscall path, which never switches CR3).
/// - Single-threaded with interrupts disabled (SYSCALL masks IF), so the
///   mappings cannot change between the check and the copy.
/// - Identity mapping (VA == PA) must hold for page-table frames.
pub unsafe fn copy_from_user(dst: &mut [u8], src: UserPtr<u8>) -> Result<(), SyscallError> {
    let len = src.byte_len();
    let dst = dst.get_mut(..len).ok_or(SyscallError::Invalid)?;
    if src.is_empty() {
        return Ok(());
    }

    ensure_user_readable(src.addr(), len)?;

    // SAFETY:
    // - `ensure_user_readable` proved every page in [addr, addr + len) is
    //   PRESENT | USER at all levels of the active page tables, so the reads
    //   cannot fault and touch only memory the caller can already read.
    // - `UserPtr::new` proved the range does not overflow and lies below
    //   USER_SPACE_END, so it cannot alias kernel memory or `dst` (a kernel
    //   stack buffer, which is supervisor-only and was rejected above).
    // - Interrupts are disabled and the kernel is single-threaded, so no
    //   mapping changes between the walk and the copy.
    // - SMAP is not enabled; when it is, this copy must be bracketed by
    //   `stac` / `clac`.
    core::ptr::copy_nonoverlapping(src.addr() as *const u8, dst.as_mut_ptr(), len);
    Ok(())
}

/// Verify that every page overlapping `[addr, addr + len)` is user-readable.
///
/// The caller guarantees `len > 0` and that the range was validated by
/// `UserPtr::new` (no overflow, below `USER_SPACE_END`), which also makes
/// every address canonical. The loop runs once per page, so its bound is
/// `len / PAGE_SIZE + 2`, which is bounded by the caller's own length cap.
///
/// # Safety
///
/// Same as [`copy_from_user`].
unsafe fn ensure_user_readable(addr: u64, len: usize) -> Result<(), SyscallError> {
    let pml4 = active_pml4_phys();
    let last = addr + (len as u64 - 1);
    let mut page = addr & !(PAGE_SIZE - 1);
    while page <= last {
        let virt = VirtualAddress::try_new(page).ok_or(SyscallError::Fault)?;
        if !is_user_readable_in(pml4, virt) {
            return Err(SyscallError::Fault);
        }
        page += PAGE_SIZE;
    }
    Ok(())
}
