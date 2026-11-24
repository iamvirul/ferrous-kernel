//! System call dispatch and handlers (Phase 2.1.5, ADR-0007).
//!
//! The arch entry stub (`arch::x86_64::syscall`) saves user state, switches
//! to the kernel stack, and calls [`dispatch`]. Its return value is placed
//! in the user's RAX. Numbers, errors, and pointer rules come from the
//! shared `ferrous-syscall` ABI crate.
//!
//! # Interim semantics (until the scheduler, Phase 2.2.2)
//!
//! - `sys_yield` calls the scheduler (Phase 2.2.2). Ring-3 code is not yet
//!   a scheduled task of its own, so today it always resumes immediately.
//! - `sys_exit` logs the exit code and halts: with a single task and no
//!   scheduler, nothing else can run.

pub mod user_memory;

use ferrous_syscall::{
    encode_result, validate_log_args, validate_log_text, SyscallNumber, SyscallResult, LOG_MAX_LEN,
};

use crate::arch::x86_64::cpu::halt_forever;

/// Raw syscall arguments in ABI order: RDI, RSI, RDX, R10, R8, R9.
pub type RawArgs = [u64; 6];

/// Decode `number`, run its handler, and encode the result for RAX.
///
/// Unknown and reserved numbers return `-ENOSYS`. No path allocates, and
/// every path finishes in time bounded by constants (see each handler).
///
/// # Safety
///
/// Must be called only from the SYSCALL entry path: CPL 0, interrupts
/// disabled, kernel stack active, and CR3 holding the caller's address
/// space (required by `copy_from_user`).
pub unsafe fn dispatch(number: u64, args: RawArgs) -> u64 {
    let result = match SyscallNumber::try_from(number) {
        Ok(SyscallNumber::Exit) => sys_exit(exit_code_from_arg(args[0])),
        Ok(SyscallNumber::Yield) => sys_yield(),
        Ok(SyscallNumber::Log) => sys_log(args[0], args[1]),
        Err(error) => Err(error),
    };
    if let Err(error) = result {
        log::debug!("syscall {number:#x} rejected: {error:?}");
    }
    encode_result(result)
}

/// `sys_exit` takes an `i32`: only the low 32 bits of RDI are meaningful,
/// matching the C ABI for a 32-bit argument (the upper half is undefined).
fn exit_code_from_arg(raw: u64) -> i32 {
    raw as u32 as i32
}

/// `sys_exit(code)`: terminate the calling process. Never returns.
///
/// Interim: there is no scheduler or process teardown yet, so after
/// recording the exit the CPU halts. Phase 2.2 replaces the halt with
/// "mark the process Exiting and schedule the next task".
fn sys_exit(code: i32) -> ! {
    log::info!("sys_exit: process exited with code {code}");
    halt_forever()
}

/// `sys_yield()`: give up the CPU (round-robin: back of the run queue).
/// Returns `0` once the caller runs again, or immediately if nothing else
/// is runnable or the scheduler is not initialised.
fn sys_yield() -> SyscallResult {
    crate::sched::scheduler::yield_now();
    Ok(0)
}

/// `sys_log(ptr, len)`: write UTF-8 text from user memory to the kernel log.
///
/// Arguments are fully validated in every build, so behaviour (including
/// every error) is identical in debug and release; only the log write is
/// compiled out of release kernels. Bounded: `len <= LOG_MAX_LEN` (checked
/// before any memory access), so at most 2 pages are walked and copied.
///
/// # Safety
///
/// Same as [`dispatch`].
unsafe fn sys_log(ptr: u64, len: u64) -> SyscallResult {
    let src = validate_log_args(ptr, len)?;
    let mut buf = [0u8; LOG_MAX_LEN];
    user_memory::copy_from_user(&mut buf, src)?;
    let text = validate_log_text(&buf[..src.byte_len()])?;

    // An empty write is a valid no-op; it emits nothing.
    #[cfg(debug_assertions)]
    if !text.is_empty() {
        log::info!(target: "user", "{text}");
    }
    #[cfg(not(debug_assertions))]
    let _ = text;

    Ok(0)
}
