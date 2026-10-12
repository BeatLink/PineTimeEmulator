// SPDX-License-Identifier: GPL-3.0-only
//! The FPv4-SP encodings (Armv7-M manual chapter A6), which use coprocessors 10 and 11.
//!
//! Double-precision arithmetic is undefined; moves, loads and stores of d0..d15 are not.

use super::*;

// ============================================================================
// Register fields
// ============================================================================

/// A single-precision register number from a 4-bit field and its extra low bit.
fn single(field: u32, extra_bit: bool) -> u8 {
    ((field << 1) | extra_bit as u32) as u8
}

/// The first single-precision register of a double-precision register, or `None` for d16..d31.
fn double(field: u32, extra_bit: bool) -> Option<u8> {
    if extra_bit {
        None
    } else {
        Some((field << 1) as u8)
    }
}

/// VFPExpandImm for single precision.
fn expand_immediate(imm8: u32) -> u32 {
    let sign = (imm8 >> 7) & 1;
    let b = (imm8 >> 6) & 1;
    let exponent_top = if b == 1 { 0b0_11111 } else { 0b1_00000 };
    (sign << 31) | (exponent_top << 25) | (((imm8 >> 4) & 3) << 23) | ((imm8 & 0xF) << 19)
}

// ============================================================================
// Top-level table
// ============================================================================

pub(super) fn decode(high: u32, low: u32) -> Instruction {
    if bits(high, 11, 9) == 0b110 {
        return load_store(high, low);
    }
    if bits(high, 11, 8) == 0b1110 {
        if bit(low, 4) {
            return register_transfer(high, low);
        }
        return data_processing(high, low);
    }
    Instruction::Undefined
}

// ============================================================================
// Loads, stores and 64-bit transfers
// ============================================================================

fn load_store(high: u32, low: u32) -> Instruction {
    let opcode = bits(high, 8, 4);
    let base = bits(high, 3, 0) as u8;
    let is_double = bit(low, 8);
    let d_bit = bit(high, 6);
    let vd = bits(low, 15, 12);
    if opcode & 0b11110 == 0b00100 {
        return pair_transfer(high, low);
    }
    let register = if is_double {
        match double(vd, d_bit) {
            Some(register) => register,
            None => return Instruction::Undefined,
        }
    } else {
        single(vd, d_bit)
    };
    let imm8 = bits(low, 7, 0);
    let index = bit(high, 8);
    let add = bit(high, 7);
    let writeback = bit(high, 5);
    let load = bit(high, 4);
    if index && !writeback {
        let magnitude = (imm8 << 2) as i32;
        let offset = if add { magnitude } else { -magnitude };
        return if load {
            Instruction::FloatLoad {
                register,
                double: is_double,
                base,
                offset,
            }
        } else {
            Instruction::FloatStore {
                register,
                double: is_double,
                base,
                offset,
            }
        };
    }
    let decrement_before = match (index, add, writeback) {
        (false, true, _) => false,
        (true, false, true) => true,
        _ => return Instruction::Undefined,
    };
    let count = if is_double { imm8 & !1 } else { imm8 };
    if count == 0 || register as u32 + count > 32 {
        return Instruction::Undefined;
    }
    if load {
        Instruction::FloatLoadMultiple {
            first_register: register,
            count: count as u8,
            base,
            writeback,
            decrement_before,
        }
    } else {
        Instruction::FloatStoreMultiple {
            first_register: register,
            count: count as u8,
            base,
            writeback,
            decrement_before,
        }
    }
}

fn pair_transfer(high: u32, low: u32) -> Instruction {
    if bits(low, 7, 6) != 0 {
        return Instruction::Undefined;
    }
    let vm = bits(low, 3, 0);
    let m_bit = bit(low, 5);
    let single_register = if bit(low, 8) {
        match double(vm, m_bit) {
            Some(register) => register,
            None => return Instruction::Undefined,
        }
    } else {
        let register = single(vm, m_bit);
        if register == 31 {
            return Instruction::Undefined;
        }
        register
    };
    Instruction::FloatMoveCorePair {
        to_core: bit(high, 4),
        first_core: bits(low, 15, 12) as u8,
        second_core: bits(high, 3, 0) as u8,
        single: single_register,
    }
}

// ============================================================================
// Core register transfers
// ============================================================================

fn register_transfer(high: u32, low: u32) -> Instruction {
    let to_core = bit(high, 4);
    let scalar = bit(low, 8);
    let a = bits(high, 7, 5);
    let core = bits(low, 15, 12) as u8;
    if bits(low, 3, 0) != 0 {
        return Instruction::Undefined;
    }
    if !scalar {
        return match a {
            0b000 => Instruction::FloatMoveCore {
                to_core,
                core,
                single: single(bits(high, 3, 0), bit(low, 7)),
            },
            0b111 if bits(high, 3, 0) == 0b0001 => Instruction::FloatStatusMove { to_core, core },
            _ => Instruction::Undefined,
        };
    }
    if bit(high, 7) || bit(high, 6) || bits(low, 6, 5) != 0 {
        return Instruction::Undefined;
    }
    let Some(first_single) = double(bits(high, 3, 0), bit(low, 7)) else {
        return Instruction::Undefined;
    };
    Instruction::FloatMoveCore {
        to_core,
        core,
        single: first_single + bit(high, 5) as u8,
    }
}

// ============================================================================
// Data processing
// ============================================================================

fn data_processing(high: u32, low: u32) -> Instruction {
    if bit(low, 8) {
        return Instruction::Undefined;
    }
    let opc1 = bits(high, 7, 4) & 0b1011;
    let opc2 = bits(high, 3, 0);
    let opc3 = bits(low, 7, 6);
    let destination = single(bits(low, 15, 12), bit(high, 6));
    let first = single(bits(high, 3, 0), bit(low, 7));
    let second = single(bits(low, 3, 0), bit(low, 5));
    let op = bit(low, 6);
    let arithmetic = |operation| Instruction::FloatArithmetic {
        operation,
        destination,
        first,
        second,
    };
    use FloatOperation::*;
    match opc1 {
        0b0000 => arithmetic(if op { MultiplySubtract } else { MultiplyAccumulate }),
        0b0001 => arithmetic(if op {
            NegateMultiplyAccumulate
        } else {
            NegateMultiplySubtract
        }),
        0b0010 => arithmetic(if op { NegateMultiply } else { Multiply }),
        0b0011 => arithmetic(if op { Subtract } else { Add }),
        0b1000 if !op => arithmetic(Divide),
        0b1001 => arithmetic(if op {
            FusedNegateMultiplyAccumulate
        } else {
            FusedNegateMultiplySubtract
        }),
        0b1010 => arithmetic(if op {
            FusedMultiplySubtract
        } else {
            FusedMultiplyAccumulate
        }),
        0b1011 => other_data_processing(high, low, opc2, opc3, destination, second),
        _ => Instruction::Undefined,
    }
}

fn other_data_processing(
    high: u32,
    low: u32,
    opc2: u32,
    opc3: u32,
    destination: u8,
    source: u8,
) -> Instruction {
    if opc3 & 1 == 0 {
        if bits(low, 7, 4) != 0 {
            return Instruction::Undefined;
        }
        return Instruction::FloatMoveImmediate {
            destination,
            value: expand_immediate((bits(high, 3, 0) << 4) | bits(low, 3, 0)),
        };
    }
    let top_bit = bit(low, 7);
    let unary = |operation| Instruction::FloatUnary {
        operation,
        destination,
        source,
    };
    match opc2 {
        0b0000 if opc3 == 0b01 => unary(FloatUnary::Move),
        0b0000 => unary(FloatUnary::Absolute),
        0b0001 if opc3 == 0b01 => unary(FloatUnary::Negate),
        0b0001 => unary(FloatUnary::SquareRoot),
        0b0010 | 0b0011 => Instruction::FloatConvertHalf {
            to_half: opc2 & 1 != 0,
            top: top_bit,
            destination,
            source,
        },
        0b0100 | 0b0101 => {
            let with_zero = opc2 & 1 != 0;
            if with_zero && bits(low, 5, 0) != 0 {
                return Instruction::Undefined;
            }
            Instruction::FloatCompare {
                first: destination,
                second: source,
                with_zero,
                signal_quiet_nan: top_bit,
            }
        }
        0b1000 => Instruction::FloatConvert {
            conversion: if top_bit {
                FloatConversion::SignedToFloat
            } else {
                FloatConversion::UnsignedToFloat
            },
            destination,
            source,
            round_to_zero: false,
        },
        0b1100 | 0b1101 => Instruction::FloatConvert {
            conversion: if opc2 & 1 != 0 {
                FloatConversion::FloatToSigned
            } else {
                FloatConversion::FloatToUnsigned
            },
            destination,
            source,
            round_to_zero: top_bit,
        },
        0b1010 | 0b1011 | 0b1110 | 0b1111 => {
            let halfword = !top_bit;
            let size = if halfword { 16 } else { 32 };
            let immediate = (bits(low, 3, 0) << 1) | bits(low, 5, 5);
            if halfword && immediate > 16 {
                return Instruction::Undefined;
            }
            Instruction::FloatConvertFixed {
                to_fixed: opc2 & 0b0100 != 0,
                signed: opc2 & 1 == 0,
                halfword,
                register: destination,
                fraction_bits: (size - immediate) as u8,
            }
        }
        _ => Instruction::Undefined,
    }
}
