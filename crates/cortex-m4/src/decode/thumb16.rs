// SPDX-License-Identifier: GPL-3.0-only
//! The 16-bit Thumb encodings (Armv7-M manual section A5.2).

use super::*;

// ============================================================================
// Top-level table
// ============================================================================

pub(super) fn decode(address: u32, halfword: u16) -> (Instruction, u8) {
    let word = halfword as u32;
    let instruction = match word >> 10 {
        0b000000..=0b001111 => shift_add_subtract_move_compare(word),
        0b010000 => data_processing(word),
        0b010001 => special_data_and_branch(word),
        0b010010 | 0b010011 => Instruction::LoadLiteral {
            kind: LoadKind::Word,
            target: bits(word, 10, 8) as u8,
            address: ((address + 4) & !3) + (bits(word, 7, 0) << 2),
        },
        0b010100..=0b100111 => load_store_single(word),
        0b101000 | 0b101001 => Instruction::Address {
            destination: bits(word, 10, 8) as u8,
            address: ((address + 4) & !3) + (bits(word, 7, 0) << 2),
        },
        0b101010 | 0b101011 => Instruction::DataImmediate {
            operation: DataOperation::Add,
            flags: FlagMode::Never,
            destination: bits(word, 10, 8) as u8,
            first: 13,
            immediate: bits(word, 7, 0) << 2,
            carry: ImmediateCarry::Unchanged,
        },
        0b101100..=0b101111 => return miscellaneous(address, word),
        0b110000 | 0b110001 => {
            let base = bits(word, 10, 8) as u8;
            Instruction::StoreMultiple {
                base,
                registers: bits(word, 7, 0) as u16,
                writeback: true,
                decrement_before: false,
            }
        }
        0b110010 | 0b110011 => {
            let base = bits(word, 10, 8) as u8;
            let registers = bits(word, 7, 0) as u16;
            Instruction::LoadMultiple {
                base,
                registers,
                writeback: registers & (1 << base) == 0,
                decrement_before: false,
            }
        }
        0b110100..=0b110111 => return conditional_branch_and_supervisor_call(address, word),
        0b111000 | 0b111001 => Instruction::Branch {
            target: address
                .wrapping_add(4)
                .wrapping_add((sign_extend(bits(word, 10, 0), 11) << 1) as u32),
        },
        _ => Instruction::Undefined,
    };
    (instruction, CONDITION_ALWAYS)
}

// ============================================================================
// Shifts, adds, subtracts, moves and compares
// ============================================================================

fn shift_add_subtract_move_compare(word: u32) -> Instruction {
    let low_destination = bits(word, 2, 0) as u8;
    let low_source = bits(word, 5, 3) as u8;
    let opcode = bits(word, 13, 9);
    match opcode {
        0b00000..=0b01011 => {
            let shift_type = bits(word, 12, 11);
            let (shift, amount) = decode_immediate_shift(shift_type, bits(word, 10, 6));
            Instruction::DataRegister {
                operation: DataOperation::Mov,
                flags: FlagMode::OutsideItBlock,
                destination: low_destination,
                first: 0,
                second: low_source,
                shift,
                amount,
            }
        }
        0b01100 | 0b01101 => Instruction::DataRegister {
            operation: if opcode == 0b01100 {
                DataOperation::Add
            } else {
                DataOperation::Sub
            },
            flags: FlagMode::OutsideItBlock,
            destination: low_destination,
            first: low_source,
            second: bits(word, 8, 6) as u8,
            shift: Shift::Lsl,
            amount: 0,
        },
        0b01110 | 0b01111 => Instruction::DataImmediate {
            operation: if opcode == 0b01110 {
                DataOperation::Add
            } else {
                DataOperation::Sub
            },
            flags: FlagMode::OutsideItBlock,
            destination: low_destination,
            first: low_source,
            immediate: bits(word, 8, 6),
            carry: ImmediateCarry::Unchanged,
        },
        _ => {
            let register = bits(word, 10, 8) as u8;
            let (operation, flags) = match bits(word, 12, 11) {
                0 => (DataOperation::Mov, FlagMode::OutsideItBlock),
                1 => (DataOperation::Cmp, FlagMode::Always),
                2 => (DataOperation::Add, FlagMode::OutsideItBlock),
                _ => (DataOperation::Sub, FlagMode::OutsideItBlock),
            };
            Instruction::DataImmediate {
                operation,
                flags,
                destination: register,
                first: register,
                immediate: bits(word, 7, 0),
                carry: ImmediateCarry::Unchanged,
            }
        }
    }
}

// ============================================================================
// Data processing on low registers
// ============================================================================

fn data_processing(word: u32) -> Instruction {
    let destination = bits(word, 2, 0) as u8;
    let source = bits(word, 5, 3) as u8;
    let register_operation = |operation, flags| Instruction::DataRegister {
        operation,
        flags,
        destination,
        first: destination,
        second: source,
        shift: Shift::Lsl,
        amount: 0,
    };
    let shift_by_register = |shift| Instruction::ShiftByRegister {
        shift,
        flags: FlagMode::OutsideItBlock,
        destination,
        value: destination,
        amount: source,
    };
    let outside = FlagMode::OutsideItBlock;
    match bits(word, 9, 6) {
        0b0000 => register_operation(DataOperation::And, outside),
        0b0001 => register_operation(DataOperation::Eor, outside),
        0b0010 => shift_by_register(Shift::Lsl),
        0b0011 => shift_by_register(Shift::Lsr),
        0b0100 => shift_by_register(Shift::Asr),
        0b0101 => register_operation(DataOperation::Adc, outside),
        0b0110 => register_operation(DataOperation::Sbc, outside),
        0b0111 => shift_by_register(Shift::Ror),
        0b1000 => register_operation(DataOperation::Tst, FlagMode::Always),
        0b1001 => Instruction::DataImmediate {
            operation: DataOperation::Rsb,
            flags: outside,
            destination,
            first: source,
            immediate: 0,
            carry: ImmediateCarry::Unchanged,
        },
        0b1010 => register_operation(DataOperation::Cmp, FlagMode::Always),
        0b1011 => register_operation(DataOperation::Cmn, FlagMode::Always),
        0b1100 => register_operation(DataOperation::Orr, outside),
        0b1101 => Instruction::Multiply {
            flags: outside,
            destination,
            first: source,
            second: destination,
        },
        0b1110 => register_operation(DataOperation::Bic, outside),
        _ => Instruction::DataRegister {
            operation: DataOperation::Mvn,
            flags: outside,
            destination,
            first: 0,
            second: source,
            shift: Shift::Lsl,
            amount: 0,
        },
    }
}

// ============================================================================
// High-register operations and branch exchange
// ============================================================================

fn special_data_and_branch(word: u32) -> Instruction {
    let destination = ((bit(word, 7) as u8) << 3) | bits(word, 2, 0) as u8;
    let source = bits(word, 6, 3) as u8;
    let operation = match bits(word, 9, 8) {
        0b00 => DataOperation::Add,
        0b01 => DataOperation::Cmp,
        0b10 => DataOperation::Mov,
        _ => {
            return if bit(word, 7) {
                Instruction::BranchLinkExchange { source }
            } else {
                Instruction::BranchExchange { source }
            }
        }
    };
    Instruction::DataRegister {
        operation,
        flags: if operation == DataOperation::Cmp {
            FlagMode::Always
        } else {
            FlagMode::Never
        },
        destination,
        first: destination,
        second: source,
        shift: Shift::Lsl,
        amount: 0,
    }
}

// ============================================================================
// Single loads and stores
// ============================================================================

fn load_store_single(word: u32) -> Instruction {
    let target = bits(word, 2, 0) as u8;
    let base = bits(word, 5, 3) as u8;
    let immediate5 = bits(word, 10, 6) as i32;
    let store_immediate = |size, offset| Instruction::StoreImmediate {
        size,
        source: target,
        base,
        offset,
        index: true,
        writeback: false,
    };
    let load_immediate = |kind, offset| Instruction::LoadImmediate {
        kind,
        target,
        base,
        offset,
        index: true,
        writeback: false,
    };
    match word >> 12 {
        0b0101 => {
            let offset_register = bits(word, 8, 6) as u8;
            let store = |size| Instruction::StoreRegister {
                size,
                source: target,
                base,
                offset_register,
                shift: 0,
            };
            let load = |kind| Instruction::LoadRegister {
                kind,
                target,
                base,
                offset_register,
                shift: 0,
            };
            match bits(word, 11, 9) {
                0 => store(StoreSize::Word),
                1 => store(StoreSize::Halfword),
                2 => store(StoreSize::Byte),
                3 => load(LoadKind::SignedByte),
                4 => load(LoadKind::Word),
                5 => load(LoadKind::Halfword),
                6 => load(LoadKind::Byte),
                _ => load(LoadKind::SignedHalfword),
            }
        }
        0b0110 if bit(word, 11) => load_immediate(LoadKind::Word, immediate5 << 2),
        0b0110 => store_immediate(StoreSize::Word, immediate5 << 2),
        0b0111 if bit(word, 11) => load_immediate(LoadKind::Byte, immediate5),
        0b0111 => store_immediate(StoreSize::Byte, immediate5),
        0b1000 if bit(word, 11) => load_immediate(LoadKind::Halfword, immediate5 << 1),
        0b1000 => store_immediate(StoreSize::Halfword, immediate5 << 1),
        _ => {
            let register = bits(word, 10, 8) as u8;
            let offset = (bits(word, 7, 0) << 2) as i32;
            if bit(word, 11) {
                Instruction::LoadImmediate {
                    kind: LoadKind::Word,
                    target: register,
                    base: 13,
                    offset,
                    index: true,
                    writeback: false,
                }
            } else {
                Instruction::StoreImmediate {
                    size: StoreSize::Word,
                    source: register,
                    base: 13,
                    offset,
                    index: true,
                    writeback: false,
                }
            }
        }
    }
}

// ============================================================================
// Miscellaneous
// ============================================================================

fn miscellaneous(address: u32, word: u32) -> (Instruction, u8) {
    let low = bits(word, 2, 0) as u8;
    let middle = bits(word, 5, 3) as u8;
    let instruction = match bits(word, 11, 5) {
        0b0000000..=0b0000011 => Instruction::DataImmediate {
            operation: DataOperation::Add,
            flags: FlagMode::Never,
            destination: 13,
            first: 13,
            immediate: bits(word, 6, 0) << 2,
            carry: ImmediateCarry::Unchanged,
        },
        0b0000100..=0b0000111 => Instruction::DataImmediate {
            operation: DataOperation::Sub,
            flags: FlagMode::Never,
            destination: 13,
            first: 13,
            immediate: bits(word, 6, 0) << 2,
            carry: ImmediateCarry::Unchanged,
        },
        0b0001000..=0b0001111 | 0b0011000..=0b0011111 | 0b1001000..=0b1001111 | 0b1011000..=0b1011111 => {
            let offset = (bits(word, 9, 9) << 6) | (bits(word, 7, 3) << 1);
            Instruction::CompareBranch {
                nonzero: bit(word, 11),
                source: low,
                target: address.wrapping_add(4).wrapping_add(offset),
            }
        }
        0b0010000..=0b0010111 => Instruction::Extend {
            kind: match bits(word, 7, 6) {
                0 => ExtendKind::SignedHalfword,
                1 => ExtendKind::SignedByte,
                2 => ExtendKind::UnsignedHalfword,
                _ => ExtendKind::UnsignedByte,
            },
            destination: low,
            addend: 15,
            source: middle,
            rotation: 0,
        },
        0b0100000..=0b0101111 => Instruction::StoreMultiple {
            base: 13,
            registers: (bits(word, 7, 0) | (bits(word, 8, 8) << 14)) as u16,
            writeback: true,
            decrement_before: true,
        },
        0b0110011 => {
            let flags = bits(word, 1, 0);
            Instruction::ChangeProcessorState {
                disable: bit(word, 4),
                primask: flags & 0b010 != 0,
                faultmask: flags & 0b001 != 0,
            }
        }
        0b1010000..=0b1010001 => Instruction::ByteReverse {
            kind: ReverseKind::Word,
            destination: low,
            source: middle,
        },
        0b1010010..=0b1010011 => Instruction::ByteReverse {
            kind: ReverseKind::PackedHalfword,
            destination: low,
            source: middle,
        },
        0b1010110..=0b1010111 => Instruction::ByteReverse {
            kind: ReverseKind::SignedHalfword,
            destination: low,
            source: middle,
        },
        0b1100000..=0b1101111 => Instruction::LoadMultiple {
            base: 13,
            registers: (bits(word, 7, 0) | (bits(word, 8, 8) << 15)) as u16,
            writeback: true,
            decrement_before: false,
        },
        0b1110000..=0b1110111 => Instruction::Breakpoint {
            immediate: bits(word, 7, 0) as u8,
        },
        0b1111000..=0b1111111 => {
            if bits(word, 3, 0) != 0 {
                Instruction::IfThen {
                    state: bits(word, 7, 0) as u8,
                }
            } else {
                Instruction::Hint {
                    kind: hint_kind(bits(word, 7, 4)),
                }
            }
        }
        _ => Instruction::Undefined,
    };
    (instruction, CONDITION_ALWAYS)
}

pub(super) fn hint_kind(option: u32) -> HintKind {
    match option {
        1 => HintKind::Yield,
        2 => HintKind::WaitForEvent,
        3 => HintKind::WaitForInterrupt,
        4 => HintKind::SendEvent,
        _ => HintKind::Nop,
    }
}

// ============================================================================
// Conditional branch, UDF and SVC
// ============================================================================

fn conditional_branch_and_supervisor_call(address: u32, word: u32) -> (Instruction, u8) {
    let condition = bits(word, 11, 8) as u8;
    match condition {
        0b1110 => (Instruction::Undefined, CONDITION_ALWAYS),
        0b1111 => (
            Instruction::SupervisorCall {
                immediate: bits(word, 7, 0) as u8,
            },
            CONDITION_ALWAYS,
        ),
        _ => (
            Instruction::Branch {
                target: address
                    .wrapping_add(4)
                    .wrapping_add((sign_extend(bits(word, 7, 0), 8) << 1) as u32),
            },
            condition,
        ),
    }
}
