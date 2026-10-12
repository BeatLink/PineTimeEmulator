// SPDX-License-Identifier: GPL-3.0-only
//! The DSP extension: saturation, parallel lane arithmetic, SEL, PKH and the DSP multiplies.

use super::alu::{shift_with_carry, signed_saturate, unsigned_saturate};
use crate::cpu::CortexM4;
use crate::decode::{
    dsp_options, DspOperation, ParallelOperation, ParallelVariant, SaturatingOperation, Shift,
};

// ============================================================================
// Lane helpers
// ============================================================================

#[inline]
fn signed_half(value: u32, top: bool) -> i64 {
    if top {
        (value >> 16) as i16 as i64
    } else {
        value as i16 as i64
    }
}

impl CortexM4 {
    // ========================================================================
    // Saturation
    // ========================================================================

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_saturate(
        &mut self,
        signed: bool,
        halfwords: bool,
        destination: u8,
        source: u8,
        bits: u8,
        shift: Shift,
        amount: u8,
    ) -> u32 {
        let value = self.read_register(source);
        let bits = bits as u32;
        let mut saturated = false;
        let result = if halfwords {
            let mut result = 0u32;
            for lane in 0..2 {
                let half = signed_half(value, lane == 1);
                let (clamped, clipped) = if signed {
                    let (clamped, clipped) = signed_saturate(half, bits);
                    (clamped as u32, clipped)
                } else {
                    unsigned_saturate(half, bits)
                };
                saturated |= clipped;
                result |= (clamped & 0xFFFF) << (16 * lane);
            }
            result
        } else {
            let (operand, _) = shift_with_carry(value, shift, amount as u32, false);
            let (clamped, clipped) = if signed {
                let (clamped, clipped) = signed_saturate(operand as i32 as i64, bits);
                (clamped as u32, clipped)
            } else {
                unsigned_saturate(operand as i32 as i64, bits)
            };
            saturated = clipped;
            clamped
        };
        if saturated {
            self.flags.saturated = true;
        }
        self.write_register(destination, result);
        1
    }

    pub(crate) fn execute_saturating_arithmetic(
        &mut self,
        operation: SaturatingOperation,
        destination: u8,
        first: u8,
        second: u8,
    ) -> u32 {
        let first_value = self.read_register(first) as i32 as i64;
        let second_value = self.read_register(second) as i32 as i64;
        let mut saturated = false;
        let mut doubled = |value: i64| {
            let (result, clipped) = signed_saturate(2 * value, 32);
            saturated |= clipped;
            result as i64
        };
        let sum = match operation {
            SaturatingOperation::Add => first_value + second_value,
            SaturatingOperation::Subtract => first_value - second_value,
            SaturatingOperation::DoubleAdd => first_value + doubled(second_value),
            SaturatingOperation::DoubleSubtract => first_value - doubled(second_value),
        };
        let (result, clipped) = signed_saturate(sum, 32);
        if saturated || clipped {
            self.flags.saturated = true;
        }
        self.write_register(destination, result as u32);
        1
    }

    // ========================================================================
    // Parallel add and subtract
    // ========================================================================

    pub(crate) fn execute_parallel(
        &mut self,
        operation: ParallelOperation,
        variant: ParallelVariant,
        destination: u8,
        first: u8,
        second: u8,
    ) -> u32 {
        use ParallelOperation::*;
        let first_value = self.read_register(first);
        let second_value = self.read_register(second);
        let signed = matches!(
            variant,
            ParallelVariant::Signed | ParallelVariant::SignedSaturating | ParallelVariant::SignedHalving
        );
        let (lane_bits, lane_count) = match operation {
            Add8 | Subtract8 => (8u32, 4u32),
            _ => (16, 2),
        };
        let lane_mask = (1u32 << lane_bits) - 1;
        let lane = |value: u32, index: u32| -> i64 {
            let raw = (value >> (index * lane_bits)) & lane_mask;
            if signed {
                ((raw << (32 - lane_bits)) as i32 >> (32 - lane_bits)) as i64
            } else {
                raw as i64
            }
        };
        let mut result = 0u32;
        let mut greater_or_equal = 0u8;
        for index in 0..lane_count {
            let (first_lane, second_lane, subtract) = match operation {
                Add16 | Add8 => (index, index, false),
                Subtract16 | Subtract8 => (index, index, true),
                AddSubtractExchange => (index, 1 - index, index == 0),
                SubtractAddExchange => (index, 1 - index, index == 1),
            };
            let x = lane(first_value, first_lane);
            let y = lane(second_value, second_lane);
            let raw = if subtract { x - y } else { x + y };
            let value = match variant {
                ParallelVariant::Signed | ParallelVariant::Unsigned => {
                    let passes = if signed || subtract {
                        raw >= 0
                    } else {
                        raw >= 1 << lane_bits
                    };
                    if passes {
                        greater_or_equal |= if lane_bits == 8 {
                            1 << index
                        } else {
                            0b11 << (2 * index)
                        };
                    }
                    raw as u32
                }
                ParallelVariant::SignedSaturating => signed_saturate(raw, lane_bits).0 as u32,
                ParallelVariant::UnsignedSaturating => unsigned_saturate(raw, lane_bits).0,
                ParallelVariant::SignedHalving | ParallelVariant::UnsignedHalving => (raw >> 1) as u32,
            };
            result |= (value & lane_mask) << (index * lane_bits);
        }
        if matches!(variant, ParallelVariant::Signed | ParallelVariant::Unsigned) {
            self.flags.greater_or_equal = greater_or_equal;
        }
        self.write_register(destination, result);
        1
    }

    pub(crate) fn execute_select(&mut self, destination: u8, first: u8, second: u8) -> u32 {
        let first_value = self.read_register(first);
        let second_value = self.read_register(second);
        let mut result = 0;
        for byte in 0..4 {
            let mask = 0xFFu32 << (byte * 8);
            let chosen = if self.flags.greater_or_equal & (1 << byte) != 0 {
                first_value
            } else {
                second_value
            };
            result |= chosen & mask;
        }
        self.write_register(destination, result);
        1
    }

    pub(crate) fn execute_pack(
        &mut self,
        top_from_second: bool,
        destination: u8,
        first: u8,
        second: u8,
        amount: u8,
    ) -> u32 {
        let first_value = self.read_register(first);
        let second_value = self.read_register(second);
        let result = if top_from_second {
            let (shifted, _) = shift_with_carry(second_value, Shift::Asr, amount as u32, false);
            (first_value & 0xFFFF_0000) | (shifted & 0xFFFF)
        } else {
            let (shifted, _) = shift_with_carry(second_value, Shift::Lsl, amount as u32, false);
            (first_value & 0xFFFF) | (shifted & 0xFFFF_0000)
        };
        self.write_register(destination, result);
        1
    }

    // ========================================================================
    // DSP multiplies
    // ========================================================================

    pub(crate) fn execute_dsp_multiply(
        &mut self,
        operation: DspOperation,
        options: u8,
        destination: u8,
        first: u8,
        second: u8,
        accumulator: u8,
    ) -> u32 {
        use DspOperation::*;
        let first_value = self.read_register(first);
        let mut second_value = self.read_register(second);
        if options & dsp_options::EXCHANGE != 0 {
            second_value = second_value.rotate_right(16);
        }
        let first_top = options & dsp_options::FIRST_TOP != 0;
        let second_top = options & dsp_options::SECOND_TOP != 0;
        let accumulate = accumulator != 15;
        let addend = if accumulate {
            self.read_register(accumulator) as i32 as i64
        } else {
            0
        };
        let long_accumulator =
            ((self.read_register(destination) as u64) << 32 | self.read_register(accumulator) as u64) as i64;
        let low_products = signed_half(first_value, false) * signed_half(second_value, false);
        let high_products = signed_half(first_value, true) * signed_half(second_value, true);

        let checked_result = |cpu: &mut CortexM4, value: i64| {
            if value != value as i32 as i64 {
                cpu.flags.saturated = true;
            }
            cpu.write_register(destination, value as u32);
        };
        match operation {
            MultiplyHalves => {
                let product = signed_half(first_value, first_top) * signed_half(second_value, second_top);
                checked_result(self, product + addend);
            }
            MultiplyWordByHalf => {
                let product = (first_value as i32 as i64 * signed_half(second_value, second_top)) >> 16;
                checked_result(self, product + addend);
            }
            DualMultiplyAdd => checked_result(self, low_products + high_products + addend),
            DualMultiplySubtract => checked_result(self, low_products - high_products + addend),
            LongDualMultiplyAdd | LongDualMultiplySubtract | LongMultiplyHalves => {
                let product = match operation {
                    LongDualMultiplyAdd => low_products + high_products,
                    LongDualMultiplySubtract => low_products - high_products,
                    _ => signed_half(first_value, first_top) * signed_half(second_value, second_top),
                };
                let result = long_accumulator.wrapping_add(product) as u64;
                self.write_register(accumulator, result as u32);
                self.write_register(destination, (result >> 32) as u32);
            }
            MostSignificantMultiplyAdd | MostSignificantMultiplySubtract => {
                let product = first_value as i32 as i64 * second_value as i32 as i64;
                let base = addend << 32;
                let mut result = if operation == MostSignificantMultiplySubtract {
                    base.wrapping_sub(product)
                } else {
                    base.wrapping_add(product)
                };
                if options & dsp_options::ROUND != 0 {
                    result = result.wrapping_add(0x8000_0000);
                }
                self.write_register(destination, (result >> 32) as u32);
            }
            SumAbsoluteDifferences => {
                let mut sum = addend as u32;
                for byte in 0..4 {
                    let x = (first_value >> (byte * 8)) & 0xFF;
                    let y = (second_value >> (byte * 8)) & 0xFF;
                    sum = sum.wrapping_add(x.abs_diff(y));
                }
                self.write_register(destination, sum);
            }
        }
        1
    }
}
