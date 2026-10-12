// SPDX-License-Identifier: GPL-3.0-only
//! Data processing, shifts, multiplies, divides and bit-field operations.
//! Ported from InfiniEmu's cpu.c and pseudocode.h.

use super::{Fault, Outcome, Trap};
use crate::cpu::CortexM4;
use crate::decode::{DataOperation, ExtendKind, FlagMode, ImmediateCarry, LongMultiply, ReverseKind, Shift};

// ============================================================================
// Arithmetic helpers
// ============================================================================

/// AddWithCarry from the Arm manual: the sum, the carry out and the signed overflow.
#[inline(always)]
pub(crate) fn add_with_carry(first: u32, second: u32, carry_in: bool) -> (u32, bool, bool) {
    let (partial, carry_one) = first.overflowing_add(second);
    let (result, carry_two) = partial.overflowing_add(carry_in as u32);
    let overflow = ((first ^ result) & (second ^ result)) >> 31 != 0;
    (result, carry_one | carry_two, overflow)
}

/// Shift_C from the Arm manual, for any amount 0..255: the result and the carry out.
#[inline(always)]
pub(crate) fn shift_with_carry(value: u32, shift: Shift, amount: u32, carry_in: bool) -> (u32, bool) {
    if amount == 0 && shift != Shift::Rrx {
        return (value, carry_in);
    }
    match shift {
        Shift::Lsl => match amount {
            1..=31 => (value << amount, (value >> (32 - amount)) & 1 != 0),
            32 => (0, value & 1 != 0),
            _ => (0, false),
        },
        Shift::Lsr => match amount {
            1..=31 => (value >> amount, (value >> (amount - 1)) & 1 != 0),
            32 => (0, value >> 31 != 0),
            _ => (0, false),
        },
        Shift::Asr => {
            if amount >= 32 {
                let sign = (value as i32 >> 31) as u32;
                (sign, sign & 1 != 0)
            } else {
                ((value as i32 >> amount) as u32, (value >> (amount - 1)) & 1 != 0)
            }
        }
        Shift::Ror => {
            let result = value.rotate_right(amount & 31);
            (result, result >> 31 != 0)
        }
        Shift::Rrx => ((value >> 1) | ((carry_in as u32) << 31), value & 1 != 0),
    }
}

/// Signed saturation to `bits` bits: the clamped value and whether it was clamped.
#[inline]
pub(crate) fn signed_saturate(value: i64, bits: u32) -> (i32, bool) {
    let maximum = (1i64 << (bits - 1)) - 1;
    let minimum = -(1i64 << (bits - 1));
    if value > maximum {
        (maximum as i32, true)
    } else if value < minimum {
        (minimum as i32, true)
    } else {
        (value as i32, false)
    }
}

/// Unsigned saturation to `bits` bits: the clamped value and whether it was clamped.
#[inline]
pub(crate) fn unsigned_saturate(value: i64, bits: u32) -> (u32, bool) {
    let maximum = (1i64 << bits) - 1;
    if value > maximum {
        (maximum as u32, true)
    } else if value < 0 {
        (0, true)
    } else {
        (value as u32, false)
    }
}

impl CortexM4 {
    // ========================================================================
    // Data processing
    // ========================================================================

    #[inline(always)]
    pub(crate) fn execute_data_immediate(
        &mut self,
        operation: DataOperation,
        flags: FlagMode,
        destination: u8,
        first: u8,
        immediate: u32,
        carry: ImmediateCarry,
    ) -> u32 {
        let shifter_carry = match carry {
            ImmediateCarry::Unchanged => self.flags.carry,
            ImmediateCarry::Clear => false,
            ImmediateCarry::Set => true,
        };
        let first_value = self.read_register(first);
        self.data_operation(
            operation,
            flags,
            destination,
            first_value,
            immediate,
            shifter_carry,
        )
    }

    #[inline(always)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_data_register(
        &mut self,
        operation: DataOperation,
        flags: FlagMode,
        destination: u8,
        first: u8,
        second: u8,
        shift: Shift,
        amount: u8,
    ) -> u32 {
        let (second_value, shifter_carry) =
            shift_with_carry(self.read_register(second), shift, amount as u32, self.flags.carry);
        let first_value = self.read_register(first);
        self.data_operation(
            operation,
            flags,
            destination,
            first_value,
            second_value,
            shifter_carry,
        )
    }

    pub(crate) fn execute_shift_by_register(
        &mut self,
        shift: Shift,
        flags: FlagMode,
        destination: u8,
        value: u8,
        amount: u8,
    ) -> u32 {
        let amount = self.read_register(amount) & 0xFF;
        let (result, carry) = shift_with_carry(self.read_register(value), shift, amount, self.flags.carry);
        self.write_register(destination, result);
        if self.sets_flags(flags) {
            self.flags.set_negative_zero(result);
            self.flags.carry = carry;
        }
        1
    }

    #[inline(always)]
    fn data_operation(
        &mut self,
        operation: DataOperation,
        flags: FlagMode,
        destination: u8,
        first: u32,
        second: u32,
        shifter_carry: bool,
    ) -> u32 {
        use DataOperation::*;
        let carry = self.flags.carry;
        let (result, arithmetic, writes) = match operation {
            And => (first & second, None, true),
            Bic => (first & !second, None, true),
            Orr => (first | second, None, true),
            Orn => (first | !second, None, true),
            Eor => (first ^ second, None, true),
            Mov => (second, None, true),
            Mvn => (!second, None, true),
            Tst => (first & second, None, false),
            Teq => (first ^ second, None, false),
            Add => arithmetic_result(add_with_carry(first, second, false), true),
            Adc => arithmetic_result(add_with_carry(first, second, carry), true),
            Sub => arithmetic_result(add_with_carry(first, !second, true), true),
            Sbc => arithmetic_result(add_with_carry(first, !second, carry), true),
            Rsb => arithmetic_result(add_with_carry(!first, second, true), true),
            Cmp => arithmetic_result(add_with_carry(first, !second, true), false),
            Cmn => arithmetic_result(add_with_carry(first, second, false), false),
        };
        if writes {
            self.write_register(destination, result);
        }
        if self.sets_flags(flags) {
            self.flags.set_negative_zero(result);
            match arithmetic {
                Some((carry_out, overflow)) => {
                    self.flags.carry = carry_out;
                    self.flags.overflow = overflow;
                }
                None => self.flags.carry = shifter_carry,
            }
        }
        if writes && destination == 15 {
            2
        } else {
            1
        }
    }

    // ========================================================================
    // Multiplies and divides
    // ========================================================================

    pub(crate) fn execute_multiply(
        &mut self,
        flags: FlagMode,
        destination: u8,
        first: u8,
        second: u8,
    ) -> u32 {
        let result = self.read_register(first).wrapping_mul(self.read_register(second));
        self.write_register(destination, result);
        if self.sets_flags(flags) {
            self.flags.set_negative_zero(result);
        }
        1
    }

    pub(crate) fn execute_multiply_accumulate(
        &mut self,
        subtract: bool,
        destination: u8,
        first: u8,
        second: u8,
        accumulator: u8,
    ) -> u32 {
        let product = self.read_register(first).wrapping_mul(self.read_register(second));
        let addend = self.read_register(accumulator);
        let result = if subtract {
            addend.wrapping_sub(product)
        } else {
            addend.wrapping_add(product)
        };
        self.write_register(destination, result);
        2
    }

    pub(crate) fn execute_multiply_long(
        &mut self,
        operation: LongMultiply,
        low: u8,
        high: u8,
        first: u8,
        second: u8,
    ) -> u32 {
        let first_value = self.read_register(first);
        let second_value = self.read_register(second);
        let accumulated = ((self.read_register(high) as u64) << 32) | self.read_register(low) as u64;
        let result = match operation {
            LongMultiply::UnsignedMultiply => first_value as u64 * second_value as u64,
            LongMultiply::SignedMultiply => (first_value as i32 as i64 * second_value as i32 as i64) as u64,
            LongMultiply::UnsignedAccumulate => {
                (first_value as u64 * second_value as u64).wrapping_add(accumulated)
            }
            LongMultiply::SignedAccumulate => {
                ((first_value as i32 as i64 * second_value as i32 as i64) as u64).wrapping_add(accumulated)
            }
            LongMultiply::UnsignedDoubleAccumulate => {
                first_value as u64 * second_value as u64
                    + self.read_register(high) as u64
                    + self.read_register(low) as u64
            }
        };
        self.write_register(low, result as u32);
        self.write_register(high, (result >> 32) as u32);
        1
    }

    pub(crate) fn execute_divide(
        &mut self,
        signed: bool,
        destination: u8,
        dividend: u8,
        divisor: u8,
    ) -> Outcome {
        let numerator = self.read_register(dividend);
        let denominator = self.read_register(divisor);
        let result = if denominator == 0 {
            if self.scb.divide_by_zero_trap() {
                return Err(Trap::Fault(Fault::DivideByZero));
            }
            0
        } else if signed {
            (numerator as i32).wrapping_div(denominator as i32) as u32
        } else {
            numerator / denominator
        };
        self.write_register(destination, result);
        Ok(6)
    }

    // ========================================================================
    // Bit fields, reversal and extension
    // ========================================================================

    pub(crate) fn execute_bit_field_clear(&mut self, destination: u8, lsb: u8, msb: u8) -> u32 {
        if msb >= lsb {
            let mask = field_mask(lsb as u32, msb as u32);
            self.write_register(destination, self.read_register(destination) & !mask);
        }
        1
    }

    pub(crate) fn execute_bit_field_insert(&mut self, destination: u8, source: u8, lsb: u8, msb: u8) -> u32 {
        if msb >= lsb {
            let mask = field_mask(lsb as u32, msb as u32);
            let inserted = (self.read_register(source) << lsb) & mask;
            self.write_register(destination, (self.read_register(destination) & !mask) | inserted);
        }
        1
    }

    pub(crate) fn execute_bit_field_extract(
        &mut self,
        signed: bool,
        destination: u8,
        source: u8,
        lsb: u8,
        width: u8,
    ) -> u32 {
        let width = (width as u32).min(32 - lsb as u32);
        let shifted = self.read_register(source) << (32 - lsb as u32 - width);
        let result = if signed {
            ((shifted as i32) >> (32 - width)) as u32
        } else {
            shifted >> (32 - width)
        };
        self.write_register(destination, result);
        1
    }

    pub(crate) fn execute_byte_reverse(&mut self, kind: ReverseKind, destination: u8, source: u8) -> u32 {
        let value = self.read_register(source);
        let result = match kind {
            ReverseKind::Word => value.swap_bytes(),
            ReverseKind::PackedHalfword => ((value & 0x00FF_00FF) << 8) | ((value >> 8) & 0x00FF_00FF),
            ReverseKind::SignedHalfword => (value as u16).swap_bytes() as i16 as i32 as u32,
        };
        self.write_register(destination, result);
        1
    }

    pub(crate) fn execute_extend(
        &mut self,
        kind: ExtendKind,
        destination: u8,
        addend: u8,
        source: u8,
        rotation: u8,
    ) -> u32 {
        let rotated = self.read_register(source).rotate_right(rotation as u32);
        let base = if addend == 15 {
            0
        } else {
            self.read_register(addend)
        };
        let result = match kind {
            ExtendKind::SignedByte => base.wrapping_add(rotated as i8 as i32 as u32),
            ExtendKind::SignedHalfword => base.wrapping_add(rotated as i16 as i32 as u32),
            ExtendKind::UnsignedByte => base.wrapping_add(rotated & 0xFF),
            ExtendKind::UnsignedHalfword => base.wrapping_add(rotated & 0xFFFF),
            ExtendKind::SignedBytePair => {
                let low = (base as u16).wrapping_add(rotated as i8 as i16 as u16);
                let high = ((base >> 16) as u16).wrapping_add((rotated >> 16) as i8 as i16 as u16);
                low as u32 | (high as u32) << 16
            }
            ExtendKind::UnsignedBytePair => {
                let low = (base as u16).wrapping_add((rotated & 0xFF) as u16);
                let high = ((base >> 16) as u16).wrapping_add(((rotated >> 16) & 0xFF) as u16);
                low as u32 | (high as u32) << 16
            }
        };
        self.write_register(destination, result);
        1
    }
}

#[inline(always)]
fn arithmetic_result(sum: (u32, bool, bool), writes: bool) -> (u32, Option<(bool, bool)>, bool) {
    (sum.0, Some((sum.1, sum.2)), writes)
}

fn field_mask(lsb: u32, msb: u32) -> u32 {
    let width = msb - lsb + 1;
    let ones = if width >= 32 { u32::MAX } else { (1 << width) - 1 };
    ones << lsb
}
