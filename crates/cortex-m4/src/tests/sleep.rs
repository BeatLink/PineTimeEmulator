// SPDX-License-Identifier: GPL-3.0-only
//! WFI, WFE and SEV, waking through the interrupt lines, and how `run` reports time and stops.

use super::support::Machine;
use crate::StopReason;

const ENABLE_INTERRUPT_ZERO: &str = "ldr r0, =0xE000E100\n movs r1, #1\n str r1, [r0]\n";

#[test]
fn wait_for_interrupt_sleeps_until_a_line_rises() {
    let mut machine = Machine::new(&format!(
        "{ENABLE_INTERRUPT_ZERO} movs r4, #0\n wfi\n bkpt #0\nvector_16:\n movs r4, #1\n bkpt #1\n bx lr"
    ));
    assert_eq!(machine.run(), StopReason::Sleeping);
    assert!(machine.cpu.is_sleeping());
    assert!(!machine.cpu.wake_reason_pending());
    let outcome = machine.cpu.run(&mut machine.bus, 1000);
    assert_eq!(
        outcome,
        crate::RunOutcome {
            cycles: 0,
            reason: StopReason::Sleeping
        }
    );

    machine.bus.set_lines(1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(machine.register(4), 1);
    assert!(!machine.cpu.is_sleeping());
}

#[test]
fn wait_for_interrupt_wakes_with_primask_set_without_taking_it() {
    let mut machine = Machine::new(&format!(
        "{ENABLE_INTERRUPT_ZERO} movs r4, #0\n cpsid i\n wfi\n movs r5, #7\n bkpt #0\nvector_16:\n movs r4, #1\n bx lr"
    ));
    assert_eq!(machine.run(), StopReason::Sleeping);
    machine.cpu.set_interrupt_pending(0);
    assert!(machine.cpu.wake_reason_pending());
    machine.run_to_breakpoint();
    assert_eq!((machine.register(4), machine.register(5)), (0, 7));
}

#[test]
fn wait_for_event_consumes_a_set_event_register() {
    let mut machine = Machine::new("sev\n wfe\n movs r4, #1\n wfe\n movs r4, #2\n bkpt #0");
    assert_eq!(machine.run(), StopReason::Sleeping);
    assert_eq!(machine.register(4), 1);
    assert!(!machine.cpu.wake_reason_pending());
    machine.cpu.send_event();
    assert!(machine.cpu.wake_reason_pending());
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 2);
}

#[test]
fn send_event_on_pending_wakes_wait_for_event_for_a_disabled_interrupt() {
    let mut machine =
        Machine::new("ldr r0, =0xE000ED10\n movs r1, #0x10\n str r1, [r0]\n wfe\n movs r4, #3\n bkpt #0");
    assert_eq!(machine.run(), StopReason::Sleeping);
    machine.bus.set_lines(1 << 5);
    machine.run_to_breakpoint();
    assert_eq!(machine.register(4), 3);
}

#[test]
fn sleep_on_exit_sleeps_after_the_handler_returns() {
    let mut machine = Machine::new(&format!(
        "{ENABLE_INTERRUPT_ZERO} ldr r0, =0xE000ED10\n movs r1, #2\n str r1, [r0]\n\
         ldr r0, =0xE000E200\n movs r1, #1\n str r1, [r0]\n movs r4, #9\n bkpt #0\nvector_16:\n movs r4, #1\n bx lr"
    ));
    assert_eq!(machine.run(), StopReason::Sleeping);
    assert_eq!(machine.register(4), 1);
}

#[test]
fn run_tells_the_bus_every_cycle_and_honours_stop_requests() {
    let mut machine = Machine::new("loop:\n adds r0, #1\n b loop");
    let outcome = machine.cpu.run(&mut machine.bus, 1000);
    assert_eq!(outcome.reason, StopReason::Deadline);
    assert!(outcome.cycles >= 1000 && outcome.cycles < 1003);
    assert_eq!(machine.bus.advanced_cycles, outcome.cycles);

    machine.bus.stop_at_cycle = Some(machine.bus.advanced_cycles + 30);
    let outcome = machine.cpu.run(&mut machine.bus, 1_000_000);
    assert_eq!(outcome.reason, StopReason::StopRequested);
    assert!(outcome.cycles >= 30 && outcome.cycles < 33);
}

#[test]
fn step_runs_one_instruction() {
    let mut machine = Machine::new("movs r0, #1\n movs r0, #2\n bkpt #0");
    let outcome = machine.cpu.step(&mut machine.bus);
    assert_eq!(
        outcome,
        crate::RunOutcome {
            cycles: 1,
            reason: StopReason::Deadline
        }
    );
    assert_eq!(machine.register(0), 1);
    assert_eq!(machine.cpu.pc(), 0x102);
    assert_eq!(machine.cpu.executed_instructions(), 1);
}

#[test]
fn breakpoints_stop_before_the_instruction() {
    let mut machine = Machine::new("movs r0, #1\n movs r0, #2\n movs r0, #3\n bkpt #0");
    machine.cpu.set_breakpoint(0x102);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!((machine.cpu.pc(), machine.register(0)), (0x102, 1));
    machine.cpu.clear_breakpoint(0x102);
    machine.run_to_breakpoint();
    assert_eq!(machine.register(0), 3);
}
