// SPDX-License-Identifier: GPL-3.0-only
//! Exception entry and return, tail-chaining, fault escalation and lockup.
//! Ported from InfiniEmu's cpu.c and the Arm manual's PushStack and ExceptionReturn.

use pinetime_core::Bus;

use crate::cpu::{CortexM4, Sleep};
use crate::execute::{Fault, Trap};
use crate::nvic::{
    group_priority, BUS_FAULT, HARD_FAULT, MEMORY_MANAGEMENT_FAULT, NON_MASKABLE_INTERRUPT, SUPERVISOR_CALL,
    USAGE_FAULT,
};
use crate::registers::{LINK_REGISTER, STACK_POINTER};
use crate::scb::{fault_status, hard_fault_status};

// ============================================================================
// Costs
// ============================================================================

pub(crate) const ENTRY_CYCLES: u32 = 12;
pub(crate) const RETURN_CYCLES: u32 = 10;
pub(crate) const TAIL_CHAIN_CYCLES: u32 = 6;

const BASIC_FRAME_SIZE: u32 = 0x20;
const EXTENDED_FRAME_SIZE: u32 = 0x68;

// ============================================================================
// Entry
// ============================================================================

impl CortexM4 {
    /// Stacks the context and enters exception `number`; the stacked return address is `self.pc`.
    pub(crate) fn enter_exception<B: Bus>(&mut self, bus: &mut B, number: usize) {
        self.push_frame(bus);
        self.take_exception(bus, number);
    }

    fn push_frame<B: Bus>(&mut self, bus: &mut B) {
        let floating_point_active = self.control.floating_point_active;
        let frame_size = if floating_point_active {
            EXTENDED_FRAME_SIZE
        } else {
            BASIC_FRAME_SIZE
        };
        let force_align = floating_point_active || self.scb.stack_align();
        let stack_pointer = self.registers[STACK_POINTER];
        let realigned = force_align && stack_pointer & 4 != 0;
        let mut frame = stack_pointer.wrapping_sub(frame_size);
        if force_align {
            frame &= !4;
        }
        self.registers[STACK_POINTER] = frame;

        let status = self.program_status() | (realigned as u32) << 9;
        let words = [
            self.registers[0],
            self.registers[1],
            self.registers[2],
            self.registers[3],
            self.registers[12],
            self.registers[LINK_REGISTER],
            self.pc,
            status,
        ];
        let mut failed = false;
        for (index, word) in words.into_iter().enumerate() {
            failed |= self.write_word(bus, frame + 4 * index as u32, word).is_err();
        }
        if floating_point_active {
            if self.fpu.lazy_stacking_enabled() {
                self.reserve_lazy_floating_point_frame(frame);
            } else {
                for index in 0..16 {
                    let value = self.fpu.registers[index];
                    failed |= self
                        .write_word(bus, frame + 0x20 + 4 * index as u32, value)
                        .is_err();
                }
                failed |= self.write_word(bus, frame + 0x60, self.fpu.status).is_err();
            }
        }
        if failed {
            self.scb.configurable_fault_status |= fault_status::STACKING_BUS_ERROR;
            self.pend_derived_fault(BUS_FAULT);
        }

        let mut exception_return = if self.in_handler_mode() {
            0xFFFF_FFF1
        } else if self.control.process_stack {
            0xFFFF_FFFD
        } else {
            0xFFFF_FFF9
        };
        if floating_point_active {
            exception_return &= !0x10;
        }
        self.registers[LINK_REGISTER] = exception_return;
    }

    /// ExceptionTaken: jumps to the handler of `number` in handler mode on the main stack.
    fn take_exception<B: Bus>(&mut self, bus: &mut B, number: usize) {
        let vector_address = self.scb.vector_table_offset.wrapping_add(4 * number as u32);
        let vector = match bus.read(vector_address, pinetime_core::AccessSize::Word) {
            Ok(vector) => vector,
            Err(_) => {
                self.scb.hard_fault_status |= hard_fault_status::VECTOR_TABLE;
                if number == HARD_FAULT || self.exceptions.execution_priority <= -1 {
                    self.enter_lockup();
                } else {
                    self.take_exception(bus, HARD_FAULT);
                }
                return;
            }
        };
        self.exception_number = number as u16;
        self.control.process_stack = false;
        self.control.floating_point_active = false;
        self.select_stack_pointer();
        self.it_state = 0;
        self.thumb = vector & 1 != 0;
        self.pc = vector & !1;
        self.exceptions.active |= 1 << number;
        self.exceptions.pending &= !(1 << number);
        self.refresh_execution_priority();
        self.event_register = true;
        self.exclusive_monitor_open = false;
    }

    // ========================================================================
    // Return
    // ========================================================================

    /// ExceptionReturn for an EXC_RETURN value branched to in handler mode; returns its cost.
    pub(crate) fn return_from_exception<B: Bus>(
        &mut self,
        bus: &mut B,
        exception_return: u32,
    ) -> Result<u32, Trap> {
        let returning = self.exception_number as usize;
        let kind = exception_return & 0xF;
        let invalid = Err(Trap::Fault(Fault::InvalidPc));
        if exception_return & 0xFFFF_FFE0 != 0xFFFF_FFE0
            || !matches!(kind, 1 | 9 | 13)
            || self.exceptions.active & (1 << returning) == 0
        {
            return invalid;
        }
        let nested = self.exceptions.active_count();
        if kind == 1 && nested == 1 || kind != 1 && nested != 1 && !self.scb.non_base_thread_enable() {
            return invalid;
        }

        self.exceptions.active &= !(1 << returning);
        if returning != NON_MASKABLE_INTERRUPT {
            self.faultmask = false;
        }
        self.refresh_execution_priority();
        self.repend_held_lines();
        self.exclusive_monitor_open = false;
        self.event_register = true;

        if let Some(next) = self.preempting_exception() {
            self.registers[LINK_REGISTER] = exception_return;
            self.take_exception(bus, next);
            return Ok(TAIL_CHAIN_CYCLES);
        }

        self.pop_frame(bus, exception_return);
        if kind != 1 && self.scb.sleep_on_exit() {
            self.sleep = Sleep::WaitForInterrupt;
        }
        Ok(RETURN_CYCLES)
    }

    fn pop_frame<B: Bus>(&mut self, bus: &mut B, exception_return: u32) {
        let kind = exception_return & 0xF;
        let extended_frame = exception_return & 0x10 == 0;
        let frame = if kind == 13 {
            self.process_stack_pointer()
        } else {
            self.main_stack_pointer()
        };
        let mut failed = false;
        let mut words = [0u32; 8];
        for (index, word) in words.iter_mut().enumerate() {
            match self.read_word(bus, frame + 4 * index as u32) {
                Ok(value) => *word = value,
                Err(_) => failed = true,
            }
        }
        if extended_frame {
            if self.fpu.lazy_state_active() {
                self.fpu.context_control &= !crate::fpu::context_control::LAZY_STATE_ACTIVE;
            } else {
                for index in 0..16 {
                    match self.read_word(bus, frame + 0x20 + 4 * index as u32) {
                        Ok(value) => self.fpu.registers[index] = value,
                        Err(_) => failed = true,
                    }
                }
                match self.read_word(bus, frame + 0x60) {
                    Ok(value) => self.fpu.status = value & crate::fpu::STATUS_WRITABLE,
                    Err(_) => failed = true,
                }
            }
        }
        if failed {
            self.scb.configurable_fault_status |= fault_status::UNSTACKING_BUS_ERROR;
            self.pend_derived_fault(BUS_FAULT);
        }

        let status = words[7];
        let frame_size = if extended_frame {
            EXTENDED_FRAME_SIZE
        } else {
            BASIC_FRAME_SIZE
        };
        let force_align = extended_frame || self.scb.stack_align();
        let realigned = force_align && status & (1 << 9) != 0;
        let restored_stack = frame.wrapping_add(frame_size) | (realigned as u32) << 2;
        if kind == 13 {
            self.set_process_stack_pointer(restored_stack);
        } else {
            self.set_main_stack_pointer(restored_stack);
        }

        self.registers[0] = words[0];
        self.registers[1] = words[1];
        self.registers[2] = words[2];
        self.registers[3] = words[3];
        self.registers[12] = words[4];
        self.registers[LINK_REGISTER] = words[5];
        self.pc = words[6] & !1;
        self.set_program_status(status & !(1 << 9));
        if kind == 1 {
            if self.exception_number == 0 {
                self.exception_number = self.highest_active_exception() as u16;
            }
        } else {
            self.exception_number = 0;
        }
        self.control.floating_point_active = extended_frame;
        self.control.process_stack = kind == 13;
        self.select_stack_pointer();
    }

    fn highest_active_exception(&self) -> usize {
        let active = self.exceptions.active;
        if active == 0 {
            0
        } else {
            63 - active.leading_zeros() as usize
        }
    }

    // ========================================================================
    // Faults
    // ========================================================================

    /// Takes a fault's exception, escalating to HardFault or lockup when it cannot preempt.
    pub(crate) fn raise_fault<B: Bus>(&mut self, bus: &mut B, fault: Fault) {
        let scb = &mut self.scb;
        let number = match fault {
            Fault::Undefined => {
                scb.configurable_fault_status |= fault_status::UNDEFINED_INSTRUCTION;
                USAGE_FAULT
            }
            Fault::InvalidState => {
                scb.configurable_fault_status |= fault_status::INVALID_STATE;
                USAGE_FAULT
            }
            Fault::InvalidPc => {
                scb.configurable_fault_status |= fault_status::INVALID_PC;
                USAGE_FAULT
            }
            Fault::NoCoprocessor => {
                scb.configurable_fault_status |= fault_status::NO_COPROCESSOR;
                USAGE_FAULT
            }
            Fault::Unaligned => {
                scb.configurable_fault_status |= fault_status::UNALIGNED;
                USAGE_FAULT
            }
            Fault::DivideByZero => {
                scb.configurable_fault_status |= fault_status::DIVIDE_BY_ZERO;
                USAGE_FAULT
            }
            Fault::DataBus(address) => {
                scb.configurable_fault_status |=
                    fault_status::PRECISE_DATA_BUS_ERROR | fault_status::BUS_FAULT_ADDRESS_VALID;
                scb.bus_fault_address = address;
                BUS_FAULT
            }
            Fault::InstructionBus => {
                scb.configurable_fault_status |= fault_status::INSTRUCTION_BUS_ERROR;
                BUS_FAULT
            }
            Fault::InstructionAccess => {
                scb.configurable_fault_status |= fault_status::INSTRUCTION_ACCESS_VIOLATION;
                MEMORY_MANAGEMENT_FAULT
            }
        };
        self.take_synchronous_exception(bus, number);
    }

    /// Enters SVCall now, or HardFault if SVCall cannot preempt.
    pub(crate) fn supervisor_call<B: Bus>(&mut self, bus: &mut B) {
        self.take_synchronous_exception(bus, SUPERVISOR_CALL);
    }

    fn take_synchronous_exception<B: Bus>(&mut self, bus: &mut B, number: usize) {
        let can_preempt = self.exceptions.enabled & (1 << number) != 0
            && group_priority(self.exceptions.priority(number), self.scb.priority_group)
                < self.exceptions.execution_priority;
        let target = if can_preempt {
            number
        } else {
            self.scb.hard_fault_status |= hard_fault_status::FORCED;
            HARD_FAULT
        };
        if target == HARD_FAULT && self.exceptions.execution_priority <= -1 {
            self.enter_lockup();
            return;
        }
        self.enter_exception(bus, target);
    }

    /// Pends the fault an exception entry or return caused, or HardFault when it is disabled.
    fn pend_derived_fault(&mut self, number: usize) {
        if self.exceptions.enabled & (1 << number) != 0 {
            self.set_exception_pending(number);
        } else {
            self.scb.hard_fault_status |= hard_fault_status::FORCED;
            self.set_exception_pending(HARD_FAULT);
        }
    }

    pub(crate) fn enter_lockup(&mut self) {
        self.locked_up = true;
    }
}
