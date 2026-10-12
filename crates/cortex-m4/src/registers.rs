// SPDX-License-Identifier: GPL-3.0-only
//! The program status register, CONTROL, the banked stack pointers and condition codes.
//! Ported from InfiniEmu's arm.h and cpu.c.

use crate::cpu::CortexM4;

// ============================================================================
// Flags
// ============================================================================

/// The APSR: condition flags, the sticky saturation flag and the SIMD greater-or-equal bits.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Flags {
    pub negative: bool,
    pub zero: bool,
    pub carry: bool,
    pub overflow: bool,
    pub saturated: bool,
    pub greater_or_equal: u8,
}

impl Flags {
    #[inline]
    pub fn set_negative_zero(&mut self, value: u32) {
        self.negative = (value as i32) < 0;
        self.zero = value == 0;
    }

    /// Whether condition code `condition` (0..13) holds; 14 and 15 always do.
    #[inline]
    pub fn condition_passed(&self, condition: u8) -> bool {
        let result = match condition >> 1 {
            0 => self.zero,
            1 => self.carry,
            2 => self.negative,
            3 => self.overflow,
            4 => self.carry && !self.zero,
            5 => self.negative == self.overflow,
            6 => self.negative == self.overflow && !self.zero,
            _ => return true,
        };
        if condition & 1 != 0 {
            !result
        } else {
            result
        }
    }

    pub fn to_bits(self) -> u32 {
        ((self.negative as u32) << 31)
            | ((self.zero as u32) << 30)
            | ((self.carry as u32) << 29)
            | ((self.overflow as u32) << 28)
            | ((self.saturated as u32) << 27)
            | ((self.greater_or_equal as u32 & 0xF) << 16)
    }

    pub fn set_from_bits(&mut self, value: u32, condition_flags: bool, greater_or_equal: bool) {
        if condition_flags {
            self.negative = value & (1 << 31) != 0;
            self.zero = value & (1 << 30) != 0;
            self.carry = value & (1 << 29) != 0;
            self.overflow = value & (1 << 28) != 0;
            self.saturated = value & (1 << 27) != 0;
        }
        if greater_or_equal {
            self.greater_or_equal = ((value >> 16) & 0xF) as u8;
        }
    }
}

// ============================================================================
// CONTROL
// ============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Control {
    /// nPRIV: thread mode runs unprivileged.
    pub unprivileged: bool,
    /// SPSEL: thread mode uses the process stack.
    pub process_stack: bool,
    /// FPCA: the floating-point context is live and exception entry must save it.
    pub floating_point_active: bool,
}

impl Control {
    pub fn to_bits(self) -> u32 {
        self.unprivileged as u32 | (self.process_stack as u32) << 1 | (self.floating_point_active as u32) << 2
    }
}

// ============================================================================
// Program status and stack pointers
// ============================================================================

pub(crate) const STACK_POINTER: usize = 13;
pub(crate) const LINK_REGISTER: usize = 14;
pub(crate) const PROGRAM_COUNTER: usize = 15;

impl CortexM4 {
    /// The whole xPSR: APSR, the IT and Thumb bits of the EPSR, and the IPSR.
    pub(crate) fn program_status(&self) -> u32 {
        self.flags.to_bits()
            | ((self.it_state as u32 & 0b11) << 25)
            | ((self.it_state as u32 >> 2) << 10)
            | ((self.thumb as u32) << 24)
            | self.exception_number as u32
    }

    /// Restores the xPSR from an exception frame.
    pub(crate) fn set_program_status(&mut self, value: u32) {
        self.flags.set_from_bits(value, true, true);
        self.it_state = (((value >> 25) & 0b11) | (((value >> 10) & 0x3F) << 2)) as u8;
        self.thumb = value & (1 << 24) != 0;
        self.exception_number = (value & 0x1FF) as u16;
    }

    #[inline]
    pub(crate) fn in_handler_mode(&self) -> bool {
        self.exception_number != 0
    }

    #[inline]
    pub(crate) fn is_privileged(&self) -> bool {
        self.in_handler_mode() || !self.control.unprivileged
    }

    /// Makes r13 the process stack pointer when thread mode selects it, otherwise the main one.
    pub(crate) fn select_stack_pointer(&mut self) {
        let use_process = self.control.process_stack && !self.in_handler_mode();
        if use_process != self.on_process_stack {
            std::mem::swap(&mut self.registers[STACK_POINTER], &mut self.other_stack_pointer);
            self.on_process_stack = use_process;
        }
    }

    pub(crate) fn main_stack_pointer(&self) -> u32 {
        if self.on_process_stack {
            self.other_stack_pointer
        } else {
            self.registers[STACK_POINTER]
        }
    }

    pub(crate) fn process_stack_pointer(&self) -> u32 {
        if self.on_process_stack {
            self.registers[STACK_POINTER]
        } else {
            self.other_stack_pointer
        }
    }

    pub(crate) fn set_main_stack_pointer(&mut self, value: u32) {
        if self.on_process_stack {
            self.other_stack_pointer = value & !3;
        } else {
            self.registers[STACK_POINTER] = value & !3;
        }
    }

    pub(crate) fn set_process_stack_pointer(&mut self, value: u32) {
        if self.on_process_stack {
            self.registers[STACK_POINTER] = value & !3;
        } else {
            self.other_stack_pointer = value & !3;
        }
    }

    /// Advances the IT state after an instruction inside an IT block.
    #[inline]
    pub(crate) fn advance_it_state(&mut self) {
        if self.it_state & 0b111 == 0 {
            self.it_state = 0;
        } else {
            self.it_state = (self.it_state & 0b1110_0000) | ((self.it_state << 1) & 0b1_1111);
        }
    }
}
