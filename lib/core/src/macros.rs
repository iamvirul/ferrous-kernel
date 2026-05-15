//! Kernel assertion and debug macros (Phase 1.4.3).
//!
//! Provides two complementary layers:
//!
//! ## Always-active assertion macros
//!
//! | Macro | Equivalent to | Extra value |
//! |-------|--------------|-------------|
//! | [`kassert!`]    | `assert!`    | `[kassert]` prefix in panic message |
//! | [`kassert_eq!`] | `assert_eq!` | Shows both operand values and source expr |
//! | [`kassert_ne!`] | `assert_ne!` | Shows both operand values and source expr |
//!
//! ## Debug-only macros (`debug_assertions` off in release)
//!
//! | Macro | Equivalent to |
//! |-------|--------------|
//! | [`kdebug_assert!`]    | `debug_assert!`    |
//! | [`kdebug_assert_eq!`] | `debug_assert_eq!` |
//! | [`kdebug_assert_ne!`] | `debug_assert_ne!` |
//!
//! ## Reachability markers
//!
//! | Macro | Purpose |
//! |-------|---------|
//! | [`kunreachable!`]   | Mark logically impossible code paths |
//! | [`kunimplemented!`] | Mark stubs not yet written |
//! | [`ktodo!`]          | Alias for `kunimplemented!` |
//!
//! # Why custom macros?
//!
//! Rust's `core` library already provides `assert!`, `debug_assert!`, etc.
//! They work correctly in `no_std` and call into the crate's `#[panic_handler]`
//! on failure.  The `k`-prefixed variants add:
//!
//! - A `[kassert]` / `[kassert_eq]` / `[kunreachable]` prefix in the panic
//!   message so assertion failures are instantly identifiable in serial output,
//!   even among other log lines.
//! - Operand source expressions in `kassert_eq!` / `kassert_ne!` — the message
//!   says *which* variables were compared, not just their values.
//! - Uniform naming under `ferrous_core::` so kernel subsystems have a single
//!   import path regardless of which binary (`boot`, `kernel`) they're compiled
//!   into.
//!
//! # Panics and the panic handler
//!
//! All macros use [`core::panic!`] on failure.  The caller's `#[panic_handler]`
//! (enhanced in Phase 1.4.2) takes over from there: it prints a banner, source
//! location, the formatted message, and a stack trace to COM1.
//!
//! # Zero cost in release
//!
//! `kdebug_assert!`, `kdebug_assert_eq!`, and `kdebug_assert_ne!` expand to
//! nothing when `debug_assertions` is not set (the default for `--release`
//! builds).  All other macros remain active in release so critical invariants
//! are always checked.
//!
//! # Examples
//!
//! ```no_run
//! use ferrous_core::{kassert, kassert_eq, kassert_ne};
//! use ferrous_core::{kdebug_assert, kunreachable};
//!
//! // Basic assertion
//! kassert!(ptr as usize % 4096 == 0, "frame must be page-aligned");
//!
//! // Equality check with automatic value display on failure
//! kassert_eq!(entry & 1, 1u64, "PTE present bit must be set");
//!
//! // Debug-only — zero cost in release
//! kdebug_assert!(free_frames > 0);
//!
//! // Unreachable branch
//! match signal {
//!     0 => handle_zero(),
//!     1 => handle_one(),
//!     _ => kunreachable!("unexpected signal: {}", signal),
//! }
//! ```

// ---------------------------------------------------------------------------
// Always-active assertions
// ---------------------------------------------------------------------------

/// Kernel assertion macro.
///
/// Panics if `$cond` evaluates to `false`. The panic message is prefixed with
/// `[kassert FAILED]` so it stands out clearly in serial output.
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kassert;
/// kassert!(ptr != core::ptr::null_mut());
/// kassert!(size % 4096 == 0, "size must be page-aligned, got {}", size);
/// ```
#[macro_export]
macro_rules! kassert {
    ($cond:expr) => {{
        if !$cond {
            ::core::panic!("[kassert FAILED] {}", ::core::stringify!($cond));
        }
    }};
    ($cond:expr, $($arg:tt)+) => {{
        if !$cond {
            ::core::panic!("[kassert FAILED] {}", ::core::format_args!($($arg)+));
        }
    }};
}

/// Kernel equality assertion.
///
/// Panics if `$left != $right`, printing both the source expressions and their
/// evaluated values. Requires both sides to implement [`core::fmt::Debug`].
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kassert_eq;
/// kassert_eq!(cr3_readback, pml4_phys);
/// kassert_eq!(frame.start_address() % 4096, 0u64, "frame not page-aligned");
/// ```
#[macro_export]
macro_rules! kassert_eq {
    ($left:expr, $right:expr) => {{
        match (&$left, &$right) {
            (l, r) => {
                if l != r {
                    ::core::panic!(
                        "[kassert_eq FAILED] `{} == {}`\n  left:  {:?}\n  right: {:?}",
                        ::core::stringify!($left),
                        ::core::stringify!($right),
                        l,
                        r,
                    );
                }
            }
        }
    }};
    ($left:expr, $right:expr, $($arg:tt)+) => {{
        match (&$left, &$right) {
            (l, r) => {
                if l != r {
                    ::core::panic!(
                        "[kassert_eq FAILED] {} — `{} == {}`\n  left:  {:?}\n  right: {:?}",
                        ::core::format_args!($($arg)+),
                        ::core::stringify!($left),
                        ::core::stringify!($right),
                        l,
                        r,
                    );
                }
            }
        }
    }};
}

/// Kernel inequality assertion.
///
/// Panics if `$left == $right`, printing both the source expressions and their
/// evaluated values. Requires both sides to implement [`core::fmt::Debug`].
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kassert_ne;
/// kassert_ne!(frame_a, frame_b, "allocator returned duplicate frames");
/// ```
#[macro_export]
macro_rules! kassert_ne {
    ($left:expr, $right:expr) => {{
        match (&$left, &$right) {
            (l, r) => {
                if l == r {
                    ::core::panic!(
                        "[kassert_ne FAILED] `{} != {}`\n  both: {:?}",
                        ::core::stringify!($left),
                        ::core::stringify!($right),
                        l,
                    );
                }
            }
        }
    }};
    ($left:expr, $right:expr, $($arg:tt)+) => {{
        match (&$left, &$right) {
            (l, r) => {
                if l == r {
                    ::core::panic!(
                        "[kassert_ne FAILED] {} — `{} != {}`\n  both: {:?}",
                        ::core::format_args!($($arg)+),
                        ::core::stringify!($left),
                        ::core::stringify!($right),
                        l,
                    );
                }
            }
        }
    }};
}

// ---------------------------------------------------------------------------
// Debug-only assertions
// ---------------------------------------------------------------------------

/// Debug-only kernel assertion.
///
/// Expands to [`kassert!`] when `debug_assertions` is set (the default for
/// `cargo build` without `--release`). **Compiles to nothing** in release
/// builds — no code is generated, the condition is never evaluated.
///
/// Use for invariant checks that are expensive or would fire on edge cases in
/// production. Prefer [`kassert!`] for critical invariants that must hold
/// in all builds.
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kdebug_assert;
/// kdebug_assert!(free_frames > 0, "allocator must have free frames");
/// ```
#[macro_export]
macro_rules! kdebug_assert {
    ($($arg:tt)+) => {{
        #[cfg(debug_assertions)]
        $crate::kassert!($($arg)+);
    }};
}

/// Debug-only kernel equality assertion.
///
/// Expands to [`kassert_eq!`] in debug builds, **compiles to nothing** in
/// release. See [`kdebug_assert!`] for rationale.
#[macro_export]
macro_rules! kdebug_assert_eq {
    ($($arg:tt)+) => {{
        #[cfg(debug_assertions)]
        $crate::kassert_eq!($($arg)+);
    }};
}

/// Debug-only kernel inequality assertion.
///
/// Expands to [`kassert_ne!`] in debug builds, **compiles to nothing** in
/// release. See [`kdebug_assert!`] for rationale.
#[macro_export]
macro_rules! kdebug_assert_ne {
    ($($arg:tt)+) => {{
        #[cfg(debug_assertions)]
        $crate::kassert_ne!($($arg)+);
    }};
}

// ---------------------------------------------------------------------------
// Reachability markers
// ---------------------------------------------------------------------------

/// Mark a code path as logically unreachable.
///
/// Triggers a kernel panic (not undefined behaviour) with a clear message
/// identifying the call site. Unlike `core::unreachable!`, this macro never
/// calls `core::hint::unreachable_unchecked()` — even in release builds it
/// panics, because silent undefined behaviour in kernel code is unacceptable.
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kunreachable;
/// match irq_vector {
///     32..=47 => handle_irq(irq_vector),
///     _ => kunreachable!("unexpected interrupt vector: {:#x}", irq_vector),
/// }
/// ```
#[macro_export]
macro_rules! kunreachable {
    () => {{
        ::core::panic!(
            "[kunreachable] entered unreachable code at {}:{}:{}",
            ::core::file!(),
            ::core::line!(),
            ::core::column!(),
        );
    }};
    ($($arg:tt)+) => {{
        ::core::panic!("[kunreachable] {}", ::core::format_args!($($arg)+));
    }};
}

/// Mark a function or branch as not yet implemented.
///
/// Always panics with a `[kunimplemented]` prefix. Use during development to
/// mark stubs; replace with real implementations before shipping.
///
/// # Usage
///
/// ```no_run
/// # use ferrous_core::kunimplemented;
/// fn schedule_next_thread() -> ! {
///     kunimplemented!("scheduler not yet implemented");
/// }
/// ```
#[macro_export]
macro_rules! kunimplemented {
    () => {{
        ::core::panic!(
            "[kunimplemented] not yet implemented at {}:{}:{}",
            ::core::file!(),
            ::core::line!(),
            ::core::column!(),
        );
    }};
    ($($arg:tt)+) => {{
        ::core::panic!("[kunimplemented] {}", ::core::format_args!($($arg)+));
    }};
}

/// Alias for [`kunimplemented!`].
///
/// Marks work-in-progress code that must be completed before the feature is
/// usable. Behaves identically to [`kunimplemented!`] — always panics.
#[macro_export]
macro_rules! ktodo {
    () => {
        $crate::kunimplemented!()
    };
    ($($arg:tt)+) => {
        $crate::kunimplemented!($($arg)+)
    };
}
