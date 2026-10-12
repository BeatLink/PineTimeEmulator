// SPDX-License-Identifier: GPL-3.0-only
//! The private peripheral bus: NVIC, SCB, SysTick, DWT, DCB and the FPU context registers.

use super::support::{run, Machine};
use crate::StopReason;

// ============================================================================
// NVIC
// ============================================================================

#[test]
fn nvic_enable_pending_and_priority_registers() {
    let machine = run(
        "ldr r0, =0xE000E100\n ldr r1, =0x80000005\n str r1, [r0]\n movs r1, #3\n str r1, [r0, #4]\n\
         ldr r2, [r0]\n ldr r3, [r0, #4]\n\
         ldr r0, =0xE000E180\n movs r1, #1\n str r1, [r0]\n ldr r0, =0xE000E100\n ldr r4, [r0]\n\
         cpsid i\n ldr r0, =0xE000E200\n ldr r1, =0x40000000\n str r1, [r0]\n ldr r5, [r0]\n\
         ldr r0, =0xE000E280\n str r1, [r0]\n ldr r0, =0xE000E200\n ldr r6, [r0]\n\
         ldr r0, =0xE000E400\n movs r1, #0xFF\n strb r1, [r0, #5]\n ldr r7, [r0, #4]\n\
         ldr r0, =0xE000E004\n ldr r8, [r0]\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x8000_0005);
    assert_eq!(machine.register(3), 3);
    assert_eq!(machine.register(4), 0x8000_0004);
    assert_eq!(machine.register(5), 0x4000_0000);
    assert_eq!(machine.register(6), 0);
    assert_eq!(machine.register(7), 0xE000, "three priority bits");
    assert_eq!(machine.register(8), 1);
}

#[test]
fn active_bit_register_and_software_trigger() {
    let machine = run(
        "ldr r0, =0xE000E100\n movs r1, #4\n str r1, [r0]\n ldr r0, =0xE000EF00\n movs r1, #2\n str r1, [r0]\n bkpt #0\n\
         vector_18:\n ldr r0, =0xE000E300\n ldr r4, [r0]\n ldr r0, =0xE000ED04\n ldr r5, [r0]\n bx lr",
    );
    assert_eq!(machine.register(4), 4);
    assert_eq!(machine.register(5) & 0x1FF, 18);
    assert_ne!(machine.register(5) & (1 << 11), 0, "RETTOBASE");
}

#[test]
fn interrupt_pending_from_outside() {
    let mut machine = Machine::new("bkpt #0");
    machine.cpu.set_interrupt_pending(40);
    assert!(machine.cpu.is_interrupt_pending(40));
    assert!(!machine.cpu.wake_reason_pending(), "not enabled");
}

// ============================================================================
// SCB
// ============================================================================

#[test]
fn scb_identification_and_configuration() {
    let machine = run(
        "ldr r0, =0xE000ED00\n ldr r1, [r0]\n ldr r2, [r0, #0x0C]\n ldr r3, [r0, #0x14]\n\
         ldr r4, =0x00002000\n str r4, [r0, #8]\n ldr r5, [r0, #8]\n\
         movs r4, #0xA0\n strb r4, [r0, #0x23]\n ldr r6, [r0, #0x20]\n ldr r7, [r0, #0x88]\n bkpt #0",
    );
    assert_eq!(machine.register(1), 0x410F_C241);
    assert_eq!(machine.register(2), 0xFA05_0000);
    assert_eq!(machine.register(3), 0x200);
    assert_eq!(machine.register(5), 0x2000);
    assert_eq!(machine.cpu.vector_table_offset(), 0x2000);
    assert_eq!(machine.register(6), 0xA000_0000, "SysTick priority in SHPR3");
    assert_eq!(machine.register(7), 0);
}

#[test]
fn system_reset_request_needs_the_vector_key() {
    let mut machine = run(
        "ldr r0, =0xE000ED0C\n ldr r1, =0x00000004\n str r1, [r0]\n bkpt #1\n ldr r1, =0x05FA0304\n str r1, [r0]\n\
         ldr r2, [r0]\n bkpt #0",
    );
    assert!(!machine.cpu.take_system_reset_request());
    assert_eq!(machine.run(), StopReason::ResetRequested);
    assert!(machine.cpu.take_system_reset_request());
    assert!(!machine.cpu.take_system_reset_request());
    machine.run_to_breakpoint();
    assert_eq!(machine.register(2), 0xFA05_0300);
}

#[test]
fn priority_grouping_lets_only_group_priority_preempt() {
    let machine = run(
        "ldr r0, =0xE000ED0C\n ldr r1, =0x05FA0600\n str r1, [r0]\n\
         ldr r0, =0xE000E100\n movs r1, #3\n str r1, [r0]\n ldr r0, =0xE000E400\n ldr r1, =0x2040\n str r1, [r0]\n\
         movs r5, #0\n ldr r0, =0xE000E200\n movs r1, #1\n str r1, [r0]\n bkpt #0\n\
         vector_16:\n ldr r0, =0xE000E200\n movs r1, #2\n str r1, [r0]\n movs r5, #1\n bx lr\n\
         vector_17:\n lsls r5, r5, #4\n adds r5, #2\n bx lr",
    );
    assert_eq!(
        machine.register(5),
        0x12,
        "with PRIGROUP 6, 0x40 and 0x20 share group 0"
    );
}

// ============================================================================
// SysTick
// ============================================================================

#[test]
fn systick_counts_cycles_and_interrupts() {
    let mut machine = Machine::new(
        "movs r4, #0\n ldr r0, =0xE000E010\n movs r1, #99\n str r1, [r0, #4]\n str r1, [r0, #8]\n movs r1, #7\n str r1, [r0]\n\
         ldr r6, [r0, #8]\n nop\n nop\n ldr r7, [r0, #8]\n\
         wait:\n cmp r4, #3\n bne wait\n ldr r8, [r0]\n ldr r9, [r0]\n bkpt #0\n\
         vector_15:\n adds r4, #1\n bx lr",
    );
    let start = machine.cpu.cycles();
    machine.run_to_breakpoint();
    assert!(machine.register(6) > machine.register(7));
    assert!(machine.register(6) <= 99);
    let elapsed = machine.cpu.cycles() - start;
    assert!(
        (300..450).contains(&elapsed),
        "three periods of 100 cycles took {elapsed}"
    );
    assert_eq!(machine.register(8) & 0x10007, 0x10007, "COUNTFLAG set");
    assert_eq!(machine.register(9) & 0x10000, 0, "reading cleared COUNTFLAG");
}

#[test]
fn systick_calibration_and_disabled_counter_holds_its_value() {
    let machine = run(
        "ldr r0, =0xE000E010\n ldr r1, =1000\n str r1, [r0, #4]\n str r1, [r0, #8]\n movs r1, #5\n str r1, [r0]\n\
         nop\n nop\n movs r1, #4\n str r1, [r0]\n ldr r2, [r0, #8]\n nop\n nop\n nop\n ldr r3, [r0, #8]\n ldr r4, [r0, #12]\n bkpt #0",
    );
    assert_eq!(machine.register(2), machine.register(3));
    assert!(machine.register(2) < 1000 && machine.register(2) > 900);
    assert_eq!(machine.register(4), 0xC000_0000);
}

// ============================================================================
// DWT, DCB and FPU registers
// ============================================================================

#[test]
fn cycle_counter_needs_trace_enable_and_counts_cycles() {
    let machine = run(
        "ldr r0, =0xE0001000\n ldr r1, [r0]\n orr r1, #1\n str r1, [r0]\n ldr r2, [r0, #4]\n nop\n ldr r3, [r0, #4]\n\
         ldr r4, =0xE000EDFC\n ldr r5, =0x01000000\n str r5, [r4]\n movs r5, #0\n str r5, [r0, #4]\n\
         ldr r6, [r0, #4]\n nop\n nop\n ldr r7, [r0, #4]\n ldr r8, [r0]\n bkpt #0",
    );
    assert_eq!(machine.register(2), machine.register(3), "TRCENA off: frozen");
    assert_eq!(machine.register(7) - machine.register(6), 4, "ldr 2 + two nops");
    assert_eq!(machine.register(8), 0x4000_0001);
}

#[test]
fn fpu_feature_and_context_registers() {
    let machine = run(
        "ldr r0, =0xE000EF34\n ldr r1, [r0]\n ldr r2, [r0, #0xC]\n ldr r3, [r0, #0x10]\n\
         ldr r4, =0x03000000\n str r4, [r0, #8]\n ldr r5, [r0, #8]\n ldr r6, =0x20000101\n str r6, [r0, #4]\n ldr r7, [r0, #4]\n\
         ldr r0, =0xE000E000\n ldr r8, [r0]\n bkpt #0",
    );
    assert_eq!(machine.register(1), 0xC000_0000);
    assert_eq!(machine.register(2), 0x1011_0021);
    assert_eq!(machine.register(3), 0x1100_0011);
    assert_eq!(machine.register(5), 0x0300_0000);
    assert_eq!(machine.register(7), 0x2000_0100);
    assert_eq!(machine.register(8), 0, "unmodelled registers read zero");
}

#[test]
fn unprivileged_access_to_the_private_bus_faults() {
    let mut machine = Machine::new(
        "movs r0, #1\n msr control, r0\n isb\n ldr r0, =0xE000ED00\n ldr r1, [r0]\n bkpt #1\n\
         vector_3:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), (1 << 9) | (1 << 15));
}
