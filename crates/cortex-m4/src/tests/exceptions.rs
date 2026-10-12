// SPDX-License-Identifier: GPL-3.0-only
//! Exception entry and return, priorities and masking, tail-chaining, faults and lockup.

use super::support::{run, Machine};
use crate::StopReason;

/// Thread code that enables IRQ 0 and 1 with priorities 0x80 and 0x40.
const ENABLE_TWO_INTERRUPTS: &str = "ldr r0, =0xE000E100\n movs r1, #3\n str r1, [r0]\n\
     ldr r0, =0xE000E400\n ldr r1, =0x4080\n str r1, [r0]\n";

// ============================================================================
// Frames and EXC_RETURN
// ============================================================================

#[test]
fn supervisor_call_stacks_a_basic_frame_and_returns() {
    let mut machine = Machine::new(
        "movs r0, #10\n movs r1, #11\n movs r2, #12\n movs r3, #13\n mov r12, r3\n ldr r4, =0x1235\n mov lr, r4\n\
         svc #5\nafter:\n adr r7, after\n bkpt #0\n\
         vector_11:\n mov r8, sp\n mov r9, lr\n mrs r10, ipsr\n movs r0, #99\n bx lr",
    );
    machine.run_to_breakpoint();
    let frame = machine.register(8);
    assert_eq!(frame, 0x2001_0000 - 0x20);
    let words: Vec<u32> = (0..8)
        .map(|index| machine.bus.read_word(frame + 4 * index))
        .collect();
    assert_eq!(&words[..6], &[10, 11, 12, 13, 13, 0x1235]);
    assert_eq!(words[6], machine.register(7) & !1);
    assert_eq!(words[7], 1 << 24);
    assert_eq!(machine.register(9), 0xFFFF_FFF9);
    assert_eq!(machine.register(10), 11);
    assert_eq!(machine.register(0), 10, "r0 comes back from the frame");
    assert_eq!(machine.cpu.sp(), 0x2001_0000);
    assert_eq!(machine.cpu.xpsr() & 0x1FF, 0);
}

#[test]
fn entry_realigns_the_stack_to_eight_bytes() {
    let machine = run("sub sp, #4\n svc #0\n bkpt #0\nvector_11:\n mov r8, sp\n bx lr");
    assert_eq!(machine.register(8), 0x2001_0000 - 4 - 4 - 0x20);
    assert_eq!(machine.cpu.sp(), 0x2001_0000 - 4);
}

#[test]
fn thread_mode_on_the_process_stack() {
    let machine = run(
        "ldr r0, =0x20008000\n msr psp, r0\n movs r0, #2\n msr control, r0\n isb\n svc #0\n\
         mrs r11, control\n bkpt #0\n\
         vector_11:\n mov r9, lr\n mrs r10, psp\n mov r8, sp\n bx lr",
    );
    assert_eq!(machine.register(9), 0xFFFF_FFFD);
    assert_eq!(machine.register(10), 0x2000_8000 - 0x20);
    assert_eq!(machine.register(8), 0x2001_0000);
    assert_eq!(machine.cpu.sp(), 0x2000_8000);
    assert_eq!(machine.register(11), 2);
}

#[test]
fn floating_point_context_uses_an_extended_lazy_frame() {
    for lazy in [true, false] {
        let lazy_setup = if lazy {
            ""
        } else {
            "ldr r0, =0xE000EF34\n ldr r1, =0x80000000\n str r1, [r0]\n"
        };
        let machine = run(&format!(
            "ldr r0, =0xE000ED88\n ldr r1, =0x00F00000\n str r1, [r0]\n {lazy_setup}\
             vmov.f32 s0, #1.0\n mrs r6, control\n svc #0\n vmov r5, s0\n bkpt #0\n\
             vector_11:\n mov r9, lr\n mov r8, sp\n vmov.f32 s0, #2.0\n bx lr"
        ));
        assert_eq!(
            machine.register(6),
            4,
            "CONTROL.FPCA set by the first FP instruction"
        );
        assert_eq!(machine.register(9), 0xFFFF_FFE9);
        assert_eq!(machine.register(8), 0x2001_0000 - 0x68);
        assert_eq!(machine.register(5), 1.0f32.to_bits(), "lazy {lazy}");
        assert_eq!(machine.cpu.sp(), 0x2001_0000);
    }
}

#[test]
fn invalid_exception_return_raises_invpc() {
    let mut machine = Machine::new(
        "ldr r0, =0xE000ED24\n ldr r1, =0x00070000\n str r1, [r0]\n ldr r0, =0xE000ED1F\n movs r1, #0x80\n strb r1, [r0]\n\
         svc #0\n bkpt #1\n\
         vector_11:\n ldr r0, =0xFFFFFFF5\n bx r0\n\
         vector_6:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), 1 << 18);
}

// ============================================================================
// Interrupt lines, priorities and masking
// ============================================================================

#[test]
fn a_high_line_is_taken_and_pends_again_while_held() {
    let mut machine = Machine::new(&format!(
        "{ENABLE_TWO_INTERRUPTS} movs r4, #0\nwait:\n b wait\n\
         vector_16:\n adds r4, #1\n cmp r4, #3\n bne back\n bkpt #1\nback:\n bx lr"
    ));
    machine.bus.set_lines(1);
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 3);
    assert_eq!(machine.cpu.xpsr() & 0x1FF, 16);
}

#[test]
fn a_line_that_drops_is_taken_once() {
    let mut machine = Machine::new(&format!(
        "movs r4, #0\n {ENABLE_TWO_INTERRUPTS}wait:\n cmp r4, #0\n beq wait\n bkpt #0\n\
         vector_16:\n adds r4, #1\n bkpt #1\n bx lr"
    ));
    machine.bus.set_lines(1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    machine.bus.set_lines(0);
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 1);
    assert_eq!(machine.cpu.xpsr() & 0x1FF, 0);
}

#[test]
fn a_line_dropped_from_outside_the_run_is_taken_once() {
    let mut machine = Machine::new(&format!(
        "movs r4, #0\n {ENABLE_TWO_INTERRUPTS}wait:\n cmp r4, #0\n beq wait\n bkpt #0\n\
         vector_16:\n adds r4, #1\n bkpt #1\n bx lr"
    ));
    machine.cpu.set_interrupt_lines(1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    machine.cpu.set_interrupt_lines(0);
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 1);
    assert_eq!(machine.cpu.xpsr() & 0x1FF, 0);
}

#[test]
fn higher_priority_preempts_and_lower_waits() {
    let machine = run(&format!(
        "{ENABLE_TWO_INTERRUPTS} movs r5, #0\n ldr r0, =0xE000E200\n movs r1, #1\n str r1, [r0]\n bkpt #0\n\
         vector_16:\n ldr r0, =0xE000E200\n movs r1, #2\n str r1, [r0]\n movs r1, #10\n mul r5, r5, r1\n adds r5, #1\n bx lr\n\
         vector_17:\n movs r1, #10\n mul r5, r5, r1\n adds r5, #2\n bx lr"
    ));
    assert_eq!(
        machine.register(5),
        21,
        "IRQ 1 ran inside IRQ 0, before IRQ 0 finished"
    );
}

#[test]
fn pending_exceptions_tail_chain_without_unstacking() {
    let mut machine = Machine::new(&format!(
        "{ENABLE_TWO_INTERRUPTS} cpsid i\n ldr r0, =0xE000E200\n movs r1, #3\n str r1, [r0]\n\
         ldr r0, =0xE000E400\n ldr r1, =0x8040\n str r1, [r0]\n cpsie i\n bkpt #0\n\
         vector_16:\n bx lr\n\
         vector_17:\n bx lr"
    ));
    let mut seen_tail_chain = false;
    for _ in 0..100 {
        let before = machine.cpu.xpsr() & 0x1FF;
        let outcome = machine.cpu.step(&mut machine.bus);
        let after = machine.cpu.xpsr() & 0x1FF;
        if before == 16 && after == 17 {
            assert_eq!(outcome.cycles, crate::exceptions::TAIL_CHAIN_CYCLES as u64);
            assert_eq!(machine.register(14), 0xFFFF_FFF9);
            seen_tail_chain = true;
        }
        if outcome.reason == StopReason::Breakpoint {
            break;
        }
    }
    assert!(seen_tail_chain);
}

#[test]
fn primask_and_basepri_hold_off_interrupts() {
    let mut machine = Machine::new(&format!(
        "movs r4, #0\n {ENABLE_TWO_INTERRUPTS} cpsid i\n bkpt #1\n nop\n bkpt #2\n cpsie i\n bkpt #3\n\
         movs r0, #0x40\n msr basepri, r0\n bkpt #4\n nop\n bkpt #5\n movs r0, #0\n msr basepri, r0\n bkpt #6\n\
         vector_16:\n adds r4, #1\n bx lr\n\
         vector_17:\n adds r4, #10\n bx lr"
    ));
    assert_eq!(machine.run(), StopReason::Breakpoint);
    machine.cpu.set_interrupt_pending(0);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(machine.register(4), 0, "PRIMASK masks it");
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(machine.register(4), 1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    machine.cpu.set_interrupt_pending(0);
    machine.cpu.set_interrupt_pending(1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(
        machine.register(4),
        1,
        "BASEPRI 0x40 masks priorities 0x40 and 0x80"
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 12);
}

#[test]
fn pendsv_and_systick_pend_through_icsr() {
    let machine = run(
        "movs r4, #0\n ldr r0, =0xE000ED04\n ldr r1, =0x14000000\n str r1, [r0]\n bkpt #0\n\
         vector_14:\n adds r4, #1\n bx lr\n vector_15:\n adds r4, #10\n bx lr",
    );
    assert_eq!(machine.register(4), 11);
}

// ============================================================================
// Faults and lockup
// ============================================================================

#[test]
fn undefined_instruction_escalates_to_hard_fault_when_usage_fault_is_disabled() {
    let mut machine = Machine::new(
        "udf #0\n bkpt #1\n\
         vector_3:\n mrs r4, ipsr\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n ldr r0, =0xE000ED2C\n ldr r6, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 3);
    assert_eq!(machine.register(5), 1 << 16);
    assert_eq!(machine.register(6), 1 << 30);
    assert_eq!(machine.cpu.sp(), 0x2001_0000 - 0x20);
    assert_eq!(
        machine.bus.read_word(0x2001_0000 - 8),
        0x100,
        "stacked PC is the faulting UDF"
    );
}

#[test]
fn enabled_usage_fault_takes_its_own_handler() {
    let mut machine = Machine::new(
        "ldr r0, =0xE000ED24\n ldr r1, =0x00040000\n str r1, [r0]\n udf #0\n bkpt #1\n\
         vector_6:\n mrs r4, ipsr\n ldr r0, =0xE000ED24\n ldr r5, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 6);
    assert_eq!(machine.register(5), 0x0004_0008, "USGFAULTENA and USGFAULTACT");
}

#[test]
fn branch_to_an_even_address_raises_invalid_state() {
    let mut machine = Machine::new(
        "adr r0, target\n bx r0\n.balign 4\ntarget:\n nop\n bkpt #1\n\
         vector_3:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), 1 << 17);
}

#[test]
fn fault_inside_hard_fault_locks_up() {
    let mut machine = Machine::new("udf #0\nvector_3:\n udf #1");
    assert_eq!(machine.run(), StopReason::Lockup);
    let status = machine.cpu.fault_status();
    assert!(status.locked_up);
    assert_eq!(status.configurable_fault_status, 1 << 16);
    assert_eq!(machine.cpu.run(&mut machine.bus, 100).reason, StopReason::Lockup);
}

#[test]
fn floating_point_without_cpacr_raises_no_coprocessor() {
    let mut machine =
        Machine::new("vmov.f32 s0, #1.0\n bkpt #1\nvector_3:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n bkpt #2");
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), 1 << 19);
}

#[test]
fn supervisor_call_under_primask_escalates_to_hard_fault() {
    let machine = run("cpsid i\n svc #0\n bkpt #0\nvector_3:\n mrs r4, ipsr\n bkpt #1\nvector_11:\n bx lr");
    assert_eq!(
        machine.register(4),
        3,
        "PRIMASK raises execution priority to 0, SVCall's own level"
    );
    assert_eq!(machine.cpu.fault_status().hard_fault_status, 1 << 30);
}
