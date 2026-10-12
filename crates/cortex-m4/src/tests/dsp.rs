// SPDX-License-Identifier: GPL-3.0-only
//! The DSP extension: saturation, parallel arithmetic, SEL, packing and DSP multiplies.

use super::support::run;

const Q: u32 = 1 << 27;

#[test]
fn saturate_instructions_clamp_and_set_q() {
    let machine = run(
        "ldr r0, =300\n usat r1, #8, r0\n ssat r2, #8, r0\n mrs r3, apsr\n ldr r4, =0x7FFF8000\n ssat16 r5, #8, r4\n\
         usat r6, #8, r0, asr #2\n bkpt #0",
    );
    assert_eq!(machine.register(1), 255);
    assert_eq!(machine.register(2), 127);
    assert_ne!(machine.register(3) & Q, 0);
    assert_eq!(machine.register(5), 0x007F_FF80);
    assert_eq!(machine.register(6), 75);
}

#[test]
fn saturating_add_and_subtract() {
    let machine = run(
        "ldr r0, =0x7FFFFFFF\n movs r1, #1\n qadd r2, r0, r1\n ldr r3, =0x80000000\n qsub r4, r3, r1\n\
         ldr r5, =0x40000000\n qdadd r6, r1, r5\n movs r7, #5\n qsub r8, r7, r1\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x7FFF_FFFF);
    assert_eq!(machine.register(4), 0x8000_0000);
    assert_eq!(machine.register(6), 0x7FFF_FFFF);
    assert_eq!(machine.register(8), 4);
}

#[test]
fn parallel_arithmetic_and_select() {
    let machine = run(
        "ldr r0, =0x7FFF0001\n ldr r1, =0x00010002\n sadd16 r2, r0, r1\n qadd16 r3, r0, r1\n\
         ldr r4, =0xFF0180FF\n ldr r5, =0x01FF8001\n uadd8 r6, r4, r5\n sel r7, r4, r5\n\
         uhadd16 r8, r0, r1\n ssub8 r9, r1, r0\n shasx r10, r0, r1\n usub16 r11, r1, r0\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x8000_0003);
    assert_eq!(machine.register(3), 0x7FFF_0003);
    assert_eq!(machine.register(6), 0x0000_0000);
    assert_eq!(
        machine.register(7),
        0xFF01_80FF,
        "UADD8 carried out of every byte"
    );
    assert_eq!(machine.register(8), 0x4000_0001);
    assert_eq!(machine.register(9), 0x8102_0001);
    assert_eq!(machine.register(10), 0x4000_0000);
    assert_eq!(machine.register(11), 0x8002_0001);
}

#[test]
fn pack_halfwords() {
    let machine = run(
        "ldr r0, =0x11112222\n ldr r1, =0x33334444\n pkhbt r2, r0, r1, lsl #16\n pkhtb r3, r0, r1, asr #16\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x4444_2222);
    assert_eq!(machine.register(3), 0x1111_3333);
}

#[test]
fn halfword_multiplies() {
    let machine = run(
        "ldr r0, =0x0003FFFE\n ldr r1, =0x00050007\n smulbb r2, r0, r1\n smultt r3, r0, r1\n smulbt r4, r0, r1\n\
         movs r5, #100\n smlabb r6, r0, r1, r5\n smuad r7, r0, r1\n smusdx r8, r0, r1\n smulwb r9, r0, r1\n\
         movs r10, #0\n movs r11, #0\n smlalbb r10, r11, r0, r1\n bkpt #0",
    );
    assert_eq!(machine.register(2) as i32, -14);
    assert_eq!(machine.register(3), 15);
    assert_eq!(machine.register(4) as i32, -10);
    assert_eq!(machine.register(6), 86);
    assert_eq!(machine.register(7), 1);
    assert_eq!(machine.register(8) as i32, -2 * 5 - 3 * 7);
    assert_eq!(machine.register(9), ((0x0003_FFFEi64 * 7) >> 16) as u32);
    assert_eq!(
        (machine.register(10) as i32, machine.register(11)),
        (-14, 0xFFFF_FFFF)
    );
}

#[test]
fn most_significant_word_multiplies_and_absolute_differences() {
    let machine = run(
        "ldr r0, =0x40000000\n ldr r1, =0x00000006\n smmul r2, r0, r1\n ldr r3, =0x00000003\n smmlar r4, r0, r3, r1\n\
         smmls r5, r0, r1, r1\n ldr r6, =0x01020304\n ldr r7, =0x04030201\n usad8 r8, r6, r7\n usada8 r9, r6, r7, r1\n\
         movs r10, #0\n movs r11, #0\n smlald r10, r11, r6, r7\n bkpt #0",
    );
    assert_eq!(machine.register(2), 1);
    assert_eq!(machine.register(4), 7);
    assert_eq!(machine.register(5), 4);
    assert_eq!(machine.register(8), 8);
    assert_eq!(machine.register(9), 14);
    assert_eq!(machine.register(10), 0x0102 * 0x0403 + 0x0304 * 0x0201);
}
