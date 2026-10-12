// SPDX-License-Identifier: GPL-3.0-only
//! FPv4-SP semantics: single-precision arithmetic with the Arm NaN rules, flush-to-zero,
//! default NaN, compares, conversions and the FP loads, stores and moves.
//! Ported from InfiniEmu's cpu.c.
//!
//! Arithmetic always rounds to nearest; only integer conversions follow FPSCR.RMode.

use pinetime_core::Bus;

use super::{Outcome, Trap};
use crate::cpu::CortexM4;
use crate::decode::{FloatConversion, FloatOperation, FloatUnary, Instruction};

// ============================================================================
// FPSCR bits
// ============================================================================

const INVALID_OPERATION: u32 = 1 << 0;
const DIVIDE_BY_ZERO: u32 = 1 << 1;
const OVERFLOW: u32 = 1 << 2;
const UNDERFLOW: u32 = 1 << 3;
const INEXACT: u32 = 1 << 4;
const INPUT_DENORMAL: u32 = 1 << 7;
const FLUSH_TO_ZERO: u32 = 1 << 24;
const DEFAULT_NAN_MODE: u32 = 1 << 25;

const DEFAULT_NAN: u32 = 0x7FC0_0000;
const QUIET_BIT: u32 = 0x0040_0000;

fn is_nan(bits: u32) -> bool {
    bits & 0x7FFF_FFFF > 0x7F80_0000
}

fn is_signaling_nan(bits: u32) -> bool {
    is_nan(bits) && bits & QUIET_BIT == 0
}

fn is_denormal(bits: u32) -> bool {
    bits & 0x7F80_0000 == 0 && bits & 0x007F_FFFF != 0
}

fn is_infinite(bits: u32) -> bool {
    bits & 0x7FFF_FFFF == 0x7F80_0000
}

// ============================================================================
// Rounding helpers
// ============================================================================

/// Rounds `value` to an integer by FPSCR rounding mode `mode` (0 nearest, 1 up, 2 down, 3 zero).
fn round_by_mode(value: f64, mode: u32) -> f64 {
    match mode {
        0 => value.round_ties_even(),
        1 => value.ceil(),
        2 => value.floor(),
        _ => value.trunc(),
    }
}

/// Single to half precision bits, rounding to nearest even.
fn single_to_half(bits: u32) -> u16 {
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exponent = ((bits >> 23) & 0xFF) as i32;
    let mantissa = bits & 0x007F_FFFF;
    if exponent == 0xFF {
        return sign
            | 0x7C00
            | if mantissa != 0 {
                0x0200 | (mantissa >> 13) as u16
            } else {
                0
            };
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7C00;
    }
    if unbiased >= -14 {
        let half_exponent = (unbiased + 15) as u32;
        let mut half = (half_exponent << 10) | (mantissa >> 13);
        let remainder = mantissa & 0x1FFF;
        if remainder > 0x1000 || remainder == 0x1000 && half & 1 != 0 {
            half += 1;
        }
        return sign | half as u16;
    }
    if unbiased < -25 {
        return sign;
    }
    let full = mantissa | 0x0080_0000;
    let shift = (-14 - unbiased + 13) as u32;
    let mut half = full >> shift;
    let remainder = full & ((1 << shift) - 1);
    let halfway = 1 << (shift - 1);
    if remainder > halfway || remainder == halfway && half & 1 != 0 {
        half += 1;
    }
    sign | half as u16
}

fn half_to_single(half: u16) -> u32 {
    let sign = ((half as u32) & 0x8000) << 16;
    let exponent = ((half >> 10) & 0x1F) as u32;
    let mantissa = (half & 0x3FF) as u32;
    match exponent {
        0 if mantissa == 0 => sign,
        0 => {
            let value = mantissa as f32 / (1 << 24) as f32;
            sign | value.to_bits()
        }
        0x1F => sign | 0x7F80_0000 | (mantissa << 13) | if mantissa != 0 { QUIET_BIT } else { 0 },
        _ => sign | ((exponent + 112) << 23) | (mantissa << 13),
    }
}

impl CortexM4 {
    // ========================================================================
    // FPSCR-driven operand and result handling
    // ========================================================================

    fn raise_floating_point_flags(&mut self, flags: u32) {
        self.fpu.status |= flags;
    }

    /// An operand after flush-to-zero.
    fn operand(&mut self, bits: u32) -> u32 {
        if self.fpu.status & FLUSH_TO_ZERO != 0 && is_denormal(bits) {
            self.raise_floating_point_flags(INPUT_DENORMAL);
            bits & 0x8000_0000
        } else {
            bits
        }
    }

    /// The quieted NaN an operation returns when an operand is a NaN, per the Arm NaN rules.
    fn propagate_nans(&mut self, operands: &[u32]) -> Option<u32> {
        let chosen = operands
            .iter()
            .copied()
            .find(|&bits| is_signaling_nan(bits))
            .or_else(|| operands.iter().copied().find(|&bits| is_nan(bits)))?;
        if is_signaling_nan(chosen) {
            self.raise_floating_point_flags(INVALID_OPERATION);
        }
        if operands.iter().any(|&bits| is_signaling_nan(bits)) {
            self.raise_floating_point_flags(INVALID_OPERATION);
        }
        Some(if self.fpu.status & DEFAULT_NAN_MODE != 0 {
            DEFAULT_NAN
        } else {
            chosen | QUIET_BIT
        })
    }

    /// A computed result after default-NaN, overflow and flush-to-zero handling.
    fn result(&mut self, value: f32, inputs_finite: bool) -> u32 {
        let bits = value.to_bits();
        if value.is_nan() {
            self.raise_floating_point_flags(INVALID_OPERATION);
            return DEFAULT_NAN;
        }
        if inputs_finite && is_infinite(bits) {
            self.raise_floating_point_flags(OVERFLOW | INEXACT);
        }
        if self.fpu.status & FLUSH_TO_ZERO != 0 && is_denormal(bits) {
            self.raise_floating_point_flags(UNDERFLOW);
            return bits & 0x8000_0000;
        }
        bits
    }

    fn arithmetic(
        &mut self,
        operation: FloatOperation,
        addend_bits: u32,
        first_bits: u32,
        second_bits: u32,
    ) -> u32 {
        use FloatOperation::*;
        let first_bits = self.operand(first_bits);
        let second_bits = self.operand(second_bits);
        let accumulates = !matches!(operation, Add | Subtract | Multiply | NegateMultiply | Divide);
        let addend_bits = if accumulates { self.operand(addend_bits) } else { 0 };
        let nan_inputs: &[u32] = if accumulates {
            &[addend_bits, first_bits, second_bits]
        } else {
            &[first_bits, second_bits]
        };
        if let Some(nan) = self.propagate_nans(nan_inputs) {
            return nan;
        }
        let finite = nan_inputs.iter().all(|&bits| !is_infinite(bits));
        let first = f32::from_bits(first_bits);
        let second = f32::from_bits(second_bits);
        let addend = f32::from_bits(addend_bits);
        if operation == Divide && second == 0.0 && first.is_finite() && first != 0.0 {
            self.raise_floating_point_flags(DIVIDE_BY_ZERO);
            return ((first_bits ^ second_bits) & 0x8000_0000) | 0x7F80_0000;
        }
        let value = match operation {
            Add => first + second,
            Subtract => first - second,
            Multiply => first * second,
            NegateMultiply => -(first * second),
            Divide => first / second,
            MultiplyAccumulate => {
                let product = self.round_intermediate(first * second);
                addend + product
            }
            MultiplySubtract => {
                let product = self.round_intermediate(first * second);
                addend - product
            }
            NegateMultiplyAccumulate => {
                let product = self.round_intermediate(first * second);
                -addend - product
            }
            NegateMultiplySubtract => {
                let product = self.round_intermediate(first * second);
                -addend + product
            }
            FusedMultiplyAccumulate => first.mul_add(second, addend),
            FusedMultiplySubtract => (-first).mul_add(second, addend),
            FusedNegateMultiplyAccumulate => (-first).mul_add(second, -addend),
            FusedNegateMultiplySubtract => first.mul_add(second, -addend),
        };
        self.result(value, finite)
    }

    /// The product a non-fused multiply-accumulate rounds, flushed like a result.
    fn round_intermediate(&mut self, product: f32) -> f32 {
        if self.fpu.status & FLUSH_TO_ZERO != 0 && is_denormal(product.to_bits()) {
            self.raise_floating_point_flags(UNDERFLOW);
            return f32::from_bits(product.to_bits() & 0x8000_0000);
        }
        product
    }

    fn compare(&mut self, first_bits: u32, second_bits: u32, signal_quiet_nan: bool) {
        let first_bits = self.operand(first_bits);
        let second_bits = self.operand(second_bits);
        let flags = if is_nan(first_bits) || is_nan(second_bits) {
            if signal_quiet_nan || is_signaling_nan(first_bits) || is_signaling_nan(second_bits) {
                self.raise_floating_point_flags(INVALID_OPERATION);
            }
            0b0011
        } else {
            let first = f32::from_bits(first_bits);
            let second = f32::from_bits(second_bits);
            if first == second {
                0b0110
            } else if first < second {
                0b1000
            } else {
                0b0010
            }
        };
        self.fpu.status = (self.fpu.status & 0x0FFF_FFFF) | (flags << 28);
    }

    fn convert_to_integer(
        &mut self,
        bits: u32,
        signed: bool,
        round_to_zero: bool,
        fraction_bits: u32,
        width: u32,
    ) -> u32 {
        let bits = self.operand(bits);
        if is_nan(bits) {
            self.raise_floating_point_flags(INVALID_OPERATION);
            return 0;
        }
        let mode = if round_to_zero {
            3
        } else {
            (self.fpu.status >> 22) & 3
        };
        let scaled = f32::from_bits(bits) as f64 * (1u64 << fraction_bits) as f64;
        let rounded = round_by_mode(scaled, mode);
        let (minimum, maximum) = if signed {
            (-(2f64.powi(width as i32 - 1)), 2f64.powi(width as i32 - 1) - 1.0)
        } else {
            (0.0, 2f64.powi(width as i32) - 1.0)
        };
        if rounded < minimum || rounded > maximum {
            self.raise_floating_point_flags(INVALID_OPERATION);
            let clamped = rounded.clamp(minimum, maximum);
            return if signed {
                clamped as i64 as u32
            } else {
                clamped as u64 as u32
            };
        }
        if rounded != scaled {
            self.raise_floating_point_flags(INEXACT);
        }
        if signed {
            rounded as i64 as u32
        } else {
            rounded as u64 as u32
        }
    }

    // ========================================================================
    // Dispatch
    // ========================================================================

    pub(crate) fn execute_floating_point<B: Bus>(
        &mut self,
        bus: &mut B,
        instruction: Instruction,
    ) -> Outcome {
        use Instruction::*;
        self.check_floating_point(bus)?;
        match instruction {
            FloatLoad {
                register,
                double,
                base,
                offset,
            } => {
                let address = self.float_base(base).wrapping_add(offset as u32);
                self.fpu.registers[register as usize] = self.read_word(bus, address)?;
                if double {
                    self.fpu.registers[register as usize + 1] =
                        self.read_word(bus, address.wrapping_add(4))?;
                    return Ok(3);
                }
                Ok(2)
            }
            FloatStore {
                register,
                double,
                base,
                offset,
            } => {
                let address = self.float_base(base).wrapping_add(offset as u32);
                self.write_word(bus, address, self.fpu.registers[register as usize])?;
                if double {
                    self.write_word(
                        bus,
                        address.wrapping_add(4),
                        self.fpu.registers[register as usize + 1],
                    )?;
                    return Ok(3);
                }
                Ok(2)
            }
            FloatLoadMultiple {
                first_register,
                count,
                base,
                writeback,
                decrement_before,
            } => self.float_multiple(
                bus,
                true,
                first_register,
                count,
                base,
                writeback,
                decrement_before,
            ),
            FloatStoreMultiple {
                first_register,
                count,
                base,
                writeback,
                decrement_before,
            } => self.float_multiple(
                bus,
                false,
                first_register,
                count,
                base,
                writeback,
                decrement_before,
            ),
            FloatMoveCore {
                to_core,
                core,
                single,
            } => {
                if to_core {
                    self.write_register(core, self.fpu.registers[single as usize]);
                } else {
                    self.fpu.registers[single as usize] = self.read_register(core);
                }
                Ok(1)
            }
            FloatMoveCorePair {
                to_core,
                first_core,
                second_core,
                single,
            } => {
                let index = single as usize;
                if to_core {
                    self.write_register(first_core, self.fpu.registers[index]);
                    self.write_register(second_core, self.fpu.registers[index + 1]);
                } else {
                    self.fpu.registers[index] = self.read_register(first_core);
                    self.fpu.registers[index + 1] = self.read_register(second_core);
                }
                Ok(1)
            }
            FloatMoveImmediate { destination, value } => {
                self.fpu.registers[destination as usize] = value;
                Ok(1)
            }
            FloatStatusMove { to_core, core } => {
                if !to_core {
                    self.fpu.status = self.read_register(core) & crate::fpu::STATUS_WRITABLE;
                } else if core == 15 {
                    self.flags
                        .set_from_bits(self.fpu.status & 0xF000_0000, true, false);
                } else {
                    self.write_register(core, self.fpu.status);
                }
                Ok(1)
            }
            FloatArithmetic {
                operation,
                destination,
                first,
                second,
            } => {
                let registers = &self.fpu.registers;
                let (addend, first_bits, second_bits) = (
                    registers[destination as usize],
                    registers[first as usize],
                    registers[second as usize],
                );
                let value = self.arithmetic(operation, addend, first_bits, second_bits);
                self.fpu.registers[destination as usize] = value;
                Ok(match operation {
                    FloatOperation::Divide => 14,
                    FloatOperation::Add
                    | FloatOperation::Subtract
                    | FloatOperation::Multiply
                    | FloatOperation::NegateMultiply => 1,
                    _ => 3,
                })
            }
            FloatUnary {
                operation,
                destination,
                source,
            } => {
                let bits = self.fpu.registers[source as usize];
                let (value, cycles) = match operation {
                    self::FloatUnary::Move => (bits, 1),
                    self::FloatUnary::Absolute => (bits & 0x7FFF_FFFF, 1),
                    self::FloatUnary::Negate => (bits ^ 0x8000_0000, 1),
                    self::FloatUnary::SquareRoot => {
                        let operand = self.operand(bits);
                        let value = match self.propagate_nans(&[operand]) {
                            Some(nan) => nan,
                            None => self.result(f32::from_bits(operand).sqrt(), true),
                        };
                        (value, 14)
                    }
                };
                self.fpu.registers[destination as usize] = value;
                Ok(cycles)
            }
            FloatCompare {
                first,
                second,
                with_zero,
                signal_quiet_nan,
            } => {
                let first_bits = self.fpu.registers[first as usize];
                let second_bits = if with_zero {
                    0
                } else {
                    self.fpu.registers[second as usize]
                };
                self.compare(first_bits, second_bits, signal_quiet_nan);
                Ok(1)
            }
            FloatConvert {
                conversion,
                destination,
                source,
                round_to_zero,
            } => {
                let bits = self.fpu.registers[source as usize];
                let value = match conversion {
                    FloatConversion::FloatToSigned => {
                        self.convert_to_integer(bits, true, round_to_zero, 0, 32)
                    }
                    FloatConversion::FloatToUnsigned => {
                        self.convert_to_integer(bits, false, round_to_zero, 0, 32)
                    }
                    FloatConversion::SignedToFloat => (bits as i32 as f32).to_bits(),
                    FloatConversion::UnsignedToFloat => (bits as f32).to_bits(),
                };
                self.fpu.registers[destination as usize] = value;
                Ok(1)
            }
            FloatConvertFixed {
                to_fixed,
                signed,
                halfword,
                register,
                fraction_bits,
            } => {
                let index = register as usize;
                let bits = self.fpu.registers[index];
                let width = if halfword { 16 } else { 32 };
                self.fpu.registers[index] = if to_fixed {
                    let fixed = self.convert_to_integer(bits, signed, true, fraction_bits as u32, width);
                    match (halfword, signed) {
                        (true, true) => fixed as i16 as i32 as u32,
                        (true, false) => fixed & 0xFFFF,
                        _ => fixed,
                    }
                } else {
                    let integer = match (halfword, signed) {
                        (true, true) => bits as i16 as f64,
                        (true, false) => (bits & 0xFFFF) as f64,
                        (false, true) => bits as i32 as f64,
                        (false, false) => bits as f64,
                    };
                    ((integer / (1u64 << fraction_bits) as f64) as f32).to_bits()
                };
                Ok(1)
            }
            FloatConvertHalf {
                to_half,
                top,
                destination,
                source,
            } => {
                let shift = if top { 16 } else { 0 };
                let source_bits = self.fpu.registers[source as usize];
                let destination_index = destination as usize;
                self.fpu.registers[destination_index] = if to_half {
                    let half = single_to_half(source_bits) as u32;
                    (self.fpu.registers[destination_index] & !(0xFFFF << shift)) | (half << shift)
                } else {
                    half_to_single((source_bits >> shift) as u16)
                };
                Ok(1)
            }
            _ => Err(Trap::Fault(super::Fault::Undefined)),
        }
    }

    fn float_base(&self, base: u8) -> u32 {
        let value = self.read_register(base);
        if base == 15 {
            value & !3
        } else {
            value
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn float_multiple<B: Bus>(
        &mut self,
        bus: &mut B,
        load: bool,
        first_register: u8,
        count: u8,
        base: u8,
        writeback: bool,
        decrement_before: bool,
    ) -> Outcome {
        let count = count as u32;
        let base_value = self.read_register(base);
        let start = if decrement_before {
            base_value.wrapping_sub(4 * count)
        } else {
            base_value
        };
        for offset in 0..count {
            let address = start.wrapping_add(4 * offset);
            let index = first_register as usize + offset as usize;
            if load {
                self.fpu.registers[index] = self.read_word(bus, address)?;
            } else {
                self.write_word(bus, address, self.fpu.registers[index])?;
            }
        }
        if writeback {
            let final_base = if decrement_before {
                start
            } else {
                base_value.wrapping_add(4 * count)
            };
            self.write_register(base, final_base);
        }
        Ok(1 + count)
    }
}
