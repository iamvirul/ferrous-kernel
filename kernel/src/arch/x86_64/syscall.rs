//! System call transition mechanisms (SYSCALL / SYSRET / IRETQ).

use super::gdt::{KERNEL_CODE_SELECTOR, USER_CODE_32_SELECTOR};
use core::arch::global_asm;

const IA32_EFER: u32 = 0xC000_0080;
const IA32_STAR: u32 = 0xC000_0081;
const IA32_LSTAR: u32 = 0xC000_0082;
const IA32_FMASK: u32 = 0xC000_0084;

/// EFER.SCE — System Call Extensions. While clear, `SYSCALL`/`SYSRET` raise #UD.
const EFER_SCE: u64 = 1 << 0;

/// Initialize the MSRs required for the `SYSCALL` and `SYSRET` instructions.
///
/// # Safety
/// Must be called at CPL=0 during early kernel initialization.
pub unsafe fn init() {
    // 0. Enable SYSCALL/SYSRET. UEFI firmware leaves EFER.SCE clear; without
    //    it the first `syscall` from ring 3 faults with #UD. Read-modify-write
    //    preserves LME/LMA/NXE, which must not change while paging is on.
    wrmsr(IA32_EFER, rdmsr(IA32_EFER) | EFER_SCE);

    // 1. Configure STAR (Segment Selector Register)
    // Bits [47:32]: Kernel CS (used for SYSCALL). Kernel SS is implicitly CS + 8.
    // Bits [63:48]: User CS base (used for SYSRET).
    //   SYSRET sets CS to (User CS base + 16) for 64-bit returns.
    //   SYSRET sets SS to (User CS base + 8).
    let star: u64 = ((USER_CODE_32_SELECTOR as u64) << 48) | ((KERNEL_CODE_SELECTOR as u64) << 32);
    wrmsr(IA32_STAR, star);

    // 2. Configure LSTAR (Target RIP for 64-bit SYSCALL)
    let lstar = syscall_entry as usize as u64;
    wrmsr(IA32_LSTAR, lstar);

    // 3. Configure SFMASK (RFLAGS mask)
    // SYSCALL clears the bits in RFLAGS that are set in SFMASK.
    // We mask IF (bit 9) to disable interrupts upon entry.
    // We also mask TF (bit 8) and DF (bit 10).
    let fmask: u64 = 0x200 | 0x100 | 0x400;
    wrmsr(IA32_FMASK, fmask);
}

/// Read a Model Specific Register.
///
/// # Safety
/// Must be called at CPL=0 with `msr` naming an MSR the CPU implements,
/// otherwise `rdmsr` raises #GP.
#[inline]
unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    core::arch::asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags),
    );
    (u64::from(high) << 32) | u64::from(low)
}

/// Write to a Model Specific Register.
#[inline]
unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    core::arch::asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nostack, preserves_flags),
    );
}

// ---------------------------------------------------------------------------
// SYSCALL Entry Stub
// ---------------------------------------------------------------------------

global_asm!(
    r#"
    .global syscall_entry
    syscall_entry:
        // 1. SYSCALL has loaded RIP from LSTAR and cleared IF.
        //    Interrupts are safely disabled.
        //    User's original RIP is in RCX.
        //    User's original RFLAGS is in R11.
        
        // 2. Swap GS to access CpuLocal
        swapgs
        
        // 3. Switch to kernel stack
        // CpuLocal has `kernel_stack` at offset 0, and `user_rsp` at offset 8.
        mov gs:[8], rsp
        mov rsp, gs:[0]
        
        // 4. Save state necessary for SYSRET
        push gs:[8]      // Push user RSP
        push r11         // Push user RFLAGS
        push rcx         // Push user RIP
        
        // 5. Construct SyscallArgs on the stack for the Rust dispatcher
        // SyscallArgs: rdi, rsi, rdx, r10, r8, r9, rax
        push rax
        push r9
        push r8
        push r10
        push rdx
        push rsi
        push rdi
        
        // 6. Call the Rust dispatcher
        // First argument (RDI) is a pointer to the SyscallArgs struct on the stack.
        mov rdi, rsp
        
        // Align stack to 16 bytes before calling C ABI function.
        // (RSP is currently an odd multiple of 8, because we pushed 10 registers total?
        // Let's count: 3 (SYSRET state) + 7 (Args) = 10 pushes = 80 bytes.
        // Before pushes, RSP was whatever the kernel stack is (16-byte aligned).
        // So 80 bytes is a multiple of 16. The stack is 16-byte aligned!
        // We can just call.
        call syscall_dispatcher

        // 7. Validate the saved user RIP while still on the kernel stack.
        //    SYSRET with a non-canonical RCX raises #GP at CPL 0 *after* RSP
        //    is already the user stack (CVE-2012-0217 class). A user can
        //    trigger this by executing SYSCALL at the top of the lower half,
        //    so the next RIP is 0x0000_8000_0000_0000. User addresses are
        //    canonical iff bits 63:47 are all zero. RAX is free as scratch
        //    here; it is reloaded from the saved frame below.
        mov rax, [rsp + 56]
        shr rax, 47
        jnz 2f

        // 8. Restore every caller-saved register the dispatcher may have
        //    clobbered, so no kernel values leak to user space. Callee-saved
        //    registers (RBX, RBP, R12-R15) are preserved by the sysv64 ABI.
        //    RAX carries the dispatcher's result (SyscallArgs::rax).
        pop rdi
        pop rsi
        pop rdx
        pop r10
        pop r8
        pop r9
        pop rax

        // 9. Restore SYSRET state
        pop rcx          // user RIP (validated canonical above)
        pop r11          // user RFLAGS
        pop gs:[8]       // user RSP

        // 10. Switch back to user stack
        mov rsp, gs:[8]

        // 11. Swap GS back to user GS
        swapgs

        // 12. Return to user space
        sysretq

    2:
        // Non-canonical return RIP: never reach SYSRET. RSP is still the
        // 16-byte-aligned kernel stack from the dispatcher call above.
        mov rdi, [rsp + 56]
        call syscall_reject_noncanonical_return
        ud2
    "#
);

extern "C" {
    fn syscall_entry();
}

#[repr(C)]
#[derive(Debug)]
pub struct SyscallArgs {
    pub rdi: u64,
    pub rsi: u64,
    pub rdx: u64,
    pub r10: u64,
    pub r8: u64,
    pub r9: u64,
    pub rax: u64,
}

/// Called from `syscall_entry` with RDI = `&mut SyscallArgs`.
///
/// The ABI is pinned to `sysv64` rather than `extern "C"`: this crate is also
/// compiled for `x86_64-unknown-uefi` (linked into the boot image), where
/// `extern "C"` is Win64 and would read the argument from RCX — which holds
/// the user RIP at that point.
///
/// On return, `args.rax` is placed in the user's RAX (the syscall result);
/// every other argument register is restored to its value at entry.
///
/// This is the arch glue only: decoding, validation, and handlers live in
/// the platform-independent [`crate::syscall`] layer.
#[no_mangle]
extern "sysv64" fn syscall_dispatcher(args: &mut SyscallArgs) {
    debug_assert!(
        super::cpu::kernel_gs_active(),
        "SYSCALL entered ring 0 without the kernel GS (swapgs imbalance)"
    );
    let raw = [args.rdi, args.rsi, args.rdx, args.r10, args.r8, args.r9];
    // SAFETY: we are on the SYSCALL entry path: CPL 0, IF cleared by
    // SFMASK, kernel stack active (stub switched via gs:[0]), and CR3 still
    // holds the caller's address space (the stub never touches CR3).
    args.rax = unsafe { crate::syscall::dispatch(args.rax, raw) };
}

/// Called from `syscall_entry` when the saved user RIP is non-canonical.
///
/// SYSRET cannot safely return there (it would #GP in ring 0 on the user
/// stack). There is no task termination yet, so fail fast and visibly; once
/// process teardown exists (Phase 2.2+) this should kill the offending task
/// instead of the kernel.
#[no_mangle]
extern "sysv64" fn syscall_reject_noncanonical_return(user_rip: u64) -> ! {
    panic!("syscall: refusing SYSRET to non-canonical user RIP {user_rip:#x}");
}

// ---------------------------------------------------------------------------
// First Userspace Entry (IRETQ)
// ---------------------------------------------------------------------------

/// Jumps to user space to execute a new process.
///
/// Constructs a fake interrupt stack frame on the current stack and executes
/// `IRETQ` to drop privileges to Ring 3.
///
/// # Safety
/// The caller must ensure `rip` and `rsp` are valid virtual addresses in the
/// user address space, and that the user page tables are currently active.
pub unsafe fn jump_to_user_mode(rip: u64, rsp: u64) -> ! {
    let user_data = super::gdt::USER_DATA_SELECTOR;
    let user_code = super::gdt::USER_CODE_64_SELECTOR;
    let rflags = 0x202; // IF=1, Reserved bit 1 = 1

    core::arch::asm!(
        // Set up data segments for user space
        "mov ds, cx",
        "mov es, cx",

        // GS invariant (ADR-0009 §3a): in ring 0 GS.Base = CPU_LOCAL and
        // IA32_KERNEL_GS_BASE = user GS. Entering ring 3 must swap them, so
        // user code runs with its own GS and the next SYSCALL / IRQ entry
        // `swapgs` restores the kernel GS. Interrupts are masked here (the
        // iretq below re-enables them), so nothing observes the swapped
        // state in ring 0.
        "cli",
        "swapgs",

        // Push the IRET frame
        "push rcx",      // SS
        "push rdx",      // RSP
        "push r8",       // RFLAGS
        "push r9",       // CS
        "push rsi",      // RIP

        // Return to user space!
        "iretq",

        in("cx") user_data,
        in("dx") rsp,
        in("r8") rflags,
        in("r9") user_code,
        in("si") rip,
        options(noreturn)
    );
}
