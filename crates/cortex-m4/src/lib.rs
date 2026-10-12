// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's Arm Cortex-M4F core: a Thumb-2 interpreter with its own decoder, the NVIC,
//! SCB, SysTick, DWT and FPU, and exception handling. Ported from InfiniEmu's cpu.c.

mod cpu;
mod debug;
pub mod decode;
mod exceptions;
mod execute;
mod fpu;
mod nvic;
mod private_bus;
mod registers;
mod scb;
mod systick;

#[cfg(test)]
mod tests;

pub use cpu::{CortexM4, FaultStatus, RunOutcome, StopReason};
pub use nvic::{EXTERNAL_INTERRUPT_COUNT, PRIORITY_BITS};
