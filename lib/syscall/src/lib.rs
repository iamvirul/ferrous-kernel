//! `ferrous-syscall`: the Ferrous system call ABI (ADR-0007).
//!
//! Shared by the kernel (which dispatches syscalls) and, later, user-space
//! runtimes (which issue them). Everything here is pure, `no_std`, and free
//! of `unsafe`, so the validation rules can be unit-tested on the host.
//!
//! # Register convention (x86-64)
//!
//! | Register | Meaning |
//! |----------|---------|
//! | RAX | syscall number on entry, result on return |
//! | RDI, RSI, RDX, R10, R8, R9 | arguments 1–6 |
//! | RCX, R11 | clobbered by the CPU (user RIP / RFLAGS) |
//!
//! # Result encoding
//!
//! Success values lie in `[0, i64::MAX]`. Errors are returned as the
//! two's-complement negation of a Linux-compatible errno, so the top 4095
//! values of `u64` are reserved for errors.

#![no_std]
#![warn(missing_docs)]

// `#[test]` needs the std harness on the host; the crate itself stays no_std.
#[cfg(test)]
extern crate std;

use core::marker::PhantomData;

/// Exclusive upper bound of the user half of the address space.
///
/// x86-64 4-level paging: user addresses are `[0, USER_SPACE_END)`. Every
/// address at or above this (the non-canonical hole and the kernel half) is
/// rejected by [`UserPtr::new`].
pub const USER_SPACE_END: u64 = 0x0000_8000_0000_0000;

/// Maximum number of bytes `sys_log` accepts in one call.
///
/// Bounds the kernel stack buffer, the page-table walk (≤ 2 pages), and the
/// handler's running time, with no heap allocation.
pub const LOG_MAX_LEN: usize = 512;

/// Largest value a successful syscall may return.
///
/// Values above this would be indistinguishable from encoded errors.
pub const MAX_SUCCESS_VALUE: u64 = i64::MAX as u64;

// ---------------------------------------------------------------------------
// Syscall numbers
// ---------------------------------------------------------------------------

/// System call numbers.
///
/// `0` is deliberately unassigned so a zeroed RAX never performs a real
/// syscall; it decodes to [`SyscallError::NoSys`] like any unknown number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallNumber {
    /// `sys_exit(code: i32) -> !`: terminate the calling process.
    Exit = 1,
    /// `sys_yield() -> 0`: voluntarily give up the CPU.
    Yield = 2,
    /// `sys_log(ptr: *const u8, len: u64) -> 0`: write UTF-8 text to the
    /// kernel log. Arguments are validated in every build; output is
    /// emitted only by debug kernels.
    Log = 3,
}

impl SyscallNumber {
    /// The raw number placed in RAX to invoke this syscall.
    pub const fn as_u64(self) -> u64 {
        self as u64
    }
}

impl TryFrom<u64> for SyscallNumber {
    type Error = SyscallError;

    fn try_from(raw: u64) -> Result<Self, Self::Error> {
        match raw {
            1 => Ok(Self::Exit),
            2 => Ok(Self::Yield),
            3 => Ok(Self::Log),
            _ => Err(SyscallError::NoSys),
        }
    }
}

// ---------------------------------------------------------------------------
// Errors and result encoding
// ---------------------------------------------------------------------------

/// Errors a syscall can return. Discriminants are Linux-compatible errnos.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u64)]
pub enum SyscallError {
    /// `EFAULT`: a pointer argument does not name readable user memory.
    Fault = 14,
    /// `EINVAL`: an argument is out of range or malformed.
    Invalid = 22,
    /// `ENOSYS`: unknown or reserved syscall number.
    NoSys = 38,
}

impl SyscallError {
    /// The positive errno value.
    pub const fn errno(self) -> u64 {
        self as u64
    }

    /// The value placed in RAX: `-(errno)` as `u64`.
    pub const fn to_return_value(self) -> u64 {
        0u64.wrapping_sub(self.errno())
    }
}

/// Result of a syscall handler before encoding into RAX.
pub type SyscallResult = Result<u64, SyscallError>;

/// Encode a handler result into the value returned in RAX.
///
/// # Panics (debug builds only)
///
/// If a handler returns a success value above [`MAX_SUCCESS_VALUE`]; that
/// is a kernel bug, because user space would decode it as an error.
pub fn encode_result(result: SyscallResult) -> u64 {
    match result {
        Ok(value) => {
            debug_assert!(
                value <= MAX_SUCCESS_VALUE,
                "syscall success value {value:#x} collides with the error range"
            );
            value
        }
        Err(error) => error.to_return_value(),
    }
}

// ---------------------------------------------------------------------------
// User pointers
// ---------------------------------------------------------------------------

/// A validated reference to `count` consecutive `T`s in user memory.
///
/// Construction checks only what can be checked without page tables:
/// overflow, bounds, and alignment. It does **not** prove the memory is
/// mapped; the kernel's `copy_from_user` does that before every access.
/// There is deliberately no way to dereference a `UserPtr` directly.
pub struct UserPtr<T> {
    addr: u64,
    count: usize,
    // `fn() -> T` keeps UserPtr Send/Sync/Copy regardless of `T`: it holds
    // an address, never a `T`.
    _marker: PhantomData<fn() -> T>,
}

impl<T> Clone for UserPtr<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T> Copy for UserPtr<T> {}

impl<T> core::fmt::Debug for UserPtr<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UserPtr")
            .field("addr", &format_args!("{:#x}", self.addr))
            .field("count", &self.count)
            .finish()
    }
}

impl<T> UserPtr<T> {
    /// Validate a user-supplied `(addr, count)` pair.
    ///
    /// # Errors
    ///
    /// [`SyscallError::Fault`] if any of these hold:
    /// - `count * size_of::<T>()` overflows, or `addr + byte_len` overflows;
    /// - `addr` is not below [`USER_SPACE_END`], or the range ends above it
    ///   (kernel or non-canonical);
    /// - `addr` is null and the range is non-empty;
    /// - `addr` is not aligned to `align_of::<T>()`.
    ///
    /// An empty range (`count == 0`, or a zero-sized `T`) at any user-half
    /// address is valid and is never dereferenced.
    pub fn new(addr: u64, count: u64) -> Result<Self, SyscallError> {
        let count = usize::try_from(count).map_err(|_| SyscallError::Fault)?;
        let byte_len = core::mem::size_of::<T>()
            .checked_mul(count)
            .and_then(|len| u64::try_from(len).ok())
            .ok_or(SyscallError::Fault)?;
        let end = addr.checked_add(byte_len).ok_or(SyscallError::Fault)?;

        // The start must itself be a user address. `end > USER_SPACE_END`
        // alone would admit an *empty* range starting at the exclusive bound.
        if addr >= USER_SPACE_END || end > USER_SPACE_END {
            return Err(SyscallError::Fault);
        }
        if byte_len != 0 && addr == 0 {
            return Err(SyscallError::Fault);
        }
        if !addr.is_multiple_of(core::mem::align_of::<T>() as u64) {
            return Err(SyscallError::Fault);
        }

        Ok(Self {
            addr,
            count,
            _marker: PhantomData,
        })
    }

    /// Start address in user space.
    pub const fn addr(self) -> u64 {
        self.addr
    }

    /// Number of `T` elements.
    pub const fn count(self) -> usize {
        self.count
    }

    /// Length of the range in bytes (cannot overflow: checked in `new`).
    pub const fn byte_len(self) -> usize {
        self.count * core::mem::size_of::<T>()
    }

    /// `true` if the range covers no bytes.
    pub const fn is_empty(self) -> bool {
        self.byte_len() == 0
    }
}

// ---------------------------------------------------------------------------
// sys_log argument rules
// ---------------------------------------------------------------------------

/// Validate `sys_log(ptr, len)` arguments before any memory is touched.
///
/// # Errors
///
/// - [`SyscallError::Invalid`] if `len > LOG_MAX_LEN` (checked first, so an
///   oversized request never triggers a page walk).
/// - [`SyscallError::Fault`] for any [`UserPtr::new`] failure.
pub fn validate_log_args(ptr: u64, len: u64) -> Result<UserPtr<u8>, SyscallError> {
    if len > LOG_MAX_LEN as u64 {
        return Err(SyscallError::Invalid);
    }
    UserPtr::new(ptr, len)
}

/// Interpret bytes copied from user space as log text.
///
/// # Errors
///
/// [`SyscallError::Invalid`] if the bytes are not valid UTF-8.
pub fn validate_log_text(bytes: &[u8]) -> Result<&str, SyscallError> {
    core::str::from_utf8(bytes).map_err(|_| SyscallError::Invalid)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // --- SyscallNumber ---

    #[test]
    fn known_numbers_decode() {
        assert_eq!(SyscallNumber::try_from(1), Ok(SyscallNumber::Exit));
        assert_eq!(SyscallNumber::try_from(2), Ok(SyscallNumber::Yield));
        assert_eq!(SyscallNumber::try_from(3), Ok(SyscallNumber::Log));
    }

    #[test]
    fn numbers_round_trip() {
        for nr in [
            SyscallNumber::Exit,
            SyscallNumber::Yield,
            SyscallNumber::Log,
        ] {
            assert_eq!(SyscallNumber::try_from(nr.as_u64()), Ok(nr));
        }
    }

    #[test]
    fn zero_is_reserved_and_returns_enosys() {
        assert_eq!(SyscallNumber::try_from(0), Err(SyscallError::NoSys));
    }

    #[test]
    fn unknown_numbers_return_enosys() {
        for raw in [4, 0xFFFF, u64::MAX] {
            assert_eq!(SyscallNumber::try_from(raw), Err(SyscallError::NoSys));
        }
    }

    // --- Result encoding ---

    #[test]
    fn errors_encode_as_negated_linux_errno() {
        assert_eq!(SyscallError::Fault.to_return_value() as i64, -14);
        assert_eq!(SyscallError::Invalid.to_return_value() as i64, -22);
        assert_eq!(SyscallError::NoSys.to_return_value() as i64, -38);
    }

    #[test]
    fn success_encodes_unchanged() {
        assert_eq!(encode_result(Ok(0)), 0);
        assert_eq!(encode_result(Ok(MAX_SUCCESS_VALUE)), MAX_SUCCESS_VALUE);
    }

    #[test]
    fn error_encodes_to_return_value() {
        assert_eq!(
            encode_result(Err(SyscallError::NoSys)),
            SyscallError::NoSys.to_return_value()
        );
    }

    #[test]
    #[should_panic(expected = "collides with the error range")]
    fn success_in_error_range_panics_in_debug() {
        encode_result(Ok(u64::MAX));
    }

    // --- UserPtr bounds ---

    #[test]
    fn valid_user_range_is_accepted() {
        let p = UserPtr::<u8>::new(0x1000_0000_0000, 16).unwrap();
        assert_eq!(p.addr(), 0x1000_0000_0000);
        assert_eq!(p.count(), 16);
        assert_eq!(p.byte_len(), 16);
        assert!(!p.is_empty());
    }

    #[test]
    fn range_ending_exactly_at_user_space_end_is_accepted() {
        assert!(UserPtr::<u8>::new(USER_SPACE_END - 8, 8).is_ok());
    }

    #[test]
    fn range_crossing_user_space_end_is_rejected() {
        assert_eq!(
            UserPtr::<u8>::new(USER_SPACE_END - 8, 9).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn kernel_half_address_is_rejected() {
        assert_eq!(
            UserPtr::<u8>::new(0xFFFF_8000_0000_0000, 1).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn non_canonical_address_is_rejected() {
        assert_eq!(
            UserPtr::<u8>::new(USER_SPACE_END, 1).unwrap_err(),
            SyscallError::Fault
        );
    }

    // --- UserPtr overflow ---

    #[test]
    fn address_plus_length_overflow_is_rejected() {
        assert_eq!(
            UserPtr::<u8>::new(u64::MAX, 2).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn element_count_times_size_overflow_is_rejected() {
        assert_eq!(
            UserPtr::<u64>::new(0x1000, u64::MAX / 4).unwrap_err(),
            SyscallError::Fault
        );
    }

    // --- UserPtr null / empty / alignment ---

    #[test]
    fn null_with_nonzero_length_is_rejected() {
        assert_eq!(UserPtr::<u8>::new(0, 1).unwrap_err(), SyscallError::Fault);
    }

    #[test]
    fn empty_range_is_accepted_even_at_null() {
        let p = UserPtr::<u8>::new(0, 0).unwrap();
        assert!(p.is_empty());
    }

    #[test]
    fn empty_range_at_user_space_end_is_rejected() {
        // USER_SPACE_END is the exclusive bound: it is not a user address,
        // even for a range that covers no bytes.
        assert_eq!(
            UserPtr::<u8>::new(USER_SPACE_END, 0).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn empty_range_in_kernel_half_is_still_rejected() {
        assert_eq!(
            UserPtr::<u8>::new(0xFFFF_8000_0000_0000, 0).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn misaligned_typed_pointer_is_rejected() {
        assert_eq!(
            UserPtr::<u64>::new(0x1001, 1).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn aligned_typed_pointer_reports_byte_length() {
        let p = UserPtr::<u64>::new(0x1008, 3).unwrap();
        assert_eq!(p.byte_len(), 24);
    }

    // --- sys_log rules ---

    #[test]
    fn log_at_max_length_is_accepted() {
        assert!(validate_log_args(0x1000, LOG_MAX_LEN as u64).is_ok());
    }

    #[test]
    fn log_above_max_length_is_invalid() {
        assert_eq!(
            validate_log_args(0x1000, LOG_MAX_LEN as u64 + 1).unwrap_err(),
            SyscallError::Invalid
        );
    }

    #[test]
    fn log_length_is_checked_before_pointer() {
        // Both arguments are bad; the cheap length check must win.
        assert_eq!(
            validate_log_args(0xFFFF_8000_0000_0000, u64::MAX).unwrap_err(),
            SyscallError::Invalid
        );
    }

    #[test]
    fn log_with_bad_pointer_faults() {
        assert_eq!(
            validate_log_args(u64::MAX, 2).unwrap_err(),
            SyscallError::Fault
        );
    }

    #[test]
    fn log_text_accepts_utf8() {
        assert_eq!(validate_log_text("héllo".as_bytes()), Ok("héllo"));
    }

    #[test]
    fn log_text_rejects_invalid_utf8() {
        assert_eq!(
            validate_log_text(&[0xC3, 0x28]).unwrap_err(),
            SyscallError::Invalid
        );
    }
}
