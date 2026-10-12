// SPDX-License-Identifier: GPL-3.0-only
//! The 32-bit Thumb-2 encodings (Armv7-M manual section A5.3), except the FPU's.

use super::floating_point;
use super::thumb16::hint_kind;
use super::*;

// ============================================================================
// Top-level table
// ============================================================================

pub(super) fn decode(address: u32, first: u16, second: u16) -> (Instruction, u8) {
    let high = first as u32;
    let low = second as u32;
    let op1 = bits(high, 12, 11);
    let op2 = bits(high, 10, 4);
    let instruction = match op1 {
        0b01 => {
            if op2 & 0b1100100 == 0b0000000 {
                load_store_multiple(high, low)
            } else if op2 & 0b1100100 == 0b0000100 {
                load_store_dual_exclusive_table(high, low)
            } else if op2 & 0b1100000 == 0b0100000 {
                data_processing_shifted_register(high, low)
            } else {
                coprocessor(high, low)
            }
        }
        0b10 => {
            if bit(low, 15) {
                return branches_and_miscellaneous(address, high, low);
            } else if op2 & 0b0100000 == 0 {
                data_processing_modified_immediate(high, low)
            } else {
                data_processing_plain_immediate(address, high, low)
            }
        }
        _ => {
            if op2 & 0b1110001 == 0b0000000 {
                store_single(high, low)
            } else if op2 & 0b1100111 == 0b0000001 {
                load_single(address, high, low, LoadWidth::Byte)
            } else if op2 & 0b1100111 == 0b0000011 {
                load_single(address, high, low, LoadWidth::Halfword)
            } else if op2 & 0b1100111 == 0b0000101 {
                load_single(address, high, low, LoadWidth::Word)
            } else if op2 & 0b1110000 == 0b0100000 {
                data_processing_register(high, low)
            } else if op2 & 0b1111000 == 0b0110000 {
                multiply(high, low)
            } else if op2 & 0b1111000 == 0b0111000 {
                long_multiply_and_divide(high, low)
            } else if op2 & 0b1000000 != 0 {
                coprocessor(high, low)
            } else {
                Instruction::Undefined
            }
        }
    };
    (instruction, CONDITION_ALWAYS)
}

fn register_at(word: u32, low_bit: u32) -> u8 {
    bits(word, low_bit + 3, low_bit) as u8
}

// ============================================================================
// Load and store multiple
// ============================================================================

fn load_store_multiple(high: u32, low: u32) -> Instruction {
    let base = register_at(high, 0);
    let writeback = bit(high, 5);
    let registers = low as u16;
    let load = bit(high, 4);
    let decrement_before = match bits(high, 8, 7) {
        0b01 => false,
        0b10 => true,
        _ => return Instruction::Undefined,
    };
    if load {
        Instruction::LoadMultiple {
            base,
            registers,
            writeback,
            decrement_before,
        }
    } else {
        Instruction::StoreMultiple {
            base,
            registers,
            writeback,
            decrement_before,
        }
    }
}

// ============================================================================
// Dual and exclusive loads and stores, table branch
// ============================================================================

fn load_store_dual_exclusive_table(high: u32, low: u32) -> Instruction {
    let base = register_at(high, 0);
    let target = register_at(low, 12);
    let second_target = register_at(low, 8);
    let op1 = bits(high, 8, 7);
    let op2 = bits(high, 5, 4);
    let op3 = bits(low, 7, 4);
    match (op1, op2) {
        (0b00, 0b00) => Instruction::StoreExclusive {
            size: StoreSize::Word,
            status: second_target,
            source: target,
            base,
            offset: bits(low, 7, 0) << 2,
        },
        (0b00, 0b01) => Instruction::LoadExclusive {
            size: StoreSize::Word,
            target,
            base,
            offset: bits(low, 7, 0) << 2,
        },
        (0b01, 0b00) => match op3 {
            0b0100 | 0b0101 => Instruction::StoreExclusive {
                size: if op3 == 0b0100 {
                    StoreSize::Byte
                } else {
                    StoreSize::Halfword
                },
                status: register_at(low, 0),
                source: target,
                base,
                offset: 0,
            },
            _ => Instruction::Undefined,
        },
        (0b01, 0b01) => match op3 {
            0b0000 | 0b0001 => Instruction::TableBranch {
                halfword: op3 == 0b0001,
                base,
                index: register_at(low, 0),
            },
            0b0100 | 0b0101 => Instruction::LoadExclusive {
                size: if op3 == 0b0100 {
                    StoreSize::Byte
                } else {
                    StoreSize::Halfword
                },
                target,
                base,
                offset: 0,
            },
            _ => Instruction::Undefined,
        },
        _ => {
            let index = bit(high, 8);
            let add = bit(high, 7);
            let writeback = bit(high, 5);
            let magnitude = (bits(low, 7, 0) << 2) as i32;
            let offset = if add { magnitude } else { -magnitude };
            if bit(high, 4) {
                Instruction::LoadDual {
                    first: target,
                    second: second_target,
                    base,
                    offset,
                    index,
                    writeback,
                }
            } else {
                Instruction::StoreDual {
                    first: target,
                    second: second_target,
                    base,
                    offset,
                    index,
                    writeback,
                }
            }
        }
    }
}

// ============================================================================
// Data processing
// ============================================================================

/// The operation an opcode names, with register 15 turning some into tests, compares or moves.
fn data_operation(opcode: u32, destination: u8, first: u8, set_flags: bool) -> Option<DataOperation> {
    let test = destination == 15 && set_flags;
    Some(match opcode {
        0b0000 if test => DataOperation::Tst,
        0b0000 => DataOperation::And,
        0b0001 => DataOperation::Bic,
        0b0010 if first == 15 => DataOperation::Mov,
        0b0010 => DataOperation::Orr,
        0b0011 if first == 15 => DataOperation::Mvn,
        0b0011 => DataOperation::Orn,
        0b0100 if test => DataOperation::Teq,
        0b0100 => DataOperation::Eor,
        0b1000 if test => DataOperation::Cmn,
        0b1000 => DataOperation::Add,
        0b1010 => DataOperation::Adc,
        0b1011 => DataOperation::Sbc,
        0b1101 if test => DataOperation::Cmp,
        0b1101 => DataOperation::Sub,
        0b1110 => DataOperation::Rsb,
        _ => return None,
    })
}

fn flag_mode(set_flags: bool) -> FlagMode {
    if set_flags {
        FlagMode::Always
    } else {
        FlagMode::Never
    }
}

fn data_processing_shifted_register(high: u32, low: u32) -> Instruction {
    let opcode = bits(high, 8, 5);
    let set_flags = bit(high, 4);
    let first = register_at(high, 0);
    let destination = register_at(low, 8);
    let second = register_at(low, 0);
    let amount = (bits(low, 14, 12) << 2) | bits(low, 7, 6);
    let shift_type = bits(low, 5, 4);
    if opcode == 0b0110 {
        if set_flags || shift_type & 1 != 0 {
            return Instruction::Undefined;
        }
        let top_from_second = bit(low, 5);
        let (_, amount) = decode_immediate_shift(shift_type, amount);
        return Instruction::Pack {
            top_from_second,
            destination,
            first,
            second,
            amount,
        };
    }
    let Some(operation) = data_operation(opcode, destination, first, set_flags) else {
        return Instruction::Undefined;
    };
    let (shift, amount) = decode_immediate_shift(shift_type, amount);
    Instruction::DataRegister {
        operation,
        flags: flag_mode(set_flags),
        destination,
        first,
        second,
        shift,
        amount,
    }
}

fn data_processing_modified_immediate(high: u32, low: u32) -> Instruction {
    let opcode = bits(high, 8, 5);
    let set_flags = bit(high, 4);
    let first = register_at(high, 0);
    let destination = register_at(low, 8);
    let imm12 = (bits(high, 10, 10) << 11) | (bits(low, 14, 12) << 8) | bits(low, 7, 0);
    let Some(operation) = data_operation(opcode, destination, first, set_flags) else {
        return Instruction::Undefined;
    };
    let (immediate, carry) = thumb_expand_immediate(imm12);
    Instruction::DataImmediate {
        operation,
        flags: flag_mode(set_flags),
        destination,
        first,
        immediate,
        carry,
    }
}

fn data_processing_plain_immediate(address: u32, high: u32, low: u32) -> Instruction {
    let first = register_at(high, 0);
    let destination = register_at(low, 8);
    let imm12 = (bits(high, 10, 10) << 11) | (bits(low, 14, 12) << 8) | bits(low, 7, 0);
    let imm16 = (bits(high, 3, 0) << 12) | imm12;
    let shift_amount = ((bits(low, 14, 12) << 2) | bits(low, 7, 6)) as u8;
    let field_bits = bits(low, 4, 0) as u8;
    let plain_add_subtract = |operation| Instruction::DataImmediate {
        operation,
        flags: FlagMode::Never,
        destination,
        first,
        immediate: imm12,
        carry: ImmediateCarry::Unchanged,
    };
    match bits(high, 8, 4) {
        0b00000 if first == 15 => Instruction::Address {
            destination,
            address: ((address + 4) & !3).wrapping_add(imm12),
        },
        0b00000 => plain_add_subtract(DataOperation::Add),
        0b00100 => Instruction::DataImmediate {
            operation: DataOperation::Mov,
            flags: FlagMode::Never,
            destination,
            first: 0,
            immediate: imm16,
            carry: ImmediateCarry::Unchanged,
        },
        0b01010 if first == 15 => Instruction::Address {
            destination,
            address: ((address + 4) & !3).wrapping_sub(imm12),
        },
        0b01010 => plain_add_subtract(DataOperation::Sub),
        0b01100 => Instruction::MoveTop {
            destination,
            immediate: imm16 as u16,
        },
        opcode @ (0b10000 | 0b10010 | 0b11000 | 0b11010) => {
            let signed = opcode & 0b01000 == 0;
            let arithmetic_shift = opcode & 0b00010 != 0;
            let halfwords = arithmetic_shift && shift_amount == 0;
            let field = if halfwords { field_bits & 0xF } else { field_bits };
            let bits_to_keep = field + signed as u8;
            Instruction::Saturate {
                signed,
                halfwords,
                destination,
                source: first,
                bits: bits_to_keep,
                shift: if arithmetic_shift { Shift::Asr } else { Shift::Lsl },
                amount: shift_amount,
            }
        }
        0b10100 | 0b11100 => Instruction::BitFieldExtract {
            signed: !bit(high, 7),
            destination,
            source: first,
            lsb: shift_amount,
            width: field_bits + 1,
        },
        0b10110 if first == 15 => Instruction::BitFieldClear {
            destination,
            lsb: shift_amount,
            msb: field_bits,
        },
        0b10110 => Instruction::BitFieldInsert {
            destination,
            source: first,
            lsb: shift_amount,
            msb: field_bits,
        },
        _ => Instruction::Undefined,
    }
}

// ============================================================================
// Branches and miscellaneous control
// ============================================================================

fn branches_and_miscellaneous(address: u32, high: u32, low: u32) -> (Instruction, u8) {
    let op1 = bits(low, 14, 12);
    let op = bits(high, 10, 4);
    let sign = bits(high, 10, 10);
    let j1 = bits(low, 13, 13);
    let j2 = bits(low, 11, 11);
    let always = CONDITION_ALWAYS;
    if op1 & 0b101 == 0b000 {
        if op & 0b0111000 != 0b0111000 {
            let offset =
                (sign << 20) | (j2 << 19) | (j1 << 18) | (bits(high, 5, 0) << 12) | (bits(low, 10, 0) << 1);
            let target = address
                .wrapping_add(4)
                .wrapping_add(sign_extend(offset, 21) as u32);
            return (Instruction::Branch { target }, bits(high, 9, 6) as u8);
        }
        let instruction = match op {
            0b0111000 | 0b0111001 => Instruction::MoveToSpecial {
                source: register_at(high, 0),
                special: bits(low, 7, 0) as u8,
                mask: bits(low, 11, 10) as u8,
            },
            0b0111010 => {
                if bits(low, 10, 8) != 0 {
                    Instruction::Undefined
                } else {
                    let option = bits(low, 7, 0);
                    Instruction::Hint {
                        kind: if option & 0xF0 == 0xF0 {
                            HintKind::Nop
                        } else {
                            hint_kind(option)
                        },
                    }
                }
            }
            0b0111011 => match bits(low, 7, 4) {
                0b0010 => Instruction::ClearExclusive,
                0b0100..=0b0110 => Instruction::Hint {
                    kind: HintKind::Barrier,
                },
                _ => Instruction::Undefined,
            },
            0b0111110 | 0b0111111 => Instruction::MoveFromSpecial {
                destination: register_at(low, 8),
                special: bits(low, 7, 0) as u8,
            },
            _ => Instruction::Undefined,
        };
        return (instruction, always);
    }
    let first_bit = !(j1 ^ sign) & 1;
    let second_bit = !(j2 ^ sign) & 1;
    let offset = (sign << 24)
        | (first_bit << 23)
        | (second_bit << 22)
        | (bits(high, 9, 0) << 12)
        | (bits(low, 10, 0) << 1);
    let target = address
        .wrapping_add(4)
        .wrapping_add(sign_extend(offset, 25) as u32);
    let instruction = match op1 {
        0b001 | 0b011 => Instruction::Branch { target },
        0b101 | 0b111 => Instruction::BranchLink { target },
        _ => Instruction::Undefined,
    };
    (instruction, always)
}

// ============================================================================
// Single loads and stores
// ============================================================================

fn store_single(high: u32, low: u32) -> Instruction {
    let base = register_at(high, 0);
    let source = register_at(low, 12);
    if base == 15 {
        return Instruction::Undefined;
    }
    let size = match bits(high, 6, 5) {
        0b00 => StoreSize::Byte,
        0b01 => StoreSize::Halfword,
        0b10 => StoreSize::Word,
        _ => return Instruction::Undefined,
    };
    if bit(high, 7) {
        return Instruction::StoreImmediate {
            size,
            source,
            base,
            offset: bits(low, 11, 0) as i32,
            index: true,
            writeback: false,
        };
    }
    if bit(low, 11) {
        let (offset, index, writeback) = immediate8_addressing(low);
        if !index && !writeback {
            return Instruction::Undefined;
        }
        return Instruction::StoreImmediate {
            size,
            source,
            base,
            offset,
            index,
            writeback,
        };
    }
    if bits(low, 10, 6) != 0 {
        return Instruction::Undefined;
    }
    Instruction::StoreRegister {
        size,
        source,
        base,
        offset_register: register_at(low, 0),
        shift: bits(low, 5, 4) as u8,
    }
}

/// The offset, pre-index flag and writeback flag of the 8-bit immediate load and store forms.
fn immediate8_addressing(low: u32) -> (i32, bool, bool) {
    let magnitude = bits(low, 7, 0) as i32;
    let offset = if bit(low, 9) { magnitude } else { -magnitude };
    (offset, bit(low, 10), bit(low, 8))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadWidth {
    Byte,
    Halfword,
    Word,
}

fn load_single(address: u32, high: u32, low: u32, width: LoadWidth) -> Instruction {
    let base = register_at(high, 0);
    let target = register_at(low, 12);
    let signed = bit(high, 8);
    let kind = match (width, signed) {
        (LoadWidth::Byte, false) => LoadKind::Byte,
        (LoadWidth::Byte, true) => LoadKind::SignedByte,
        (LoadWidth::Halfword, false) => LoadKind::Halfword,
        (LoadWidth::Halfword, true) => LoadKind::SignedHalfword,
        (LoadWidth::Word, false) => LoadKind::Word,
        (LoadWidth::Word, true) => return Instruction::Undefined,
    };
    let preload_hint = target == 15 && width != LoadWidth::Word;
    if base == 15 {
        if preload_hint {
            return Instruction::Hint {
                kind: HintKind::Preload,
            };
        }
        let magnitude = bits(low, 11, 0);
        let literal_base = (address + 4) & !3;
        let literal = if bit(high, 7) {
            literal_base.wrapping_add(magnitude)
        } else {
            literal_base.wrapping_sub(magnitude)
        };
        return Instruction::LoadLiteral {
            kind,
            target,
            address: literal,
        };
    }
    if bit(high, 7) {
        if preload_hint {
            return Instruction::Hint {
                kind: HintKind::Preload,
            };
        }
        return Instruction::LoadImmediate {
            kind,
            target,
            base,
            offset: bits(low, 11, 0) as i32,
            index: true,
            writeback: false,
        };
    }
    if bit(low, 11) {
        let (offset, index, writeback) = immediate8_addressing(low);
        if !index && !writeback {
            return Instruction::Undefined;
        }
        if preload_hint && index && !writeback && offset < 0 {
            return Instruction::Hint {
                kind: HintKind::Preload,
            };
        }
        return Instruction::LoadImmediate {
            kind,
            target,
            base,
            offset,
            index,
            writeback,
        };
    }
    if bits(low, 10, 6) != 0 {
        return Instruction::Undefined;
    }
    if preload_hint {
        return Instruction::Hint {
            kind: HintKind::Preload,
        };
    }
    Instruction::LoadRegister {
        kind,
        target,
        base,
        offset_register: register_at(low, 0),
        shift: bits(low, 5, 4) as u8,
    }
}

// ============================================================================
// Data processing on registers
// ============================================================================

fn data_processing_register(high: u32, low: u32) -> Instruction {
    let op1 = bits(high, 7, 4);
    let op2 = bits(low, 7, 4);
    let first = register_at(high, 0);
    let destination = register_at(low, 8);
    let second = register_at(low, 0);
    if bits(low, 15, 12) != 0b1111 {
        return Instruction::Undefined;
    }
    if op2 == 0 && op1 & 0b1000 == 0 {
        let shift = match op1 >> 1 {
            0 => Shift::Lsl,
            1 => Shift::Lsr,
            2 => Shift::Asr,
            _ => Shift::Ror,
        };
        return Instruction::ShiftByRegister {
            shift,
            flags: flag_mode(op1 & 1 != 0),
            destination,
            value: first,
            amount: second,
        };
    }
    if op1 & 0b1000 == 0 && op2 & 0b1000 != 0 {
        let kind = match op1 {
            0b0000 => ExtendKind::SignedHalfword,
            0b0001 => ExtendKind::UnsignedHalfword,
            0b0010 => ExtendKind::SignedBytePair,
            0b0011 => ExtendKind::UnsignedBytePair,
            0b0100 => ExtendKind::SignedByte,
            0b0101 => ExtendKind::UnsignedByte,
            _ => return Instruction::Undefined,
        };
        return Instruction::Extend {
            kind,
            destination,
            addend: first,
            source: second,
            rotation: (bits(low, 5, 4) * 8) as u8,
        };
    }
    if op1 & 0b1000 != 0 && op2 & 0b1000 == 0 {
        let operation = match op1 & 0b111 {
            0b001 => ParallelOperation::Add16,
            0b010 => ParallelOperation::AddSubtractExchange,
            0b110 => ParallelOperation::SubtractAddExchange,
            0b101 => ParallelOperation::Subtract16,
            0b000 => ParallelOperation::Add8,
            0b100 => ParallelOperation::Subtract8,
            _ => return Instruction::Undefined,
        };
        let variant = match op2 {
            0b0000 => ParallelVariant::Signed,
            0b0001 => ParallelVariant::SignedSaturating,
            0b0010 => ParallelVariant::SignedHalving,
            0b0100 => ParallelVariant::Unsigned,
            0b0101 => ParallelVariant::UnsignedSaturating,
            0b0110 => ParallelVariant::UnsignedHalving,
            _ => return Instruction::Undefined,
        };
        return Instruction::Parallel {
            operation,
            variant,
            destination,
            first,
            second,
        };
    }
    if op1 & 0b1100 == 0b1000 && op2 & 0b1100 == 0b1000 {
        let selector = ((op1 & 0b11) << 2) | (op2 & 0b11);
        let saturating = |operation| Instruction::SaturatingArithmetic {
            operation,
            destination,
            first: second,
            second: first,
        };
        let reverse = |kind| Instruction::ByteReverse {
            kind,
            destination,
            source: second,
        };
        return match selector {
            0b0000 => saturating(SaturatingOperation::Add),
            0b0001 => saturating(SaturatingOperation::DoubleAdd),
            0b0010 => saturating(SaturatingOperation::Subtract),
            0b0011 => saturating(SaturatingOperation::DoubleSubtract),
            0b0100 => reverse(ReverseKind::Word),
            0b0101 => reverse(ReverseKind::PackedHalfword),
            0b0110 => Instruction::ReverseBits {
                destination,
                source: second,
            },
            0b0111 => reverse(ReverseKind::SignedHalfword),
            0b1000 => Instruction::Select {
                destination,
                first,
                second,
            },
            0b1100 => Instruction::CountLeadingZeros {
                destination,
                source: second,
            },
            _ => Instruction::Undefined,
        };
    }
    Instruction::Undefined
}

// ============================================================================
// Multiplies
// ============================================================================

fn multiply(high: u32, low: u32) -> Instruction {
    let op1 = bits(high, 6, 4);
    let op2 = bits(low, 5, 4);
    let first = register_at(high, 0);
    let accumulator = register_at(low, 12);
    let destination = register_at(low, 8);
    let second = register_at(low, 0);
    if bits(low, 7, 6) != 0 {
        return Instruction::Undefined;
    }
    let dsp = |operation, options| Instruction::DspMultiply {
        operation,
        options,
        destination,
        first,
        second,
        accumulator,
    };
    let low_bit = (op2 & 1) as u8;
    match (op1, op2) {
        (0b000, 0b00) if accumulator == 15 => Instruction::Multiply {
            flags: FlagMode::Never,
            destination,
            first,
            second,
        },
        (0b000, 0b00) | (0b000, 0b01) => Instruction::MultiplyAccumulate {
            subtract: op2 == 0b01,
            destination,
            first,
            second,
            accumulator,
        },
        (0b001, _) => dsp(DspOperation::MultiplyHalves, (op2 >> 1) as u8 | (low_bit << 1)),
        (0b010, 0b00 | 0b01) => dsp(DspOperation::DualMultiplyAdd, low_bit * dsp_options::EXCHANGE),
        (0b011, 0b00 | 0b01) => dsp(
            DspOperation::MultiplyWordByHalf,
            low_bit * dsp_options::SECOND_TOP,
        ),
        (0b100, 0b00 | 0b01) => dsp(
            DspOperation::DualMultiplySubtract,
            low_bit * dsp_options::EXCHANGE,
        ),
        (0b101, 0b00 | 0b01) => dsp(
            DspOperation::MostSignificantMultiplyAdd,
            low_bit * dsp_options::ROUND,
        ),
        (0b110, 0b00 | 0b01) if accumulator != 15 => dsp(
            DspOperation::MostSignificantMultiplySubtract,
            low_bit * dsp_options::ROUND,
        ),
        (0b111, 0b00) => dsp(DspOperation::SumAbsoluteDifferences, 0),
        _ => Instruction::Undefined,
    }
}

fn long_multiply_and_divide(high: u32, low: u32) -> Instruction {
    let op1 = bits(high, 6, 4);
    let op2 = bits(low, 7, 4);
    let first = register_at(high, 0);
    let low_register = register_at(low, 12);
    let high_register = register_at(low, 8);
    let second = register_at(low, 0);
    let long = |operation| Instruction::MultiplyLong {
        operation,
        low: low_register,
        high: high_register,
        first,
        second,
    };
    let dsp = |operation, options| Instruction::DspMultiply {
        operation,
        options,
        destination: high_register,
        first,
        second,
        accumulator: low_register,
    };
    match (op1, op2) {
        (0b000, 0b0000) => long(LongMultiply::SignedMultiply),
        (0b001, 0b1111) | (0b011, 0b1111) if low_register == 15 => Instruction::Divide {
            signed: op1 == 0b001,
            destination: high_register,
            dividend: first,
            divisor: second,
        },
        (0b010, 0b0000) => long(LongMultiply::UnsignedMultiply),
        (0b100, 0b0000) => long(LongMultiply::SignedAccumulate),
        (0b100, 0b1000..=0b1011) => dsp(
            DspOperation::LongMultiplyHalves,
            (bits(op2, 1, 1) as u8) | ((op2 & 1) as u8) << 1,
        ),
        (0b100, 0b1100 | 0b1101) => dsp(
            DspOperation::LongDualMultiplyAdd,
            (op2 & 1) as u8 * dsp_options::EXCHANGE,
        ),
        (0b101, 0b1100 | 0b1101) => dsp(
            DspOperation::LongDualMultiplySubtract,
            (op2 & 1) as u8 * dsp_options::EXCHANGE,
        ),
        (0b110, 0b0000) => long(LongMultiply::UnsignedAccumulate),
        (0b110, 0b0110) => long(LongMultiply::UnsignedDoubleAccumulate),
        _ => Instruction::Undefined,
    }
}

// ============================================================================
// Coprocessor space
// ============================================================================

fn coprocessor(high: u32, low: u32) -> Instruction {
    let coprocessor_number = bits(low, 11, 8);
    let is_floating_point = coprocessor_number & 0b1110 == 0b1010;
    if is_floating_point && !bit(high, 12) {
        return floating_point::decode(high, low);
    }
    if is_floating_point {
        return Instruction::Undefined;
    }
    Instruction::NoCoprocessor
}
