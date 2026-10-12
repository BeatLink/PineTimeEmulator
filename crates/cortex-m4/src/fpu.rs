// SPDX-License-Identifier: GPL-3.0-only
//! The FPv4-SP register file, FPSCR, and the context registers FPCCR, FPCAR and FPDSCR that
//! drive lazy stacking. Ported from InfiniEmu's scb_fp.c.

use pinetime_core::Bus;

use crate::cpu::CortexM4;
use crate::execute::{Fault, Trap};
use crate::nvic::{BUS_FAULT, DEBUG_MONITOR, MEMORY_MANAGEMENT_FAULT};

// ============================================================================
// Register bits
// ============================================================================

pub mod context_control {
    pub const LAZY_STATE_ACTIVE: u32 = 1 << 0;
    pub const USER: u32 = 1 << 1;
    pub const THREAD: u32 = 1 << 3;
    pub const HARD_FAULT_READY: u32 = 1 << 4;
    pub const MEMORY_MANAGE_READY: u32 = 1 << 5;
    pub const BUS_FAULT_READY: u32 = 1 << 6;
    pub const MONITOR_READY: u32 = 1 << 8;
    pub const LAZY_STACKING_ENABLE: u32 = 1 << 30;
    pub const AUTOMATIC_STATE_PRESERVATION: u32 = 1 << 31;
    pub const WRITABLE: u32 = 0xC000_017B;
}

pub const STATUS_WRITABLE: u32 = 0xF7C0_009F;
const DEFAULT_STATUS_WRITABLE: u32 = 0x07C0_0000;
const MEDIA_FEATURES_0: u32 = 0x1011_0021;
const MEDIA_FEATURES_1: u32 = 0x1100_0011;

// ============================================================================
// State
// ============================================================================

pub(crate) struct FloatingPoint {
    /// s0..s31, as raw bits; d<n> is s<2n> (low word) and s<2n+1>.
    pub registers: [u32; 32],
    pub status: u32,
    pub context_control: u32,
    pub context_address: u32,
    pub default_status: u32,
}

impl FloatingPoint {
    pub fn new() -> FloatingPoint {
        FloatingPoint {
            registers: [0; 32],
            status: 0,
            context_control: context_control::AUTOMATIC_STATE_PRESERVATION
                | context_control::LAZY_STACKING_ENABLE,
            context_address: 0,
            default_status: 0,
        }
    }

    pub fn lazy_state_active(&self) -> bool {
        self.context_control & context_control::LAZY_STATE_ACTIVE != 0
    }

    pub fn lazy_stacking_enabled(&self) -> bool {
        self.context_control & context_control::LAZY_STACKING_ENABLE != 0
    }
}

impl CortexM4 {
    // ========================================================================
    // Context registers
    // ========================================================================

    /// Reads the FPU context word at `offset` from 0xE000EF30.
    pub(crate) fn read_floating_point_control(&self, offset: u32) -> u32 {
        match offset {
            0x4 => self.fpu.context_control,
            0x8 => self.fpu.context_address,
            0xC => self.fpu.default_status,
            0x10 => MEDIA_FEATURES_0,
            0x14 => MEDIA_FEATURES_1,
            _ => 0,
        }
    }

    /// Writes the FPU context word at `offset` from 0xE000EF30.
    pub(crate) fn write_floating_point_control(&mut self, offset: u32, value: u32) {
        match offset {
            0x4 => self.fpu.context_control = value & context_control::WRITABLE,
            0x8 => self.fpu.context_address = value & !7,
            0xC => self.fpu.default_status = value & DEFAULT_STATUS_WRITABLE,
            _ => {}
        }
    }

    // ========================================================================
    // Lazy stacking
    // ========================================================================

    /// Records the frame space lazy stacking fills if the handler uses the FPU.
    pub(crate) fn reserve_lazy_floating_point_frame(&mut self, frame_address: u32) {
        use context_control::*;
        self.fpu.context_address = frame_address + 0x20;
        let execution_priority = self.exceptions.execution_priority;
        let can_take = |number: usize, cpu: &CortexM4| {
            cpu.exceptions.enabled & (1 << number) != 0
                && crate::nvic::group_priority(cpu.exceptions.priority(number), cpu.scb.priority_group)
                    < execution_priority
        };
        let mut control = self.fpu.context_control
            & !(USER | THREAD | HARD_FAULT_READY | MEMORY_MANAGE_READY | BUS_FAULT_READY | MONITOR_READY);
        control |= LAZY_STATE_ACTIVE;
        if !self.is_privileged() {
            control |= USER;
        }
        if !self.in_handler_mode() {
            control |= THREAD;
        }
        if -1 < execution_priority {
            control |= HARD_FAULT_READY;
        }
        if can_take(MEMORY_MANAGEMENT_FAULT, self) {
            control |= MEMORY_MANAGE_READY;
        }
        if can_take(BUS_FAULT, self) {
            control |= BUS_FAULT_READY;
        }
        if can_take(DEBUG_MONITOR, self) {
            control |= MONITOR_READY;
        }
        self.fpu.context_control = control;
    }

    /// Writes s0..s15 and FPSCR into the space a lazy exception frame reserved for them.
    pub(crate) fn preserve_floating_point_state<B: Bus>(&mut self, bus: &mut B) -> Result<(), Trap> {
        let address = self.fpu.context_address;
        for index in 0..16 {
            self.write_word(bus, address + 4 * index as u32, self.fpu.registers[index])?;
        }
        self.write_word(bus, address + 0x40, self.fpu.status)?;
        self.fpu.context_control &= !context_control::LAZY_STATE_ACTIVE;
        Ok(())
    }

    /// ExecuteFPCheck from the Arm manual: runs before every floating-point instruction.
    #[inline]
    pub(crate) fn check_floating_point<B: Bus>(&mut self, bus: &mut B) -> Result<(), Trap> {
        let access = self.scb.floating_point_access();
        if access == 0 || access == 1 && !self.is_privileged() || access == 2 {
            return Err(Trap::Fault(Fault::NoCoprocessor));
        }
        if self.fpu.lazy_state_active() {
            self.preserve_floating_point_state(bus)?;
        }
        if self.fpu.context_control & context_control::AUTOMATIC_STATE_PRESERVATION != 0
            && !self.control.floating_point_active
        {
            self.fpu.status = (self.fpu.status & !DEFAULT_STATUS_WRITABLE) | self.fpu.default_status;
            self.control.floating_point_active = true;
        }
        Ok(())
    }
}
