// SPDX-License-Identifier: GPL-3.0-only
//! The private peripheral bus at 0xE0000000..0xE0100000, which the core answers itself.
//!
//! Byte and halfword accesses pick lanes of a word; unmodelled registers read zero.

use pinetime_core::AccessSize;

use crate::cpu::CortexM4;

// ============================================================================
// Address map
// ============================================================================

pub const START: u32 = 0xE000_0000;
pub const END: u32 = 0xE010_0000;

const DATA_WATCHPOINT: u32 = 0xE000_1000;
const SYSTEM_TICK: u32 = 0xE000_E010;
const NVIC: u32 = 0xE000_E100;
const SYSTEM_CONTROL: u32 = 0xE000_ED00;
const DEBUG_CONTROL: u32 = 0xE000_EDF0;
const SOFTWARE_TRIGGER: u32 = 0xE000_EF00;
const FLOATING_POINT_CONTROL: u32 = 0xE000_EF30;

/// ICTR: up to 64 external interrupts are implemented.
const INTERRUPT_CONTROLLER_TYPE: u32 = 1;

fn lane_mask(address: u32, size: AccessSize) -> u32 {
    size.mask() << ((address & 3) * 8)
}

impl CortexM4 {
    /// Whether an unprivileged access to the private bus at `address` is allowed.
    pub(crate) fn private_bus_allows(&self, address: u32) -> bool {
        self.is_privileged() || address & !3 == SOFTWARE_TRIGGER && self.scb.configuration_control & 2 != 0
    }

    /// Whether `address` is on the CPU's private peripheral bus rather than the chip's.
    pub fn is_private_bus_address(address: u32) -> bool {
        (START..END).contains(&address)
    }

    /// Reads a private bus register for a debugger, with no privilege check.
    pub fn read_private_bus_for_debugger(&mut self, address: u32, size: AccessSize) -> u32 {
        self.read_private_bus(address, size)
    }

    /// Writes a private bus register for a debugger, with no privilege check.
    pub fn write_private_bus_for_debugger(&mut self, address: u32, value: u32, size: AccessSize) {
        self.write_private_bus(address, value, size)
    }

    pub(crate) fn read_private_bus(&mut self, address: u32, size: AccessSize) -> u32 {
        let word_address = address & !3;
        let word = self.read_private_word(word_address);
        (word >> ((address & 3) * 8)) & size.mask()
    }

    pub(crate) fn write_private_bus(&mut self, address: u32, value: u32, size: AccessSize) {
        let word_address = address & !3;
        let shift = (address & 3) * 8;
        self.write_private_word(word_address, value << shift, lane_mask(address, size));
    }

    fn read_private_word(&mut self, address: u32) -> u32 {
        match address {
            DATA_WATCHPOINT..=0xE000_1FFF => self.read_data_watchpoint(address - DATA_WATCHPOINT),
            0xE000_E004 => INTERRUPT_CONTROLLER_TYPE,
            SYSTEM_TICK..=0xE000_E01F => self.read_system_tick(address - SYSTEM_TICK),
            NVIC..=0xE000_E4FF => self.read_nvic(address - NVIC),
            SYSTEM_CONTROL..=0xE000_ED8F => self.read_system_control(address - SYSTEM_CONTROL),
            DEBUG_CONTROL..=0xE000_EDFF => self.read_debug_control(address - DEBUG_CONTROL),
            FLOATING_POINT_CONTROL..=0xE000_EF47 => {
                self.read_floating_point_control(address - FLOATING_POINT_CONTROL)
            }
            _ => 0,
        }
    }

    fn write_private_word(&mut self, address: u32, value: u32, lanes: u32) {
        let full = lanes == 0xFFFF_FFFF;
        match address {
            DATA_WATCHPOINT..=0xE000_1FFF if full => {
                self.write_data_watchpoint(address - DATA_WATCHPOINT, value)
            }
            SYSTEM_TICK..=0xE000_E01F if full => self.write_system_tick(address - SYSTEM_TICK, value),
            NVIC..=0xE000_E4FF => self.write_nvic(address - NVIC, value, lanes),
            SYSTEM_CONTROL..=0xE000_ED8F => self.write_system_control(address - SYSTEM_CONTROL, value, lanes),
            DEBUG_CONTROL..=0xE000_EDFF if full => self.write_debug_control(address - DEBUG_CONTROL, value),
            SOFTWARE_TRIGGER => self.write_software_trigger(value & lanes),
            FLOATING_POINT_CONTROL..=0xE000_EF47 if full => {
                self.write_floating_point_control(address - FLOATING_POINT_CONTROL, value)
            }
            _ => {}
        }
    }
}
