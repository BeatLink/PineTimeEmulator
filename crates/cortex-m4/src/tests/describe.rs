// SPDX-License-Identifier: GPL-3.0-only
//! Names a decoded instruction the way Capstone does and lists the registers it names, so the
//! differential test can compare the two decoders.

use crate::decode::*;

// ============================================================================
// Mnemonics
// ============================================================================

fn data_name(operation: DataOperation) -> &'static str {
    use DataOperation::*;
    match operation {
        And => "and",
        Bic => "bic",
        Orr => "orr",
        Orn => "orn",
        Eor => "eor",
        Add => "add",
        Adc => "adc",
        Sub => "sub",
        Sbc => "sbc",
        Rsb => "rsb",
        Mov => "mov",
        Mvn => "mvn",
        Tst => "tst",
        Teq => "teq",
        Cmp => "cmp",
        Cmn => "cmn",
    }
}

fn shift_name(shift: Shift) -> &'static str {
    match shift {
        Shift::Lsl => "lsl",
        Shift::Lsr => "lsr",
        Shift::Asr => "asr",
        Shift::Ror => "ror",
        Shift::Rrx => "rrx",
    }
}

fn with_flags(name: &str, flags: FlagMode, operation: Option<DataOperation>) -> String {
    let compare = matches!(
        operation,
        Some(DataOperation::Tst | DataOperation::Teq | DataOperation::Cmp | DataOperation::Cmn)
    );
    if flags != FlagMode::Never && !compare {
        format!("{name}s")
    } else {
        name.to_string()
    }
}

fn load_suffix(kind: LoadKind) -> &'static str {
    match kind {
        LoadKind::Word => "",
        LoadKind::Halfword => "h",
        LoadKind::SignedHalfword => "sh",
        LoadKind::Byte => "b",
        LoadKind::SignedByte => "sb",
    }
}

fn store_suffix(size: StoreSize) -> &'static str {
    match size {
        StoreSize::Word => "",
        StoreSize::Halfword => "h",
        StoreSize::Byte => "b",
    }
}

fn half_letter(top: bool) -> &'static str {
    if top {
        "t"
    } else {
        "b"
    }
}

/// Whether a 32-bit single load or store uses the unprivileged (LDRT-style) encoding.
fn is_unprivileged(decoded: &Decoded) -> bool {
    decoded.length == 4
        && decoded.first_halfword >> 9 == 0b111_1100
        && decoded.first_halfword & 0x0080 == 0
        && decoded.first_halfword & 0x000F != 0x000F
        && decoded.second_halfword & 0x0F00 == 0x0E00
}

pub fn mnemonic(decoded: &Decoded) -> String {
    use Instruction::*;
    let first = decoded.first_halfword;
    match decoded.instruction {
        DataImmediate { operation, flags, .. } => {
            if decoded.length == 4 && first & 0xFBF0 == 0xF240 {
                return "movw".into();
            }
            if decoded.length == 4 && first & 0xFA00 == 0xF200 {
                return format!("{}w", data_name(operation));
            }
            with_flags(data_name(operation), flags, Some(operation))
        }
        DataRegister {
            operation,
            flags,
            shift,
            amount,
            ..
        } => {
            if operation == DataOperation::Mov && !(shift == Shift::Lsl && amount == 0) {
                return with_flags(shift_name(shift), flags, None);
            }
            with_flags(data_name(operation), flags, Some(operation))
        }
        ShiftByRegister { shift, flags, .. } => with_flags(shift_name(shift), flags, None),
        Address { .. } if decoded.length == 4 => if decoded.first_halfword & 0x00A0 != 0 {
            "subw"
        } else {
            "addw"
        }
        .into(),
        Address { .. } => "adr".into(),
        MoveTop { .. } => "movt".into(),
        Multiply { flags, .. } => with_flags("mul", flags, None),
        MultiplyAccumulate { subtract, .. } => if subtract { "mls" } else { "mla" }.into(),
        MultiplyLong { operation, .. } => match operation {
            LongMultiply::UnsignedMultiply => "umull",
            LongMultiply::SignedMultiply => "smull",
            LongMultiply::UnsignedAccumulate => "umlal",
            LongMultiply::SignedAccumulate => "smlal",
            LongMultiply::UnsignedDoubleAccumulate => "umaal",
        }
        .into(),
        Divide { signed, .. } => if signed { "sdiv" } else { "udiv" }.into(),
        DspMultiply {
            operation,
            options,
            accumulator,
            ..
        } => dsp_mnemonic(operation, options, accumulator),
        BitFieldClear { .. } => "bfc".into(),
        BitFieldInsert { .. } => "bfi".into(),
        BitFieldExtract { signed, .. } => if signed { "sbfx" } else { "ubfx" }.into(),
        CountLeadingZeros { .. } => "clz".into(),
        ReverseBits { .. } => "rbit".into(),
        ByteReverse { kind, .. } => match kind {
            ReverseKind::Word => "rev",
            ReverseKind::PackedHalfword => "rev16",
            ReverseKind::SignedHalfword => "revsh",
        }
        .into(),
        Extend { kind, addend, .. } => {
            let (sign, size) = match kind {
                ExtendKind::SignedByte => ("s", "b"),
                ExtendKind::SignedHalfword => ("s", "h"),
                ExtendKind::UnsignedByte => ("u", "b"),
                ExtendKind::UnsignedHalfword => ("u", "h"),
                ExtendKind::SignedBytePair => ("s", "b16"),
                ExtendKind::UnsignedBytePair => ("u", "b16"),
            };
            let add = if addend == 15 { "" } else { "a" };
            format!("{sign}xt{add}{size}")
        }
        Saturate {
            signed, halfwords, ..
        } => {
            format!(
                "{}sat{}",
                if signed { "s" } else { "u" },
                if halfwords { "16" } else { "" }
            )
        }
        SaturatingArithmetic { operation, .. } => match operation {
            SaturatingOperation::Add => "qadd",
            SaturatingOperation::Subtract => "qsub",
            SaturatingOperation::DoubleAdd => "qdadd",
            SaturatingOperation::DoubleSubtract => "qdsub",
        }
        .into(),
        Parallel {
            operation, variant, ..
        } => {
            let prefix = match variant {
                ParallelVariant::Signed => "s",
                ParallelVariant::SignedSaturating => "q",
                ParallelVariant::SignedHalving => "sh",
                ParallelVariant::Unsigned => "u",
                ParallelVariant::UnsignedSaturating => "uq",
                ParallelVariant::UnsignedHalving => "uh",
            };
            let name = match operation {
                ParallelOperation::Add16 => "add16",
                ParallelOperation::AddSubtractExchange => "asx",
                ParallelOperation::SubtractAddExchange => "sax",
                ParallelOperation::Subtract16 => "sub16",
                ParallelOperation::Add8 => "add8",
                ParallelOperation::Subtract8 => "sub8",
            };
            format!("{prefix}{name}")
        }
        Select { .. } => "sel".into(),
        Pack { top_from_second, .. } => if top_from_second { "pkhtb" } else { "pkhbt" }.into(),
        Branch { .. } => "b".into(),
        BranchLink { .. } => "bl".into(),
        BranchExchange { .. } => "bx".into(),
        BranchLinkExchange { .. } => "blx".into(),
        CompareBranch { nonzero, .. } => if nonzero { "cbnz" } else { "cbz" }.into(),
        TableBranch { halfword, .. } => if halfword { "tbh" } else { "tbb" }.into(),
        IfThen { .. } => "it".into(),
        LoadImmediate { kind, .. } | LoadLiteral { kind, .. } | LoadRegister { kind, .. } => {
            let unprivileged = if is_unprivileged(decoded) { "t" } else { "" };
            format!("ldr{}{unprivileged}", load_suffix(kind))
        }
        StoreImmediate { size, .. } | StoreRegister { size, .. } => {
            let unprivileged = if is_unprivileged(decoded) { "t" } else { "" };
            format!("str{}{unprivileged}", store_suffix(size))
        }
        LoadDual { .. } => "ldrd".into(),
        StoreDual { .. } => "strd".into(),
        LoadExclusive { size, .. } => format!("ldrex{}", store_suffix(size)),
        StoreExclusive { size, .. } => format!("strex{}", store_suffix(size)),
        ClearExclusive => "clrex".into(),
        LoadMultiple {
            base,
            writeback,
            decrement_before,
            ..
        } => {
            if base == 13 && writeback && !decrement_before {
                "pop".into()
            } else if decrement_before {
                "ldmdb".into()
            } else {
                "ldm".into()
            }
        }
        StoreMultiple {
            base,
            writeback,
            decrement_before,
            ..
        } => {
            if base == 13 && writeback && decrement_before {
                "push".into()
            } else if decrement_before {
                "stmdb".into()
            } else {
                "stm".into()
            }
        }
        MoveFromSpecial { .. } => "mrs".into(),
        MoveToSpecial { .. } => "msr".into(),
        ChangeProcessorState { disable, .. } => if disable { "cpsid" } else { "cpsie" }.into(),
        Hint { kind } => match kind {
            HintKind::Nop => "nop",
            HintKind::Yield => "yield",
            HintKind::WaitForEvent => "wfe",
            HintKind::WaitForInterrupt => "wfi",
            HintKind::SendEvent => "sev",
            HintKind::Barrier => match (decoded.second_halfword >> 4) & 0xF {
                4 => "dsb",
                5 => "dmb",
                _ => "isb",
            },
            HintKind::Preload => {
                if first & 0x0100 != 0 {
                    "pli"
                } else {
                    "pld"
                }
            }
        }
        .into(),
        Breakpoint { .. } => "bkpt".into(),
        SupervisorCall { .. } => "svc".into(),
        NoCoprocessor => "coprocessor".into(),
        Undefined => "undefined".into(),
        FloatLoad { .. } => "vldr".into(),
        FloatStore { .. } => "vstr".into(),
        FloatLoadMultiple {
            base,
            writeback,
            decrement_before,
            ..
        } => {
            if base == 13 && writeback && !decrement_before {
                "vpop".into()
            } else if decrement_before {
                "vldmdb".into()
            } else {
                "vldmia".into()
            }
        }
        FloatStoreMultiple {
            base,
            writeback,
            decrement_before,
            ..
        } => {
            if base == 13 && writeback && decrement_before {
                "vpush".into()
            } else if decrement_before {
                "vstmdb".into()
            } else {
                "vstmia".into()
            }
        }
        FloatMoveCore { .. } | FloatMoveCorePair { .. } | FloatMoveImmediate { .. } => "vmov".into(),
        FloatStatusMove { to_core, .. } => if to_core { "vmrs" } else { "vmsr" }.into(),
        FloatArithmetic { operation, .. } => {
            use FloatOperation::*;
            match operation {
                Add => "vadd",
                Subtract => "vsub",
                Multiply => "vmul",
                NegateMultiply => "vnmul",
                Divide => "vdiv",
                MultiplyAccumulate => "vmla",
                MultiplySubtract => "vmls",
                NegateMultiplyAccumulate => "vnmla",
                NegateMultiplySubtract => "vnmls",
                FusedMultiplyAccumulate => "vfma",
                FusedMultiplySubtract => "vfms",
                FusedNegateMultiplyAccumulate => "vfnma",
                FusedNegateMultiplySubtract => "vfnms",
            }
            .into()
        }
        FloatUnary { operation, .. } => match operation {
            crate::decode::FloatUnary::Move => "vmov",
            crate::decode::FloatUnary::Absolute => "vabs",
            crate::decode::FloatUnary::Negate => "vneg",
            crate::decode::FloatUnary::SquareRoot => "vsqrt",
        }
        .into(),
        FloatCompare { signal_quiet_nan, .. } => if signal_quiet_nan { "vcmpe" } else { "vcmp" }.into(),
        FloatConvert {
            round_to_zero,
            conversion,
            ..
        } => {
            let to_integer = matches!(
                conversion,
                FloatConversion::FloatToSigned | FloatConversion::FloatToUnsigned
            );
            if to_integer && !round_to_zero {
                "vcvtr"
            } else {
                "vcvt"
            }
            .into()
        }
        FloatConvertFixed { .. } => "vcvt".into(),
        FloatConvertHalf { top, .. } => format!("vcvt{}", half_letter(top)),
    }
}

fn dsp_mnemonic(operation: DspOperation, options: u8, accumulator: u8) -> String {
    use DspOperation::*;
    let first_top = half_letter(options & dsp_options::FIRST_TOP != 0);
    let second_top = half_letter(options & dsp_options::SECOND_TOP != 0);
    let exchange = if options & dsp_options::EXCHANGE != 0 {
        "x"
    } else {
        ""
    };
    let round = if options & dsp_options::ROUND != 0 {
        "r"
    } else {
        ""
    };
    let accumulate = accumulator != 15;
    match operation {
        MultiplyHalves if accumulate => format!("smla{first_top}{second_top}"),
        MultiplyHalves => format!("smul{first_top}{second_top}"),
        MultiplyWordByHalf if accumulate => format!("smlaw{second_top}"),
        MultiplyWordByHalf => format!("smulw{second_top}"),
        DualMultiplyAdd if accumulate => format!("smlad{exchange}"),
        DualMultiplyAdd => format!("smuad{exchange}"),
        DualMultiplySubtract if accumulate => format!("smlsd{exchange}"),
        DualMultiplySubtract => format!("smusd{exchange}"),
        LongDualMultiplyAdd => format!("smlald{exchange}"),
        LongDualMultiplySubtract => format!("smlsld{exchange}"),
        LongMultiplyHalves => format!("smlal{first_top}{second_top}"),
        MostSignificantMultiplyAdd if accumulate => format!("smmla{round}"),
        MostSignificantMultiplyAdd => format!("smmul{round}"),
        MostSignificantMultiplySubtract => format!("smmls{round}"),
        SumAbsoluteDifferences if accumulate => "usada8".into(),
        SumAbsoluteDifferences => "usad8".into(),
    }
}

// ============================================================================
// Registers
// ============================================================================

/// The core registers (bit per register) and single-precision registers an instruction names.
pub fn registers(decoded: &Decoded) -> (u16, u64) {
    use Instruction::*;
    let mut core = 0u16;
    let mut float = 0u64;
    let mut add = |register: u8| core |= 1 << register;
    match decoded.instruction {
        DataImmediate {
            operation,
            destination,
            first,
            ..
        } => {
            if !matches!(
                operation,
                DataOperation::Tst | DataOperation::Teq | DataOperation::Cmp | DataOperation::Cmn
            ) {
                add(destination);
            }
            if !matches!(operation, DataOperation::Mov | DataOperation::Mvn) {
                add(first);
            }
        }
        DataRegister {
            operation,
            destination,
            first,
            second,
            ..
        } => {
            if !matches!(
                operation,
                DataOperation::Tst | DataOperation::Teq | DataOperation::Cmp | DataOperation::Cmn
            ) {
                add(destination);
            }
            if !matches!(operation, DataOperation::Mov | DataOperation::Mvn) {
                add(first);
            }
            add(second);
        }
        ShiftByRegister {
            destination,
            value,
            amount,
            ..
        } => {
            add(destination);
            add(value);
            add(amount);
        }
        Address { destination, .. } => {
            add(destination);
            if decoded.length == 4 {
                add(15);
            }
        }
        MoveTop { destination, .. } => add(destination),
        Multiply {
            destination,
            first,
            second,
            ..
        }
        | Divide {
            destination,
            dividend: first,
            divisor: second,
            ..
        }
        | Select {
            destination,
            first,
            second,
        }
        | Pack {
            destination,
            first,
            second,
            ..
        }
        | SaturatingArithmetic {
            destination,
            first,
            second,
            ..
        }
        | Parallel {
            destination,
            first,
            second,
            ..
        } => {
            add(destination);
            add(first);
            add(second);
        }
        MultiplyAccumulate {
            destination,
            first,
            second,
            accumulator,
            ..
        } => {
            add(destination);
            add(first);
            add(second);
            add(accumulator);
        }
        DspMultiply {
            operation,
            destination,
            first,
            second,
            accumulator,
            ..
        } => {
            add(destination);
            add(first);
            add(second);
            let long = matches!(
                operation,
                DspOperation::LongDualMultiplyAdd
                    | DspOperation::LongDualMultiplySubtract
                    | DspOperation::LongMultiplyHalves
            );
            if accumulator != 15 || long {
                add(accumulator);
            }
        }
        MultiplyLong {
            low,
            high,
            first,
            second,
            ..
        } => {
            add(low);
            add(high);
            add(first);
            add(second);
        }
        BitFieldClear { destination, .. } => add(destination),
        BitFieldInsert {
            destination, source, ..
        }
        | BitFieldExtract {
            destination, source, ..
        }
        | CountLeadingZeros { destination, source }
        | ReverseBits { destination, source }
        | ByteReverse {
            destination, source, ..
        }
        | Saturate {
            destination, source, ..
        } => {
            add(destination);
            add(source);
        }
        Extend {
            destination,
            addend,
            source,
            ..
        } => {
            add(destination);
            add(source);
            if addend != 15 {
                add(addend);
            }
        }
        BranchExchange { source } | BranchLinkExchange { source } | CompareBranch { source, .. } => {
            add(source)
        }
        TableBranch { base, index, .. } => {
            add(base);
            add(index);
        }
        LoadImmediate { target, base, .. } => {
            add(target);
            add(base);
        }
        LoadLiteral { target, .. } => {
            add(target);
            add(15);
        }
        LoadRegister {
            target,
            base,
            offset_register,
            ..
        }
        | StoreRegister {
            source: target,
            base,
            offset_register,
            ..
        } => {
            add(target);
            add(base);
            add(offset_register);
        }
        StoreImmediate { source, base, .. } => {
            add(source);
            add(base);
        }
        LoadDual {
            first, second, base, ..
        }
        | StoreDual {
            first, second, base, ..
        } => {
            add(first);
            add(second);
            add(base);
        }
        LoadExclusive { target, base, .. } => {
            add(target);
            add(base);
        }
        StoreExclusive {
            status, source, base, ..
        } => {
            add(status);
            add(source);
            add(base);
        }
        LoadMultiple {
            base,
            registers,
            writeback,
            decrement_before,
        } => {
            core |= registers;
            if !(base == 13 && writeback && !decrement_before) {
                core |= 1 << base;
            }
        }
        StoreMultiple {
            base,
            registers,
            writeback,
            decrement_before,
        } => {
            core |= registers;
            if !(base == 13 && writeback && decrement_before) {
                core |= 1 << base;
            }
        }
        MoveFromSpecial { destination, .. } => add(destination),
        MoveToSpecial { source, .. } => add(source),
        FloatLoad {
            register,
            double,
            base,
            ..
        }
        | FloatStore {
            register,
            double,
            base,
            ..
        } => {
            add(base);
            float |= if double { 3 } else { 1 } << register;
        }
        FloatLoadMultiple {
            first_register,
            count,
            base,
            writeback,
            decrement_before,
        } => {
            if !(base == 13 && writeback && !decrement_before) {
                add(base);
            }
            float |= ((1u64 << count) - 1) << first_register;
        }
        FloatStoreMultiple {
            first_register,
            count,
            base,
            writeback,
            decrement_before,
        } => {
            if !(base == 13 && writeback && decrement_before) {
                add(base);
            }
            float |= ((1u64 << count) - 1) << first_register;
        }
        FloatMoveCore {
            core: register,
            single,
            ..
        } => {
            add(register);
            float |= 1 << single;
        }
        FloatMoveCorePair {
            first_core,
            second_core,
            single,
            ..
        } => {
            add(first_core);
            add(second_core);
            float |= 3 << single;
        }
        FloatMoveImmediate { destination, .. } => float |= 1 << destination,
        FloatStatusMove { core: register, .. } => {
            if register != 15 {
                add(register);
            }
        }
        FloatArithmetic {
            destination,
            first,
            second,
            ..
        } => float |= (1 << destination) | (1 << first) | (1 << second),
        FloatUnary {
            destination, source, ..
        }
        | FloatConvert {
            destination, source, ..
        }
        | FloatConvertHalf {
            destination, source, ..
        } => float |= (1 << destination) | (1 << source),
        FloatCompare {
            first,
            second,
            with_zero,
            ..
        } => {
            float |= 1 << first;
            if !with_zero {
                float |= 1 << second;
            }
        }
        FloatConvertFixed { register, .. } => float |= 1 << register,
        Hint {
            kind: HintKind::Preload,
        } => {
            let base = (decoded.first_halfword & 0xF) as u8;
            add(base);
            let register_form = decoded.first_halfword & 0x0080 == 0 && decoded.second_halfword & 0x0FC0 == 0;
            if base != 15 && register_form {
                add((decoded.second_halfword & 0xF) as u8);
            }
        }
        Branch { .. }
        | BranchLink { .. }
        | IfThen { .. }
        | ClearExclusive
        | ChangeProcessorState { .. }
        | Hint { .. }
        | Breakpoint { .. }
        | SupervisorCall { .. }
        | NoCoprocessor
        | Undefined => {}
    }
    (core, float)
}
