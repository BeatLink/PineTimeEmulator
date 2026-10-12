// SPDX-License-Identifier: GPL-3.0-only
//! Interpreter speed on a tight loop; run with `cargo test --release -- --ignored --nocapture`.

use std::time::Instant;

use super::support::Machine;
use crate::StopReason;

/// Runs the program to its BKPT five times from reset and returns the fastest time in seconds.
fn best_of_five(machine: &mut Machine) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..5 {
        machine.cpu.reset(&mut machine.bus);
        let start = Instant::now();
        assert_eq!(machine.run(), StopReason::Breakpoint);
        best = best.min(start.elapsed().as_secs_f64());
    }
    best
}

#[test]
#[ignore]
fn instructions_per_second() {
    let mut machine = Machine::new(
        "ldr r1, =2000000\n ldr r5, =0x20000100\n movs r0, #0\nloop:\n adds r0, r0, #3\n eor r2, r0, r1, lsl #2\n\
         str r2, [r5]\n ldr r3, [r5]\n add r4, r4, r3\n subs r1, r1, #1\n bne loop\n bkpt #0",
    );
    let elapsed = best_of_five(&mut machine);
    let instructions = machine.cpu.executed_instructions() / 5;
    println!(
        "{instructions} instructions in {elapsed:.3} s: {:.1} M instructions/s, {:.1} M cycles/s",
        instructions as f64 / elapsed / 1e6,
        machine.cpu.cycles() as f64 / 5.0 / elapsed / 1e6
    );
    assert_eq!(machine.register(0), 6_000_000);
}

#[test]
#[ignore]
fn two_instruction_loop_speed() {
    let mut machine = Machine::new("ldr r1, =5000000\nloop:\n subs r1, r1, #1\n bne loop\n bkpt #0");
    let elapsed = best_of_five(&mut machine);
    let instructions = machine.cpu.executed_instructions() / 5;
    println!(
        "two-instruction loop: {:.1} M instructions/s",
        instructions as f64 / elapsed / 1e6
    );
}
