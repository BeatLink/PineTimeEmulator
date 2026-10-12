// SPDX-License-Identifier: GPL-3.0-only
//! Data processing, shifts, flags, IT blocks, multiplies, divides and bit-field instructions.

use super::support::{run, Machine};
use crate::StopReason;

const N: u32 = 0b1000;
const Z: u32 = 0b0100;
const C: u32 = 0b0010;
const V: u32 = 0b0001;

// ============================================================================
// Reset and harness
// ============================================================================

#[test]
fn reset_loads_stack_pointer_and_entry_from_the_vector_table() {
    let machine = Machine::new("bkpt #0");
    assert_eq!(machine.cpu.sp(), 0x2001_0000);
    assert_eq!(machine.cpu.pc(), 0x100);
    assert_eq!(machine.cpu.xpsr(), 1 << 24);
    assert_eq!(machine.register(14), 0xFFFF_FFFF);
}

// ============================================================================
// Add, subtract and flags
// ============================================================================

#[test]
fn adds_sets_carry_and_overflow() {
    let machine = run("ldr r0, =0x7FFFFFFF\n movs r1, #1\n adds r2, r0, r1\n bkpt #0");
    assert_eq!(machine.register(2), 0x8000_0000);
    assert_eq!(machine.flags(), N | V);

    let machine = run("ldr r0, =0xFFFFFFFF\n adds r1, r0, #1\n bkpt #0");
    assert_eq!(machine.register(1), 0);
    assert_eq!(machine.flags(), Z | C);
}

#[test]
fn subtract_compare_and_reverse_subtract() {
    let machine = run("movs r0, #5\n subs r1, r0, #7\n bkpt #0");
    assert_eq!(machine.register(1), (-2i32) as u32);
    assert_eq!(machine.flags(), N);

    let machine = run("movs r0, #7\n cmp r0, #7\n bkpt #0");
    assert_eq!(machine.flags(), Z | C);

    let machine = run("movs r0, #3\n rsbs r1, r0, #0\n rsb.w r2, r0, #10\n bkpt #0");
    assert_eq!(machine.register(1), (-3i32) as u32);
    assert_eq!(machine.register(2), 7);
}

#[test]
fn add_and_subtract_with_carry() {
    let machine = run(
        "ldr r0, =0xFFFFFFFF\n movs r1, #1\n adds r2, r0, r1\n movs r3, #0\n adcs r3, r3, #0\n\
         movs r4, #10\n movs r5, #3\n cmp r4, r4\n sbcs r4, r4, r5\n bkpt #0",
    );
    assert_eq!(machine.register(3), 1);
    assert_eq!(machine.register(4), 7);
}

#[test]
fn cmn_tst_and_teq_only_set_flags() {
    let machine = run("movs r0, #1\n ldr r1, =0xFFFFFFFF\n cmn r0, r1\n bkpt #0");
    assert_eq!(machine.flags(), Z | C);
    let machine = run("movs r0, #0xF0\n tst r0, #0x0F\n bkpt #0");
    assert_eq!(machine.flags() & Z, Z);
    let machine = run("movs r0, #0xF0\n teq r0, #0xF0\n bkpt #0");
    assert_eq!(machine.flags() & Z, Z);
}

#[test]
fn add_to_stack_pointer_and_address_of_label() {
    let machine = run("sub sp, #16\n add r0, sp, #8\n adr r1, here\n nop\n.balign 4\nhere:\n bkpt #0");
    assert_eq!(machine.cpu.sp(), 0x2000_FFF0);
    assert_eq!(machine.register(0), 0x2000_FFF8);
    assert_eq!(machine.register(1), machine.cpu.pc());
}

#[test]
fn wide_moves_and_move_top() {
    let machine = run("movw r0, #0x1234\n movt r0, #0xABCD\n mov.w r1, #0x00FF00FF\n mvn r2, #0\n bkpt #0");
    assert_eq!(machine.register(0), 0xABCD_1234);
    assert_eq!(machine.register(1), 0x00FF_00FF);
    assert_eq!(machine.register(2), 0xFFFF_FFFF);
}

// ============================================================================
// Logical operations and shifts
// ============================================================================

#[test]
fn logical_operations() {
    let machine = run(
        "ldr r0, =0xF0F0F0F0\n ldr r1, =0xFF00FF00\n and r2, r0, r1\n orr r3, r0, r1\n eor r4, r0, r1\n\
         bic r5, r0, r1\n orn r6, r0, r1\n mvn r7, r0\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0xF000_F000);
    assert_eq!(machine.register(3), 0xFFF0_FFF0);
    assert_eq!(machine.register(4), 0x0FF0_0FF0);
    assert_eq!(machine.register(5), 0x00F0_00F0);
    assert_eq!(machine.register(6), 0xF0FF_F0FF);
    assert_eq!(machine.register(7), 0x0F0F_0F0F);
}

#[test]
fn modified_immediate_with_rotation_sets_carry_on_logical_ops() {
    let machine = run("movs r0, #0\n ands r1, r0, #0x80000000\n bkpt #0");
    assert_eq!(machine.flags(), Z | C);
}

#[test]
fn immediate_shifts_and_their_carry() {
    let machine = run("movs r0, #0x81\n lsls r1, r0, #25\n bkpt #0");
    assert_eq!(machine.register(1), 0x0200_0000);
    assert_eq!(machine.flags(), C);

    let machine = run("ldr r0, =0x80000001\n lsrs r1, r0, #1\n asrs r2, r0, #1\n bkpt #0");
    assert_eq!(machine.register(1), 0x4000_0000);
    assert_eq!(machine.register(2), 0xC000_0000);
    assert_eq!(machine.flags(), N | C);

    let machine = run("ldr r0, =0x80000000\n asrs r1, r0, #32\n bkpt #0");
    assert_eq!(machine.register(1), 0xFFFF_FFFF);
}

#[test]
fn rotate_and_rotate_through_carry() {
    let machine = run("movs r0, #1\n ror r1, r0, #1\n movs r2, #3\n cmp r2, r2\n rrx r3, r2\n bkpt #0");
    assert_eq!(machine.register(1), 0x8000_0000);
    assert_eq!(machine.register(3), 0x8000_0001);
}

#[test]
fn register_shifts_handle_large_amounts() {
    let machine = run("movs r0, #1\n movs r1, #32\n lsls r2, r0, r1\n bkpt #0");
    assert_eq!(machine.register(2), 0);
    assert_eq!(machine.flags(), Z | C);

    let machine = run("movs r0, #0xF0\n movs r1, #36\n rors r0, r1\n bkpt #0");
    assert_eq!(machine.register(0), 0x0000_000F);
}

#[test]
fn shifted_register_operand() {
    let machine =
        run("movs r0, #1\n movs r1, #3\n add.w r2, r0, r1, lsl #4\n sub.w r3, r1, r0, asr #1\n bkpt #0");
    assert_eq!(machine.register(2), 0x31);
    assert_eq!(machine.register(3), 3);
}

// ============================================================================
// IT blocks and conditional branches
// ============================================================================

#[test]
fn it_block_runs_only_matching_instructions_and_keeps_flags() {
    let machine = run(
        "movs r0, #1\n cmp r0, #1\n ite eq\n moveq r1, #10\n movne r1, #20\n\
         itt ne\n addne r2, r2, #1\n addne r2, r2, #1\n bkpt #0",
    );
    assert_eq!(machine.register(1), 10);
    assert_eq!(machine.register(2), 0);
}

#[test]
fn sixteen_bit_adds_inside_it_does_not_set_flags() {
    let machine = run("movs r0, #0\n cmp r0, #0\n it eq\n addeq r0, r0, #1\n bkpt #0");
    assert_eq!(machine.register(0), 1);
    assert_eq!(machine.flags(), Z | C);
}

#[test]
fn conditional_branches_and_compare_branch() {
    let machine = run(
        "movs r0, #0\n movs r1, #5\nloop:\n adds r0, r0, #2\n subs r1, r1, #1\n bne loop\n\
         movs r2, #0\n cbz r2, skip\n movs r3, #99\nskip:\n cbnz r2, never\n movs r4, #1\nnever:\n bkpt #0",
    );
    assert_eq!(machine.register(0), 10);
    assert_eq!(machine.register(3), 0);
    assert_eq!(machine.register(4), 1);
}

#[test]
fn signed_conditions() {
    let machine = run(
        "movs r0, #0\n subs r0, r0, #1\n cmp r0, #1\n ite lt\n movlt r1, #1\n movge r1, #2\n\
         ite hi\n movhi r2, #1\n movls r2, #2\n bkpt #0",
    );
    assert_eq!(machine.register(1), 1);
    assert_eq!(machine.register(2), 1);
}

#[test]
fn branch_with_link_and_return() {
    let machine = run("movs r0, #1\n bl function\n adds r0, r0, #10\n bkpt #0\n\
         .thumb_func\nfunction:\n adds r0, r0, #100\n bx lr");
    assert_eq!(machine.register(0), 111);
}

#[test]
fn branch_link_exchange_register() {
    let machine = run("adr r1, function + 1\n blx r1\n bkpt #0\n.balign 4\nfunction:\n movs r0, #42\n bx lr");
    assert_eq!(machine.register(0), 42);
}

#[test]
fn table_branches() {
    let machine = run(
        "movs r0, #2\n tbb [pc, r0]\ntable:\n .byte (case0 - table) / 2, (case1 - table) / 2, (case2 - table) / 2, 0\n\
         case0:\n movs r1, #10\n b done\ncase1:\n movs r1, #11\n b done\ncase2:\n movs r1, #12\n\
         done:\n movs r0, #1\n tbh [pc, r0, lsl #1]\nhtable:\n .hword (h0 - htable) / 2, (h1 - htable) / 2\n\
         h0:\n movs r2, #20\n b end\nh1:\n movs r2, #21\nend:\n bkpt #0",
    );
    assert_eq!(machine.register(1), 12);
    assert_eq!(machine.register(2), 21);
}

// ============================================================================
// Multiplies and divides
// ============================================================================

#[test]
fn multiplies() {
    let machine = run(
        "movs r0, #7\n movs r1, #6\n mul r2, r0, r1\n movs r3, #100\n mla r4, r0, r1, r3\n mls r5, r0, r1, r3\n bkpt #0",
    );
    assert_eq!(machine.register(2), 42);
    assert_eq!(machine.register(4), 142);
    assert_eq!(machine.register(5), 58);
}

#[test]
fn long_multiplies() {
    let machine = run(
        "ldr r0, =0xFFFFFFFF\n movs r1, #2\n umull r2, r3, r0, r1\n smull r4, r5, r0, r1\n\
         movs r6, #1\n movs r7, #0\n umlal r6, r7, r0, r1\n\
         ldr r8, =0xFFFFFFFF\n ldr r9, =0xFFFFFFFF\n umaal r8, r9, r0, r0\n bkpt #0",
    );
    assert_eq!((machine.register(2), machine.register(3)), (0xFFFF_FFFE, 1));
    assert_eq!(
        (machine.register(4), machine.register(5)),
        (0xFFFF_FFFE, 0xFFFF_FFFF)
    );
    assert_eq!((machine.register(6), machine.register(7)), (0xFFFF_FFFF, 1));
    assert_eq!(
        (machine.register(8), machine.register(9)),
        (0xFFFF_FFFF, 0xFFFF_FFFF)
    );
}

#[test]
fn divides_round_toward_zero_and_return_zero_for_zero_divisor() {
    let machine = run(
        "movs r0, #0\n subs r0, r0, #7\n movs r1, #2\n sdiv r2, r0, r1\n udiv r3, r1, r1\n movs r4, #0\n udiv r5, r1, r4\n bkpt #0",
    );
    assert_eq!(machine.register(2) as i32, -3);
    assert_eq!(machine.register(3), 1);
    assert_eq!(machine.register(5), 0);
}

#[test]
fn divide_by_zero_traps_when_enabled() {
    let mut machine = Machine::new(
        "ldr r0, =0xE000ED14\n ldr r1, [r0]\n orr r1, r1, #0x10\n str r1, [r0]\n\
         ldr r0, =0xE000ED24\n ldr r1, =0x00070000\n str r1, [r0]\n\
         movs r2, #1\n movs r3, #0\n udiv r4, r2, r3\n bkpt #1\n\
         vector_6:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), 1 << 25);
    assert_eq!(machine.cpu.xpsr() & 0x1FF, 6);
}

// ============================================================================
// Bit fields, reversal and extension
// ============================================================================

#[test]
fn bit_field_operations() {
    let machine = run(
        "ldr r0, =0xFFFFFFFF\n bfc r0, #4, #8\n ldr r1, =0x12345678\n movs r2, #0xAB\n bfi r1, r2, #8, #8\n\
         ldr r3, =0x00000F00\n sbfx r4, r3, #8, #4\n ubfx r5, r3, #8, #4\n bkpt #0",
    );
    assert_eq!(machine.register(0), 0xFFFF_F00F);
    assert_eq!(machine.register(1), 0x1234_AB78);
    assert_eq!(machine.register(4), 0xFFFF_FFFF);
    assert_eq!(machine.register(5), 0xF);
}

#[test]
fn counting_and_reversal() {
    let machine = run(
        "ldr r0, =0x00F01234\n clz r1, r0\n rbit r2, r0\n rev r3, r0\n rev16 r4, r0\n revsh r5, r0\n bkpt #0",
    );
    assert_eq!(machine.register(1), 8);
    assert_eq!(machine.register(2), 0x00F01234u32.reverse_bits());
    assert_eq!(machine.register(3), 0x3412_F000);
    assert_eq!(machine.register(4), 0xF000_3412);
    assert_eq!(machine.register(5), 0x0000_3412);
}

#[test]
fn sign_and_zero_extension() {
    let machine = run(
        "ldr r0, =0x8081F0F1\n sxtb r1, r0\n uxtb r2, r0\n sxth r3, r0\n uxth r4, r0\n\
         sxtb r5, r0, ror #8\n movs r6, #1\n uxtab r7, r6, r0\n sxtb16 r8, r0\n uxtb16 r9, r0, ror #8\n bkpt #0",
    );
    assert_eq!(machine.register(1), 0xFFFF_FFF1);
    assert_eq!(machine.register(2), 0xF1);
    assert_eq!(machine.register(3), 0xFFFF_F0F1);
    assert_eq!(machine.register(4), 0xF0F1);
    assert_eq!(machine.register(5), 0xFFFF_FFF0);
    assert_eq!(machine.register(7), 0xF2);
    assert_eq!(machine.register(8), 0xFF81_FFF1);
    assert_eq!(machine.register(9), 0x0080_00F0);
}

#[test]
fn breakpoint_stops_and_resumes_past_it() {
    let mut machine = Machine::new("movs r0, #1\n bkpt #3\n movs r0, #2\n bkpt #4");
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(machine.register(0), 1);
    assert_eq!(machine.run(), StopReason::Breakpoint);
    assert_eq!(machine.register(0), 2);
}
