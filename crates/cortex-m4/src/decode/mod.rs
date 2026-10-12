// SPDX-License-Identifier: GPL-3.0-only
//! Thumb and Thumb-2 decoding for ARMv7E-M with the DSP extension and FPv4-SP, into `Instruction`.
//!
//! Branch targets and literal addresses are resolved here, since the cache keys by address.

pub mod cache;
mod floating_point;
mod thumb16;
mod thumb32;

// ============================================================================
// Operand types
// ============================================================================

/// Whether an instruction writes the condition flags.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlagMode {
    Never,
    Always,
    /// The 16-bit encodings that set flags only outside an IT block.
    OutsideItBlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shift {
    Lsl,
    Lsr,
    Asr,
    Ror,
    /// Rotate right by one through the carry flag.
    Rrx,
}

/// The carry a modified immediate hands to a flag-setting logical instruction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImmediateCarry {
    Unchanged,
    Clear,
    Set,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DataOperation {
    And,
    Bic,
    Orr,
    Orn,
    Eor,
    Add,
    Adc,
    Sub,
    Sbc,
    Rsb,
    Mov,
    Mvn,
    Tst,
    Teq,
    Cmp,
    Cmn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoadKind {
    Word,
    Halfword,
    SignedHalfword,
    Byte,
    SignedByte,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoreSize {
    Word,
    Halfword,
    Byte,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LongMultiply {
    UnsignedMultiply,
    SignedMultiply,
    UnsignedAccumulate,
    SignedAccumulate,
    /// UMAAL: both halves of the result are accumulated separately.
    UnsignedDoubleAccumulate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverseKind {
    Word,
    PackedHalfword,
    SignedHalfword,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExtendKind {
    SignedByte,
    SignedHalfword,
    UnsignedByte,
    UnsignedHalfword,
    SignedBytePair,
    UnsignedBytePair,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaturatingOperation {
    Add,
    Subtract,
    DoubleAdd,
    DoubleSubtract,
}

/// Which lanes a parallel add or subtract works on and how it combines them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelOperation {
    Add16,
    AddSubtractExchange,
    SubtractAddExchange,
    Subtract16,
    Add8,
    Subtract8,
}

/// The arithmetic a parallel add or subtract uses on each lane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParallelVariant {
    Signed,
    SignedSaturating,
    SignedHalving,
    Unsigned,
    UnsignedSaturating,
    UnsignedHalving,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DspOperation {
    /// SMLA<x><y> and SMUL<x><y> (accumulator 15 means no accumulate).
    MultiplyHalves,
    /// SMLAW<y> and SMULW<y>.
    MultiplyWordByHalf,
    /// SMLAD and SMUAD.
    DualMultiplyAdd,
    /// SMLSD and SMUSD.
    DualMultiplySubtract,
    /// SMLALD.
    LongDualMultiplyAdd,
    /// SMLSLD.
    LongDualMultiplySubtract,
    /// SMLAL<x><y>.
    LongMultiplyHalves,
    /// SMMUL and SMMLA.
    MostSignificantMultiplyAdd,
    /// SMMLS.
    MostSignificantMultiplySubtract,
    /// USAD8 and USADA8.
    SumAbsoluteDifferences,
}

/// Option bits of a DSP multiply.
pub mod dsp_options {
    pub const FIRST_TOP: u8 = 1;
    pub const SECOND_TOP: u8 = 2;
    pub const EXCHANGE: u8 = 4;
    pub const ROUND: u8 = 8;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HintKind {
    Nop,
    Yield,
    WaitForEvent,
    WaitForInterrupt,
    SendEvent,
    /// DMB, DSB and ISB, which order nothing in an emulator with one bus master.
    Barrier,
    /// PLD and PLI.
    Preload,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatOperation {
    Add,
    Subtract,
    Multiply,
    NegateMultiply,
    Divide,
    MultiplyAccumulate,
    MultiplySubtract,
    NegateMultiplyAccumulate,
    NegateMultiplySubtract,
    FusedMultiplyAccumulate,
    FusedMultiplySubtract,
    FusedNegateMultiplyAccumulate,
    FusedNegateMultiplySubtract,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatUnary {
    Move,
    Absolute,
    Negate,
    SquareRoot,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatConversion {
    FloatToSigned,
    FloatToUnsigned,
    SignedToFloat,
    UnsignedToFloat,
}

// ============================================================================
// Instructions
// ============================================================================

/// One decoded instruction; register fields hold r0..r15, FP fields hold s0..s31.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Instruction {
    DataImmediate {
        operation: DataOperation,
        flags: FlagMode,
        destination: u8,
        first: u8,
        immediate: u32,
        carry: ImmediateCarry,
    },
    DataRegister {
        operation: DataOperation,
        flags: FlagMode,
        destination: u8,
        first: u8,
        second: u8,
        shift: Shift,
        amount: u8,
    },
    ShiftByRegister {
        shift: Shift,
        flags: FlagMode,
        destination: u8,
        value: u8,
        amount: u8,
    },
    /// ADR, with the address already resolved.
    Address {
        destination: u8,
        address: u32,
    },
    MoveTop {
        destination: u8,
        immediate: u16,
    },
    Multiply {
        flags: FlagMode,
        destination: u8,
        first: u8,
        second: u8,
    },
    MultiplyAccumulate {
        subtract: bool,
        destination: u8,
        first: u8,
        second: u8,
        accumulator: u8,
    },
    MultiplyLong {
        operation: LongMultiply,
        low: u8,
        high: u8,
        first: u8,
        second: u8,
    },
    Divide {
        signed: bool,
        destination: u8,
        dividend: u8,
        divisor: u8,
    },
    DspMultiply {
        operation: DspOperation,
        options: u8,
        destination: u8,
        first: u8,
        second: u8,
        /// The accumulator register, 15 for none; the low result register for the long forms.
        accumulator: u8,
    },
    BitFieldClear {
        destination: u8,
        lsb: u8,
        msb: u8,
    },
    BitFieldInsert {
        destination: u8,
        source: u8,
        lsb: u8,
        msb: u8,
    },
    BitFieldExtract {
        signed: bool,
        destination: u8,
        source: u8,
        lsb: u8,
        width: u8,
    },
    CountLeadingZeros {
        destination: u8,
        source: u8,
    },
    ReverseBits {
        destination: u8,
        source: u8,
    },
    ByteReverse {
        kind: ReverseKind,
        destination: u8,
        source: u8,
    },
    Extend {
        kind: ExtendKind,
        destination: u8,
        /// The register added to the extended value, 15 for none.
        addend: u8,
        source: u8,
        rotation: u8,
    },
    Saturate {
        signed: bool,
        halfwords: bool,
        destination: u8,
        source: u8,
        bits: u8,
        shift: Shift,
        amount: u8,
    },
    SaturatingArithmetic {
        operation: SaturatingOperation,
        destination: u8,
        first: u8,
        second: u8,
    },
    Parallel {
        operation: ParallelOperation,
        variant: ParallelVariant,
        destination: u8,
        first: u8,
        second: u8,
    },
    Select {
        destination: u8,
        first: u8,
        second: u8,
    },
    Pack {
        top_from_second: bool,
        destination: u8,
        first: u8,
        second: u8,
        amount: u8,
    },
    Branch {
        target: u32,
    },
    BranchLink {
        target: u32,
    },
    BranchExchange {
        source: u8,
    },
    BranchLinkExchange {
        source: u8,
    },
    CompareBranch {
        nonzero: bool,
        source: u8,
        target: u32,
    },
    TableBranch {
        halfword: bool,
        base: u8,
        index: u8,
    },
    IfThen {
        state: u8,
    },
    LoadImmediate {
        kind: LoadKind,
        target: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    },
    LoadLiteral {
        kind: LoadKind,
        target: u8,
        address: u32,
    },
    LoadRegister {
        kind: LoadKind,
        target: u8,
        base: u8,
        offset_register: u8,
        shift: u8,
    },
    StoreImmediate {
        size: StoreSize,
        source: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    },
    StoreRegister {
        size: StoreSize,
        source: u8,
        base: u8,
        offset_register: u8,
        shift: u8,
    },
    LoadDual {
        first: u8,
        second: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    },
    StoreDual {
        first: u8,
        second: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    },
    LoadExclusive {
        size: StoreSize,
        target: u8,
        base: u8,
        offset: u32,
    },
    StoreExclusive {
        size: StoreSize,
        status: u8,
        source: u8,
        base: u8,
        offset: u32,
    },
    ClearExclusive,
    LoadMultiple {
        base: u8,
        registers: u16,
        writeback: bool,
        decrement_before: bool,
    },
    StoreMultiple {
        base: u8,
        registers: u16,
        writeback: bool,
        decrement_before: bool,
    },
    MoveFromSpecial {
        destination: u8,
        special: u8,
    },
    MoveToSpecial {
        source: u8,
        special: u8,
        mask: u8,
    },
    ChangeProcessorState {
        disable: bool,
        primask: bool,
        faultmask: bool,
    },
    Hint {
        kind: HintKind,
    },
    Breakpoint {
        immediate: u8,
    },
    SupervisorCall {
        immediate: u8,
    },
    /// A coprocessor instruction for a coprocessor other than the FPU.
    NoCoprocessor,
    Undefined,
    FloatLoad {
        register: u8,
        double: bool,
        base: u8,
        offset: i32,
    },
    FloatStore {
        register: u8,
        double: bool,
        base: u8,
        offset: i32,
    },
    FloatLoadMultiple {
        first_register: u8,
        count: u8,
        base: u8,
        writeback: bool,
        decrement_before: bool,
    },
    FloatStoreMultiple {
        first_register: u8,
        count: u8,
        base: u8,
        writeback: bool,
        decrement_before: bool,
    },
    /// VMOV between one core register and one single-precision register or scalar.
    FloatMoveCore {
        to_core: bool,
        core: u8,
        single: u8,
    },
    /// VMOV between two core registers and two consecutive single-precision registers.
    FloatMoveCorePair {
        to_core: bool,
        first_core: u8,
        second_core: u8,
        single: u8,
    },
    FloatMoveImmediate {
        destination: u8,
        value: u32,
    },
    /// VMRS (core register 15 means the APSR flags) and VMSR.
    FloatStatusMove {
        to_core: bool,
        core: u8,
    },
    FloatArithmetic {
        operation: FloatOperation,
        destination: u8,
        first: u8,
        second: u8,
    },
    FloatUnary {
        operation: FloatUnary,
        destination: u8,
        source: u8,
    },
    FloatCompare {
        first: u8,
        second: u8,
        with_zero: bool,
        signal_quiet_nan: bool,
    },
    FloatConvert {
        conversion: FloatConversion,
        destination: u8,
        source: u8,
        round_to_zero: bool,
    },
    FloatConvertFixed {
        to_fixed: bool,
        signed: bool,
        halfword: bool,
        register: u8,
        fraction_bits: u8,
    },
    FloatConvertHalf {
        to_half: bool,
        top: bool,
        destination: u8,
        source: u8,
    },
}

/// The condition code meaning "always".
pub const CONDITION_ALWAYS: u8 = 14;

/// An instruction with its encoding, ready to cache.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub instruction: Instruction,
    pub first_halfword: u16,
    pub second_halfword: u16,
    /// 2 or 4 bytes.
    pub length: u8,
    /// The condition of a conditional branch, otherwise `CONDITION_ALWAYS`.
    pub condition: u8,
}

// ============================================================================
// Entry points
// ============================================================================

/// Whether `first_halfword` starts a 32-bit instruction.
#[inline]
pub fn is_32bit(first_halfword: u16) -> bool {
    first_halfword >= 0xE800
}

/// Decodes a 16-bit instruction at `address`.
pub fn decode_16(address: u32, halfword: u16) -> Decoded {
    let (instruction, condition) = thumb16::decode(address, halfword);
    Decoded {
        instruction,
        first_halfword: halfword,
        second_halfword: 0,
        length: 2,
        condition,
    }
}

/// Decodes a 32-bit instruction at `address`.
pub fn decode_32(address: u32, first_halfword: u16, second_halfword: u16) -> Decoded {
    let (instruction, condition) = thumb32::decode(address, first_halfword, second_halfword);
    Decoded {
        instruction,
        first_halfword,
        second_halfword,
        length: 4,
        condition,
    }
}

// ============================================================================
// Shared field helpers
// ============================================================================

#[inline]
pub(crate) fn bits(value: u32, high: u32, low: u32) -> u32 {
    (value >> low) & ((1u32 << (high - low + 1)) - 1)
}

#[inline]
pub(crate) fn bit(value: u32, position: u32) -> bool {
    (value >> position) & 1 != 0
}

#[inline]
pub(crate) fn sign_extend(value: u32, bit_count: u32) -> i32 {
    let shift = 32 - bit_count;
    ((value << shift) as i32) >> shift
}

/// DecodeImmShift from the Arm manual: the shift type and amount of a shifted register operand.
pub(crate) fn decode_immediate_shift(shift_type: u32, amount: u32) -> (Shift, u8) {
    match shift_type {
        0 => (Shift::Lsl, amount as u8),
        1 => (Shift::Lsr, if amount == 0 { 32 } else { amount as u8 }),
        2 => (Shift::Asr, if amount == 0 { 32 } else { amount as u8 }),
        _ if amount == 0 => (Shift::Rrx, 1),
        _ => (Shift::Ror, amount as u8),
    }
}

/// ThumbExpandImm_C from the Arm manual: a 12-bit modified immediate and the carry it produces.
pub(crate) fn thumb_expand_immediate(imm12: u32) -> (u32, ImmediateCarry) {
    if imm12 >> 10 == 0 {
        let byte = imm12 & 0xFF;
        let value = match (imm12 >> 8) & 3 {
            0 => byte,
            1 => byte | (byte << 16),
            2 => (byte << 8) | (byte << 24),
            _ => byte | (byte << 8) | (byte << 16) | (byte << 24),
        };
        (value, ImmediateCarry::Unchanged)
    } else {
        let unrotated = 0x80 | (imm12 & 0x7F);
        let value = unrotated.rotate_right(imm12 >> 7);
        let carry = if value >> 31 != 0 {
            ImmediateCarry::Set
        } else {
            ImmediateCarry::Clear
        };
        (value, carry)
    }
}
