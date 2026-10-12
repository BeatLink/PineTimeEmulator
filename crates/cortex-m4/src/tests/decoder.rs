// SPDX-License-Identifier: GPL-3.0-only
//! The decoder against Capstone: every 16-bit halfword and a large sample of 32-bit encodings
//! must agree on length, mnemonic and the registers named.

use std::collections::{BTreeMap, BTreeSet};

use capstone::arch::arm::{ArchExtraMode, ArchMode};
use capstone::prelude::*;

use super::describe::{mnemonic, registers};
use crate::decode::{decode_16, decode_32, is_32bit, Decoded, Instruction};

// ============================================================================
// Capstone side
// ============================================================================

const CONDITIONS: [&str; 17] = [
    "eq", "ne", "hs", "cs", "lo", "cc", "mi", "pl", "vs", "vc", "hi", "ls", "ge", "lt", "gt", "le", "al",
];

fn capstone() -> Capstone {
    Capstone::new()
        .arm()
        .mode(ArchMode::Thumb)
        .extra_mode([ArchExtraMode::MClass].iter().copied())
        .build()
        .unwrap()
}

fn normalize_mnemonic(raw: &str) -> String {
    let base = raw.split('.').next().unwrap().to_lowercase();
    if base.len() == 3 && base.starts_with('b') && CONDITIONS.contains(&&base[1..]) {
        return "b".into();
    }
    if base.starts_with("it") && base[2..].chars().all(|letter| letter == 't' || letter == 'e') {
        return "it".into();
    }
    if base.len() == 5
        && (base.starts_with("mrs") || base.starts_with("msr"))
        && CONDITIONS.contains(&&base[3..])
    {
        return base[..3].into();
    }
    base
}

fn capstone_registers(operands: &str) -> (u16, u64) {
    let mut core = 0u16;
    let mut float = 0u64;
    for token in operands.split(|character: char| !character.is_ascii_alphanumeric() && character != '_') {
        let named = match token {
            "sb" => Some(9),
            "sl" => Some(10),
            "fp" => Some(11),
            "ip" => Some(12),
            "sp" => Some(13),
            "lr" => Some(14),
            "pc" => Some(15),
            _ => None,
        };
        if let Some(register) = named {
            core |= 1 << register;
            continue;
        }
        let number = |prefix: char| -> Option<u32> {
            token.strip_prefix(prefix).and_then(|digits| digits.parse().ok())
        };
        if let Some(register) = number('r') {
            core |= 1 << register;
        } else if let Some(register) = number('s') {
            float |= 1 << register;
        } else if let Some(register) = number('d') {
            float |= 3 << (2 * register);
        }
    }
    (core, float)
}

struct Reference {
    mnemonic: String,
    operands: String,
    length: usize,
}

fn reference(engine: &Capstone, bytes: &[u8]) -> Option<Reference> {
    let instructions = engine.disasm_count(bytes, 0x1000, 1).ok()?;
    let instruction = instructions.iter().next()?;
    Some(Reference {
        mnemonic: normalize_mnemonic(instruction.mnemonic().unwrap_or("")),
        operands: instruction.op_str().unwrap_or("").to_string(),
        length: instruction.len(),
    })
}

/// Clears Capstone's IT-block state, which it carries from one call to the next.
fn flush_it_state(engine: &Capstone) {
    for _ in 0..4 {
        let _ = engine.disasm_count(&[0x00, 0xBF], 0x1000, 1);
    }
}

// ============================================================================
// Comparison
// ============================================================================

/// Names that differ for good reason, as (Capstone, ours), mostly Armv8-M names Capstone knows.
const KNOWN_NAME_DIFFERENCES: &[(&str, &str)] = &[
    ("bxns", "bx"),
    ("blxns", "blx"),
    ("hint", "nop"),
    ("pldw", "pld"),
    ("tt", "strex"),
    ("tta", "strex"),
    ("ttt", "strex"),
    ("ttat", "strex"),
    ("fldmiax", "vldmia"),
    ("fldmdbx", "vldmdb"),
    ("fstmiax", "vstmia"),
    ("fstmdbx", "vstmdb"),
];

/// Whether an encoding Capstone accepts may be undefined here (Neon, Armv8, UNPREDICTABLE).
fn undefined_is_acceptable(
    reference: &Reference,
    decoded: &Decoded,
    our_mnemonics: &BTreeSet<String>,
) -> bool {
    let advanced_simd = decoded.length == 4 && decoded.first_halfword & 0xEF00 == 0xEF00;
    if advanced_simd || !our_mnemonics.contains(&reference.mnemonic) {
        return true;
    }
    let operands = &reference.operands;
    let beyond_d15 = (16..32).any(|register| {
        operands
            .split(|character: char| !character.is_ascii_alphanumeric())
            .any(|token| token == format!("d{register}"))
    });
    if beyond_d15 || operands.contains("fpexc") || operands.contains('q') {
        return true;
    }
    match reference.mnemonic.as_str() {
        "smmls" | "smmlsr" => operands.ends_with("pc"),
        "vcvt" => operands.contains("#0xfff"),
        "vmov" => {
            operands.contains('[')
                && (decoded.second_halfword & 0x0060 != 0 || decoded.first_halfword & 0x0040 != 0)
        }
        "vldmia" | "vldmdb" | "vstmia" | "vstmdb" | "vpush" | "vpop" => {
            let listed =
                operands.matches(',').count() + 1 - operands.split('{').next().unwrap().matches(',').count();
            let words = (decoded.second_halfword & 0xFF) as usize;
            let double = decoded.second_halfword & 0x0100 != 0;
            listed != if double { words / 2 } else { words }
        }
        _ => false,
    }
}

/// Encodings whose two copies of a register field differ, which is UNPREDICTABLE.
fn has_mismatched_register_copies(decoded: &Decoded) -> bool {
    matches!(
        decoded.instruction,
        Instruction::CountLeadingZeros { .. }
            | Instruction::ReverseBits { .. }
            | Instruction::ByteReverse { .. }
    ) && decoded.length == 4
        && decoded.first_halfword & 0xF != decoded.second_halfword & 0xF
}

#[derive(Default)]
struct Tally {
    compared: usize,
    mismatches: BTreeMap<String, (usize, String)>,
    capstone_only_undefined: usize,
}

impl Tally {
    fn record(&mut self, key: String, example: String) {
        let entry = self.mismatches.entry(key).or_insert((0, example));
        entry.0 += 1;
    }

    fn report(&self) -> String {
        let mut report = String::new();
        for (key, (count, example)) in &self.mismatches {
            report.push_str(&format!("{count:6}  {key}   e.g. {example}\n"));
        }
        report
    }
}

struct Pair {
    decoded: Decoded,
    reference: Option<Reference>,
}

fn collect(engine: &Capstone, decoded: Decoded, bytes: &[u8]) -> Pair {
    let reference = reference(engine, bytes);
    if reference
        .as_ref()
        .is_some_and(|reference| reference.mnemonic == "it")
    {
        flush_it_state(engine);
    }
    Pair { decoded, reference }
}

fn compare_all(pairs: &[Pair]) -> Tally {
    let our_mnemonics: BTreeSet<String> = pairs.iter().map(|pair| mnemonic(&pair.decoded)).collect();
    let mut tally = Tally::default();
    for pair in pairs {
        compare(&mut tally, pair, &our_mnemonics);
    }
    tally
}

fn compare(tally: &mut Tally, pair: &Pair, our_mnemonics: &BTreeSet<String>) {
    let decoded = pair.decoded;
    let ours_valid = !matches!(decoded.instruction, Instruction::Undefined);
    let Some(reference) = &pair.reference else {
        if ours_valid {
            tally.capstone_only_undefined += 1;
        }
        return;
    };
    tally.compared += 1;
    let encoding = if decoded.length == 4 {
        format!("{:04x} {:04x}", decoded.first_halfword, decoded.second_halfword)
    } else {
        format!("{:04x}", decoded.first_halfword)
    };
    let example = format!("{encoding}: {} {}", reference.mnemonic, reference.operands);
    if !ours_valid {
        if !undefined_is_acceptable(reference, &decoded, our_mnemonics) {
            tally.record(format!("{} -> undefined", reference.mnemonic), example);
        }
        return;
    }
    if matches!(decoded.instruction, Instruction::NoCoprocessor) {
        return;
    }
    if reference.length != decoded.length as usize {
        tally.record(format!("length {}", reference.mnemonic), example);
        return;
    }
    let ours = mnemonic(&decoded);
    if ours != reference.mnemonic {
        if !KNOWN_NAME_DIFFERENCES.contains(&(reference.mnemonic.as_str(), ours.as_str())) {
            tally.record(format!("{} -> {ours}", reference.mnemonic), example);
        }
        return;
    }
    if has_mismatched_register_copies(&decoded) {
        return;
    }
    let expected = capstone_registers(&reference.operands);
    if ours == "msr" && expected.0 == 0 {
        return;
    }
    let actual = registers(&decoded);
    if expected != actual {
        tally.record(
            format!(
                "registers {ours} ({:04x},{:x} vs {:04x},{:x})",
                expected.0, expected.1, actual.0, actual.1
            )
            .chars()
            .take(40)
            .collect(),
            format!("{example}   ours {:?}", decoded.instruction),
        );
    }
}

/// A small xorshift generator, so the sample is the same on every run.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

#[test]
fn every_16_bit_encoding_matches_capstone() {
    let engine = capstone();
    let mut pairs = Vec::new();
    for halfword in 0..=0xFFFFu32 {
        let halfword = halfword as u16;
        if is_32bit(halfword) {
            continue;
        }
        pairs.push(collect(
            &engine,
            decode_16(0x1000, halfword),
            &halfword.to_le_bytes(),
        ));
    }
    let tally = compare_all(&pairs);
    assert!(tally.compared > 50_000, "compared {}", tally.compared);
    assert!(tally.mismatches.is_empty(), "mismatches:\n{}", tally.report());
}

#[test]
fn sampled_32_bit_encodings_match_capstone() {
    let engine = capstone();
    let mut pairs = Vec::new();
    let mut random = Random(0x9E37_79B9_7F4A_7C15);
    for first in 0xE800..=0xFFFFu32 {
        for _ in 0..48 {
            let second = random.next() & 0xFFFF;
            let mut bytes = (first as u16).to_le_bytes().to_vec();
            bytes.extend_from_slice(&(second as u16).to_le_bytes());
            pairs.push(collect(
                &engine,
                decode_32(0x1000, first as u16, second as u16),
                &bytes,
            ));
        }
    }
    let tally = compare_all(&pairs);
    assert!(tally.compared > 100_000, "compared {}", tally.compared);
    let our_mnemonics: BTreeSet<String> = pairs.iter().map(|pair| mnemonic(&pair.decoded)).collect();
    assert!(
        our_mnemonics.len() > 200,
        "only {} kinds decoded",
        our_mnemonics.len()
    );
    assert!(tally.mismatches.is_empty(), "mismatches:\n{}", tally.report());
}
