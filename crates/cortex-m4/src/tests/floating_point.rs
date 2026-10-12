// SPDX-License-Identifier: GPL-3.0-only
//! FPv4-SP instructions: arithmetic, NaN and flag rules, compares, conversions, loads and moves.

use super::support::run;

const ENABLE_FPU: &str = "ldr r0, =0xE000ED88\n ldr r1, =0x00F00000\n str r1, [r0]\n";

fn run_float(body: &str) -> super::support::Machine {
    run(&format!("{ENABLE_FPU}{body}\n bkpt #0"))
}

fn single(machine: &super::support::Machine, number: usize) -> f32 {
    f32::from_bits(machine.cpu.float_register(number))
}

#[test]
fn arithmetic() {
    let machine = run_float(
        "vmov.f32 s0, #1.5\n vmov.f32 s1, #-2.0\n vadd.f32 s2, s0, s1\n vsub.f32 s3, s0, s1\n vmul.f32 s4, s0, s1\n\
         vdiv.f32 s5, s0, s1\n vnmul.f32 s6, s0, s1\n vabs.f32 s7, s1\n vneg.f32 s8, s0\n vmov.f32 s9, #4.0\n vsqrt.f32 s10, s9\n\
         vmov.f32 s11, s0",
    );
    assert_eq!(single(&machine, 2), -0.5);
    assert_eq!(single(&machine, 3), 3.5);
    assert_eq!(single(&machine, 4), -3.0);
    assert_eq!(single(&machine, 5), -0.75);
    assert_eq!(single(&machine, 6), 3.0);
    assert_eq!(single(&machine, 7), 2.0);
    assert_eq!(single(&machine, 8), -1.5);
    assert_eq!(single(&machine, 10), 2.0);
    assert_eq!(single(&machine, 11), 1.5);
}

#[test]
fn multiply_accumulate_variants() {
    let machine = run_float(
        "vmov.f32 s0, #2.0\n vmov.f32 s1, #3.0\n vmov.f32 s2, #1.0\n vmla.f32 s2, s0, s1\n\
         vmov.f32 s3, #1.0\n vmls.f32 s3, s0, s1\n vmov.f32 s4, #1.0\n vnmla.f32 s4, s0, s1\n\
         vmov.f32 s5, #1.0\n vnmls.f32 s5, s0, s1\n vmov.f32 s6, #1.0\n vfma.f32 s6, s0, s1\n\
         vmov.f32 s7, #1.0\n vfms.f32 s7, s0, s1\n vmov.f32 s8, #1.0\n vfnma.f32 s8, s0, s1\n\
         vmov.f32 s9, #1.0\n vfnms.f32 s9, s0, s1",
    );
    let results: Vec<f32> = (2..10).map(|number| single(&machine, number)).collect();
    assert_eq!(results, vec![7.0, -5.0, -7.0, 5.0, 7.0, -5.0, -7.0, 5.0]);
}

#[test]
fn compare_sets_fpscr_and_vmrs_copies_flags() {
    let machine = run_float(
        "vmov.f32 s0, #1.0\n vmov.f32 s1, #2.0\n vcmp.f32 s0, s1\n vmrs APSR_nzcv, fpscr\n mrs r4, apsr\n\
         vcmp.f32 s1, s1\n vmrs r5, fpscr\n vcmp.f32 s0, #0.0\n vmrs r6, fpscr",
    );
    assert_eq!(machine.register(4) >> 28, 0b1000);
    assert_eq!(machine.register(5) >> 28, 0b0110);
    assert_eq!(machine.register(6) >> 28, 0b0010);
}

#[test]
fn nan_rules_and_cumulative_flags() {
    let machine = run_float(
        "ldr r0, =0x7F800000\n vmov s0, r0\n vsub.f32 s1, s0, s0\n vmrs r4, fpscr\n\
         ldr r0, =0x7F800001\n vmov s2, r0\n vmov.f32 s3, #1.0\n vadd.f32 s4, s3, s2\n\
         ldr r1, =0\n vmov s5, r1\n vdiv.f32 s6, s3, s5\n vmrs r5, fpscr\n vcmpe.f32 s4, s3\n vmrs r6, fpscr",
    );
    assert_eq!(
        machine.cpu.float_register(1),
        0x7FC0_0000,
        "default NaN for inf - inf"
    );
    assert_eq!(machine.register(4) & 1, 1, "IOC");
    assert_eq!(
        machine.cpu.float_register(4),
        0x7FC0_0001,
        "quieted signalling NaN propagates"
    );
    assert_eq!(machine.cpu.float_register(6), 0x7F80_0000);
    assert_eq!(machine.register(5) & 2, 2, "DZC");
    assert_eq!(machine.register(6) >> 28, 0b0011, "unordered");
}

#[test]
fn flush_to_zero_and_default_nan_modes() {
    let machine = run_float(
        "ldr r0, =0x03000000\n vmsr fpscr, r0\n ldr r1, =0x00000001\n vmov s0, r1\n vmov.f32 s1, #1.0\n vadd.f32 s2, s0, s1\n\
         vmul.f32 s3, s0, s1\n ldr r1, =0x7F800001\n vmov s4, r1\n vadd.f32 s5, s4, s1\n vmrs r4, fpscr",
    );
    assert_eq!(single(&machine, 2), 1.0);
    assert_eq!(machine.cpu.float_register(3), 0);
    assert_eq!(machine.cpu.float_register(5), 0x7FC0_0000);
    assert_ne!(machine.register(4) & 0x80, 0, "IDC");
}

#[test]
fn integer_conversions() {
    let machine = run_float(
        "ldr r0, =0xC0700000\n vmov s0, r0\n vcvt.s32.f32 s1, s0\n vcvtr.s32.f32 s2, s0\n vcvt.u32.f32 s3, s0\n\
         movs r1, #7\n vmov s4, r1\n vcvt.f32.s32 s5, s4\n ldr r1, =0xFFFFFFFF\n vmov s6, r1\n vcvt.f32.u32 s7, s6\n\
         vcvt.f32.s32 s8, s6\n ldr r0, =0x4F800000\n vmov s9, r0\n vcvt.s32.f32 s10, s9\n vmrs r4, fpscr",
    );
    assert_eq!(machine.cpu.float_register(1) as i32, -3, "-3.75 truncates");
    assert_eq!(
        machine.cpu.float_register(2) as i32,
        -4,
        "VCVTR rounds to nearest"
    );
    assert_eq!(
        machine.cpu.float_register(3),
        0,
        "negative saturates to 0 unsigned"
    );
    assert_eq!(single(&machine, 5), 7.0);
    assert_eq!(single(&machine, 7), 4294967296.0);
    assert_eq!(single(&machine, 8), -1.0);
    assert_eq!(machine.cpu.float_register(10), 0x7FFF_FFFF, "2^32 saturates");
    assert_eq!(machine.register(4) & 1, 1);
}

#[test]
fn fixed_point_and_half_precision_conversions() {
    let machine = run_float(
        "vmov.f32 s0, #1.5\n vcvt.s32.f32 s0, s0, #16\n vmov r4, s0\n vcvt.f32.s32 s0, s0, #16\n\
         movs r1, #0\n subs r1, #8\n vmov s1, r1\n vcvt.f32.s16 s1, s1, #2\n\
         vmov.f32 s2, #-2.5\n vcvtb.f16.f32 s3, s2\n vcvtt.f16.f32 s3, s0\n vcvtt.f32.f16 s4, s3\n vcvtb.f32.f16 s5, s3",
    );
    assert_eq!(machine.register(4), 0x0001_8000);
    assert_eq!(single(&machine, 0), 1.5);
    assert_eq!(single(&machine, 1), -2.0);
    assert_eq!(machine.cpu.float_register(3), 0x3E00_C100);
    assert_eq!(single(&machine, 4), 1.5);
    assert_eq!(single(&machine, 5), -2.5);
}

#[test]
fn loads_stores_and_core_moves() {
    let machine = run_float(
        "ldr r0, =0x20000100\n vmov.f32 s0, #1.0\n vmov.f32 s1, #2.0\n vstr s0, [r0]\n vstr d0, [r0, #8]\n\
         vldr s2, [r0, #12]\n vpush {s0, s1}\n vpop {s4, s5}\n vstmia r0!, {s0-s1}\n vldmdb r0!, {s6-s7}\n\
         vmov r4, r5, d0\n vmov s8, s9, r5, r4\n vmov r6, s9\n vldr s10, value\n vmov s11, r6\n b done\n\
         .balign 4\nvalue: .word 0x40490FDB\ndone:",
    );
    assert_eq!(single(&machine, 2), 2.0);
    assert_eq!((single(&machine, 4), single(&machine, 5)), (1.0, 2.0));
    assert_eq!((single(&machine, 6), single(&machine, 7)), (1.0, 2.0));
    assert_eq!(machine.register(0), 0x2000_0100);
    assert_eq!(
        (machine.register(4), machine.register(5)),
        (1.0f32.to_bits(), 2.0f32.to_bits())
    );
    assert_eq!((single(&machine, 8), single(&machine, 9)), (2.0, 1.0));
    assert_eq!(machine.register(6), 1.0f32.to_bits());
    assert_eq!(machine.cpu.float_register(10), 0x4049_0FDB);
    assert_eq!(machine.cpu.sp(), 0x2001_0000);
}
