// SPDX-License-Identifier: GPL-3.0-only
//! Instruction semantics. Ported from InfiniEmu's cpu.c, with its cycle counts.
//!
//! While an instruction runs, r15 reads as its address plus 4 and `pc` holds the next one.

mod alu;
mod dsp;
mod floating_point;
mod memory;
mod system;

use pinetime_core::Bus;

use crate::cpu::CortexM4;
use crate::decode::{FlagMode, Instruction};
use crate::registers::{PROGRAM_COUNTER, STACK_POINTER};

// ============================================================================
// Traps
// ============================================================================

/// A fault an instruction raises; each maps to a CFSR bit and a fault exception.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fault {
    Undefined,
    InvalidState,
    InvalidPc,
    NoCoprocessor,
    Unaligned,
    DivideByZero,
    DataBus(u32),
    InstructionBus,
    InstructionAccess,
}

/// Why an instruction did not simply finish.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trap {
    Fault(Fault),
    /// A branch to an EXC_RETURN value in handler mode.
    ExceptionReturn(u32),
    SupervisorCall,
    Breakpoint,
}

impl From<Fault> for Trap {
    fn from(fault: Fault) -> Trap {
        Trap::Fault(fault)
    }
}

pub(crate) type Outcome = Result<u32, Trap>;

// ============================================================================
// Register access
// ============================================================================

impl CortexM4 {
    #[inline(always)]
    pub(crate) fn read_register(&self, number: u8) -> u32 {
        self.registers[number as usize]
    }

    /// Writes a register as data processing does: r15 branches and r13 stays word aligned.
    #[inline(always)]
    pub(crate) fn write_register(&mut self, number: u8, value: u32) {
        match number as usize {
            PROGRAM_COUNTER => self.pc = value & !1,
            STACK_POINTER => self.registers[STACK_POINTER] = value & !3,
            index => self.registers[index] = value,
        }
    }

    /// BXWritePC and LoadWritePC: interworking branches that may also return from an exception.
    #[inline]
    pub(crate) fn branch_exchange(&mut self, value: u32) -> Result<(), Trap> {
        if self.in_handler_mode() && value >= 0xF000_0000 {
            return Err(Trap::ExceptionReturn(value));
        }
        self.thumb = value & 1 != 0;
        self.pc = value & !1;
        Ok(())
    }

    /// Writes a register loaded from memory; r15 interworks.
    #[inline]
    pub(crate) fn write_loaded_register(&mut self, number: u8, value: u32) -> Result<(), Trap> {
        if number as usize == PROGRAM_COUNTER {
            self.branch_exchange(value)
        } else {
            self.write_register(number, value);
            Ok(())
        }
    }

    #[inline(always)]
    pub(crate) fn sets_flags(&self, flags: FlagMode) -> bool {
        match flags {
            FlagMode::Never => false,
            FlagMode::Always => true,
            FlagMode::OutsideItBlock => self.it_state == 0,
        }
    }

    // ========================================================================
    // Dispatch
    // ========================================================================

    #[inline(always)]
    pub(crate) fn execute<B: Bus>(&mut self, bus: &mut B, instruction: Instruction) -> Outcome {
        use Instruction::*;
        match instruction {
            DataImmediate {
                operation,
                flags,
                destination,
                first,
                immediate,
                carry,
            } => Ok(self.execute_data_immediate(operation, flags, destination, first, immediate, carry)),
            DataRegister {
                operation,
                flags,
                destination,
                first,
                second,
                shift,
                amount,
            } => Ok(self.execute_data_register(operation, flags, destination, first, second, shift, amount)),
            ShiftByRegister {
                shift,
                flags,
                destination,
                value,
                amount,
            } => Ok(self.execute_shift_by_register(shift, flags, destination, value, amount)),
            Address { destination, address } => {
                self.write_register(destination, address);
                Ok(1)
            }
            MoveTop {
                destination,
                immediate,
            } => {
                let value = (self.read_register(destination) & 0xFFFF) | ((immediate as u32) << 16);
                self.write_register(destination, value);
                Ok(1)
            }
            Multiply {
                flags,
                destination,
                first,
                second,
            } => Ok(self.execute_multiply(flags, destination, first, second)),
            MultiplyAccumulate {
                subtract,
                destination,
                first,
                second,
                accumulator,
            } => Ok(self.execute_multiply_accumulate(subtract, destination, first, second, accumulator)),
            MultiplyLong {
                operation,
                low,
                high,
                first,
                second,
            } => Ok(self.execute_multiply_long(operation, low, high, first, second)),
            Divide {
                signed,
                destination,
                dividend,
                divisor,
            } => self.execute_divide(signed, destination, dividend, divisor),
            DspMultiply {
                operation,
                options,
                destination,
                first,
                second,
                accumulator,
            } => Ok(self.execute_dsp_multiply(operation, options, destination, first, second, accumulator)),
            BitFieldClear {
                destination,
                lsb,
                msb,
            } => Ok(self.execute_bit_field_clear(destination, lsb, msb)),
            BitFieldInsert {
                destination,
                source,
                lsb,
                msb,
            } => Ok(self.execute_bit_field_insert(destination, source, lsb, msb)),
            BitFieldExtract {
                signed,
                destination,
                source,
                lsb,
                width,
            } => Ok(self.execute_bit_field_extract(signed, destination, source, lsb, width)),
            CountLeadingZeros { destination, source } => {
                self.write_register(destination, self.read_register(source).leading_zeros());
                Ok(1)
            }
            ReverseBits { destination, source } => {
                self.write_register(destination, self.read_register(source).reverse_bits());
                Ok(1)
            }
            ByteReverse {
                kind,
                destination,
                source,
            } => Ok(self.execute_byte_reverse(kind, destination, source)),
            Extend {
                kind,
                destination,
                addend,
                source,
                rotation,
            } => Ok(self.execute_extend(kind, destination, addend, source, rotation)),
            Saturate {
                signed,
                halfwords,
                destination,
                source,
                bits,
                shift,
                amount,
            } => Ok(self.execute_saturate(signed, halfwords, destination, source, bits, shift, amount)),
            SaturatingArithmetic {
                operation,
                destination,
                first,
                second,
            } => Ok(self.execute_saturating_arithmetic(operation, destination, first, second)),
            Parallel {
                operation,
                variant,
                destination,
                first,
                second,
            } => Ok(self.execute_parallel(operation, variant, destination, first, second)),
            Select {
                destination,
                first,
                second,
            } => Ok(self.execute_select(destination, first, second)),
            Pack {
                top_from_second,
                destination,
                first,
                second,
                amount,
            } => Ok(self.execute_pack(top_from_second, destination, first, second, amount)),
            Branch { target } => {
                self.pc = target;
                Ok(2)
            }
            BranchLink { target } => {
                self.registers[14] = self.pc | 1;
                self.pc = target;
                Ok(2)
            }
            BranchExchange { source } => {
                self.branch_exchange(self.read_register(source))?;
                Ok(2)
            }
            BranchLinkExchange { source } => {
                let target = self.read_register(source);
                self.registers[14] = self.pc | 1;
                self.branch_exchange(target)?;
                Ok(2)
            }
            CompareBranch {
                nonzero,
                source,
                target,
            } => {
                if (self.read_register(source) != 0) == nonzero {
                    self.pc = target;
                }
                Ok(1)
            }
            TableBranch {
                halfword,
                base,
                index,
            } => self.execute_table_branch(bus, halfword, base, index),
            IfThen { state } => {
                self.it_state = state;
                Ok(1)
            }
            LoadImmediate {
                kind,
                target,
                base,
                offset,
                index,
                writeback,
            } => self.execute_load_immediate(bus, kind, target, base, offset, index, writeback),
            LoadLiteral {
                kind,
                target,
                address,
            } => self.execute_load_literal(bus, kind, target, address),
            LoadRegister {
                kind,
                target,
                base,
                offset_register,
                shift,
            } => self.execute_load_register(bus, kind, target, base, offset_register, shift),
            StoreImmediate {
                size,
                source,
                base,
                offset,
                index,
                writeback,
            } => self.execute_store_immediate(bus, size, source, base, offset, index, writeback),
            StoreRegister {
                size,
                source,
                base,
                offset_register,
                shift,
            } => self.execute_store_register(bus, size, source, base, offset_register, shift),
            LoadDual {
                first,
                second,
                base,
                offset,
                index,
                writeback,
            } => self.execute_load_dual(bus, first, second, base, offset, index, writeback),
            StoreDual {
                first,
                second,
                base,
                offset,
                index,
                writeback,
            } => self.execute_store_dual(bus, first, second, base, offset, index, writeback),
            LoadExclusive {
                size,
                target,
                base,
                offset,
            } => self.execute_load_exclusive(bus, size, target, base, offset),
            StoreExclusive {
                size,
                status,
                source,
                base,
                offset,
            } => self.execute_store_exclusive(bus, size, status, source, base, offset),
            ClearExclusive => {
                self.exclusive_monitor_open = false;
                Ok(1)
            }
            LoadMultiple {
                base,
                registers,
                writeback,
                decrement_before,
            } => self.execute_load_multiple(bus, base, registers, writeback, decrement_before),
            StoreMultiple {
                base,
                registers,
                writeback,
                decrement_before,
            } => self.execute_store_multiple(bus, base, registers, writeback, decrement_before),
            MoveFromSpecial { destination, special } => {
                Ok(self.execute_move_from_special(destination, special))
            }
            MoveToSpecial {
                source,
                special,
                mask,
            } => Ok(self.execute_move_to_special(source, special, mask)),
            ChangeProcessorState {
                disable,
                primask,
                faultmask,
            } => Ok(self.execute_change_processor_state(disable, primask, faultmask)),
            Hint { kind } => Ok(self.execute_hint(kind)),
            Breakpoint { .. } => Err(Trap::Breakpoint),
            SupervisorCall { .. } => Err(Trap::SupervisorCall),
            NoCoprocessor => Err(Trap::Fault(Fault::NoCoprocessor)),
            Undefined => Err(Trap::Fault(Fault::Undefined)),
            floating_point => self.execute_floating_point(bus, floating_point),
        }
    }
}
