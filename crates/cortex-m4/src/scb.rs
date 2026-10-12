// SPDX-License-Identifier: GPL-3.0-only
//! The system control block: CPUID, ICSR, VTOR, AIRCR, SCR, CCR, SHPR1-3, SHCSR, the fault status
//! and address registers, and CPACR. Ported from InfiniEmu's scb.c.

use crate::cpu::CortexM4;
use crate::nvic::{
    BUS_FAULT, DEBUG_MONITOR, MEMORY_MANAGEMENT_FAULT, NON_MASKABLE_INTERRUPT, PENDABLE_SERVICE,
    SUPERVISOR_CALL, SYSTEM_TICK, USAGE_FAULT,
};

// ============================================================================
// Register bits
// ============================================================================

pub const CPUID: u32 = 0x410F_C241;

pub mod fault_status {
    pub const INSTRUCTION_ACCESS_VIOLATION: u32 = 1 << 0;
    pub const INSTRUCTION_BUS_ERROR: u32 = 1 << 8;
    pub const PRECISE_DATA_BUS_ERROR: u32 = 1 << 9;
    pub const UNSTACKING_BUS_ERROR: u32 = 1 << 11;
    pub const STACKING_BUS_ERROR: u32 = 1 << 12;
    pub const BUS_FAULT_ADDRESS_VALID: u32 = 1 << 15;
    pub const UNDEFINED_INSTRUCTION: u32 = 1 << 16;
    pub const INVALID_STATE: u32 = 1 << 17;
    pub const INVALID_PC: u32 = 1 << 18;
    pub const NO_COPROCESSOR: u32 = 1 << 19;
    pub const UNALIGNED: u32 = 1 << 24;
    pub const DIVIDE_BY_ZERO: u32 = 1 << 25;
}

pub mod hard_fault_status {
    pub const VECTOR_TABLE: u32 = 1 << 1;
    pub const FORCED: u32 = 1 << 30;
}

const CCR_UNALIGNED_TRAP: u32 = 1 << 3;
const CCR_DIVIDE_BY_ZERO_TRAP: u32 = 1 << 4;
const CCR_STACK_ALIGN: u32 = 1 << 9;
const CCR_NON_BASE_THREAD_ENABLE: u32 = 1 << 0;
const CCR_WRITABLE: u32 = 0x31B;

const SCR_SLEEP_ON_EXIT: u32 = 1 << 1;
const SCR_SEND_EVENT_ON_PENDING: u32 = 1 << 4;
const SCR_WRITABLE: u32 = 0x16;

const VECTOR_KEY: u32 = 0x05FA;
const AIRCR_SYSTEM_RESET_REQUEST: u32 = 1 << 2;
const AIRCR_CLEAR_ACTIVE: u32 = 1 << 1;

// ============================================================================
// State
// ============================================================================

pub(crate) struct SystemControl {
    pub vector_table_offset: u32,
    pub priority_group: u32,
    pub system_control: u32,
    pub configuration_control: u32,
    pub coprocessor_access: u32,
    pub configurable_fault_status: u32,
    pub hard_fault_status: u32,
    pub memory_manage_address: u32,
    pub bus_fault_address: u32,
    /// The enable bits of SHCSR (MEMFAULTENA, BUSFAULTENA, USGFAULTENA).
    pub fault_handler_enables: u32,
}

impl SystemControl {
    pub fn new(vector_table_offset: u32) -> SystemControl {
        SystemControl {
            vector_table_offset,
            priority_group: 0,
            system_control: 0,
            configuration_control: CCR_STACK_ALIGN,
            coprocessor_access: 0,
            configurable_fault_status: 0,
            hard_fault_status: 0,
            memory_manage_address: 0,
            bus_fault_address: 0,
            fault_handler_enables: 0,
        }
    }

    #[inline]
    pub fn send_event_on_pending(&self) -> bool {
        self.system_control & SCR_SEND_EVENT_ON_PENDING != 0
    }

    pub fn sleep_on_exit(&self) -> bool {
        self.system_control & SCR_SLEEP_ON_EXIT != 0
    }

    pub fn unaligned_trap(&self) -> bool {
        self.configuration_control & CCR_UNALIGNED_TRAP != 0
    }

    pub fn divide_by_zero_trap(&self) -> bool {
        self.configuration_control & CCR_DIVIDE_BY_ZERO_TRAP != 0
    }

    pub fn stack_align(&self) -> bool {
        self.configuration_control & CCR_STACK_ALIGN != 0
    }

    pub fn non_base_thread_enable(&self) -> bool {
        self.configuration_control & CCR_NON_BASE_THREAD_ENABLE != 0
    }

    /// The CP10 access field of CPACR: 0 denied, 1 privileged only, 3 full.
    pub fn floating_point_access(&self) -> u32 {
        (self.coprocessor_access >> 20) & 3
    }
}

/// The SHCSR active and pending bits, by exception number.
const HANDLER_ACTIVE_BITS: [(usize, u32); 7] = [
    (MEMORY_MANAGEMENT_FAULT, 0),
    (BUS_FAULT, 1),
    (USAGE_FAULT, 3),
    (SUPERVISOR_CALL, 7),
    (DEBUG_MONITOR, 8),
    (PENDABLE_SERVICE, 10),
    (SYSTEM_TICK, 11),
];
const HANDLER_PENDING_BITS: [(usize, u32); 4] = [
    (USAGE_FAULT, 12),
    (MEMORY_MANAGEMENT_FAULT, 13),
    (BUS_FAULT, 14),
    (SUPERVISOR_CALL, 15),
];
const FAULT_ENABLE_BITS: [(usize, u32); 3] =
    [(MEMORY_MANAGEMENT_FAULT, 16), (BUS_FAULT, 17), (USAGE_FAULT, 18)];

impl CortexM4 {
    // ========================================================================
    // SCB registers
    // ========================================================================

    /// Reads the SCB word at `offset` from 0xE000ED00.
    pub(crate) fn read_system_control(&self, offset: u32) -> u32 {
        let scb = &self.scb;
        match offset {
            0x00 => CPUID,
            0x04 => self.interrupt_control_state(),
            0x08 => scb.vector_table_offset,
            0x0C => (0xFA05 << 16) | (scb.priority_group << 8),
            0x10 => scb.system_control,
            0x14 => scb.configuration_control,
            0x18..=0x23 => {
                let first = 4 + (offset - 0x18) as usize;
                (0..4).fold(0, |word, lane| {
                    word | ((self.exceptions.priorities[first + lane] as u32) << (lane * 8))
                })
            }
            0x24 => self.handler_control_state(),
            0x28 => scb.configurable_fault_status,
            0x2C => scb.hard_fault_status,
            0x34 => scb.memory_manage_address,
            0x38 => scb.bus_fault_address,
            0x40 => 0x0000_0030,
            0x44 => 0x0000_0200,
            0x48 => 0x0010_0000,
            0x50 => 0x0010_0030,
            0x58 => 0x0100_0000,
            0x60 => 0x0110_1110,
            0x64 => 0x0211_2000,
            0x68 => 0x2123_2231,
            0x6C => 0x0111_1131,
            0x70 => 0x0131_0132,
            0x88 => scb.coprocessor_access,
            _ => 0,
        }
    }

    /// Writes the SCB word at `offset` from 0xE000ED00; `lanes` marks the bytes written.
    pub(crate) fn write_system_control(&mut self, offset: u32, value: u32, lanes: u32) {
        let merged = |old: u32| (old & !lanes) | (value & lanes);
        match offset {
            0x04 => self.write_interrupt_control_state(value & lanes),
            0x08 => self.scb.vector_table_offset = merged(self.scb.vector_table_offset) & 0xFFFF_FF80,
            0x0C if lanes == 0xFFFF_FFFF && value >> 16 == VECTOR_KEY => {
                self.scb.priority_group = (value >> 8) & 7;
                if value & AIRCR_SYSTEM_RESET_REQUEST != 0 {
                    self.system_reset_requested = true;
                }
                if value & AIRCR_CLEAR_ACTIVE != 0 {
                    self.exceptions.active = 0;
                }
                self.refresh_execution_priority();
            }
            0x10 => self.scb.system_control = merged(self.scb.system_control) & SCR_WRITABLE,
            0x14 => self.scb.configuration_control = merged(self.scb.configuration_control) & CCR_WRITABLE,
            0x18..=0x23 => {
                let first = 4 + (offset - 0x18) as usize;
                for lane in 0..4 {
                    if lanes & (0xFF << (lane * 8)) != 0 {
                        self.set_exception_priority(first + lane, (value >> (lane * 8)) as u8);
                    }
                }
            }
            0x24 => self.write_handler_control_state(merged(self.handler_control_state())),
            0x28 => self.scb.configurable_fault_status &= !(value & lanes),
            0x2C => self.scb.hard_fault_status &= !(value & lanes),
            0x34 => self.scb.memory_manage_address = merged(self.scb.memory_manage_address),
            0x38 => self.scb.bus_fault_address = merged(self.scb.bus_fault_address),
            0x88 => self.scb.coprocessor_access = merged(self.scb.coprocessor_access) & 0x00F0_0000,
            _ => {}
        }
    }

    // ========================================================================
    // ICSR and SHCSR
    // ========================================================================

    fn interrupt_control_state(&self) -> u32 {
        let pending = self.exceptions.pending;
        let mut value = self.exception_number as u32;
        if let Some((number, _)) = self.highest_pending_exception() {
            value |= (number as u32) << 12;
        }
        if pending >> crate::nvic::FIRST_EXTERNAL != 0 {
            value |= 1 << 22;
        }
        let other_active = self.exceptions.active & !(1u64 << self.exception_number);
        if self.exception_number != 0 && other_active == 0 {
            value |= 1 << 11;
        }
        if pending & (1 << SYSTEM_TICK) != 0 {
            value |= 1 << 26;
        }
        if pending & (1 << PENDABLE_SERVICE) != 0 {
            value |= 1 << 28;
        }
        if pending & (1 << NON_MASKABLE_INTERRUPT) != 0 {
            value |= 1 << 31;
        }
        value
    }

    fn write_interrupt_control_state(&mut self, value: u32) {
        if value & (1 << 31) != 0 {
            self.set_exception_pending(NON_MASKABLE_INTERRUPT);
        }
        if value & (1 << 28) != 0 {
            self.set_exception_pending(PENDABLE_SERVICE);
        } else if value & (1 << 27) != 0 {
            self.clear_exception_pending(PENDABLE_SERVICE);
        }
        if value & (1 << 26) != 0 {
            self.set_exception_pending(SYSTEM_TICK);
        } else if value & (1 << 25) != 0 {
            self.clear_exception_pending(SYSTEM_TICK);
        }
    }

    fn handler_control_state(&self) -> u32 {
        let mut value = self.scb.fault_handler_enables;
        for (number, position) in HANDLER_ACTIVE_BITS {
            if self.exceptions.active & (1 << number) != 0 {
                value |= 1 << position;
            }
        }
        for (number, position) in HANDLER_PENDING_BITS {
            if self.exceptions.pending & (1 << number) != 0 {
                value |= 1 << position;
            }
        }
        value
    }

    fn write_handler_control_state(&mut self, value: u32) {
        for (number, position) in HANDLER_ACTIVE_BITS {
            if value & (1 << position) != 0 {
                self.exceptions.active |= 1 << number;
            } else {
                self.exceptions.active &= !(1 << number);
            }
        }
        for (number, position) in HANDLER_PENDING_BITS {
            if value & (1 << position) != 0 {
                self.exceptions.pending |= 1 << number;
            } else {
                self.exceptions.pending &= !(1 << number);
            }
        }
        self.scb.fault_handler_enables = value & (0b111 << 16);
        for (number, position) in FAULT_ENABLE_BITS {
            if value & (1 << position) != 0 {
                self.exceptions.enabled |= 1 << number;
            } else {
                self.exceptions.enabled &= !(1 << number);
            }
        }
        self.refresh_execution_priority();
    }
}
