// SPDX-License-Identifier: GPL-3.0-only
//! Loads and stores: addressing modes, sizes, unaligned access, exclusives and multiples.

use super::support::{run, Machine, RAM_START};

#[test]
fn store_and_load_every_size() {
    let mut machine = run(
        "ldr r0, =0x20000100\n ldr r1, =0x8899AABB\n str r1, [r0]\n strh r1, [r0, #4]\n strb r1, [r0, #6]\n\
         ldr r2, [r0]\n ldrh r3, [r0]\n ldrsh r4, [r0]\n ldrb r5, [r0]\n ldrsb r6, [r0]\n ldr r7, [r0, #4]\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x8899_AABB);
    assert_eq!(machine.register(3), 0xAABB);
    assert_eq!(machine.register(4), 0xFFFF_AABB);
    assert_eq!(machine.register(5), 0xBB);
    assert_eq!(machine.register(6), 0xFFFF_FFBB);
    assert_eq!(machine.register(7), 0xFFBB_AABB & 0x00FF_FFFF);
    assert_eq!(machine.bus.read_word(RAM_START + 0x100), 0x8899_AABB);
}

#[test]
fn pre_and_post_indexed_writeback() {
    let machine = run(
        "ldr r0, =0x20000100\n movs r1, #7\n str r1, [r0, #8]!\n str r1, [r0], #-4\n ldr r2, [r0, #4]\n bkpt #0",
    );
    assert_eq!(machine.register(0), 0x2000_0104);
    assert_eq!(machine.register(2), 7);
}

#[test]
fn register_offset_and_literal_loads() {
    let machine = run(
        "ldr r0, =0x20000100\n movs r1, #2\n ldr r2, =0x55\n str.w r2, [r0, r1, lsl #2]\n ldr.w r3, [r0, r1, lsl #2]\n\
         ldr r4, value\n bkpt #0\n.balign 4\nvalue: .word 0xCAFEF00D",
    );
    assert_eq!(machine.register(3), 0x55);
    assert_eq!(machine.register(4), 0xCAFE_F00D);
}

#[test]
fn unaligned_word_access_works_unless_trapped() {
    let machine = run(
        "ldr r0, =0x20000101\n ldr r1, =0x11223344\n str r1, [r0]\n ldr r2, [r0]\n ldrh r3, [r0, #1]\n bkpt #0",
    );
    assert_eq!(machine.register(2), 0x1122_3344);
    assert_eq!(machine.register(3), 0x2233);

    let mut machine = Machine::new(
        "ldr r0, =0xE000ED14\n ldr r1, [r0]\n orr r1, #8\n str r1, [r0]\n ldr r0, =0x20000101\n ldr r1, [r0]\n bkpt #1\n\
         vector_3:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n ldr r0, =0xE000ED2C\n ldr r6, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), 1 << 24);
    assert_eq!(machine.register(6), 1 << 30);
}

#[test]
fn dual_loads_and_stores() {
    let machine = run(
        "ldr r0, =0x20000100\n movs r1, #1\n movs r2, #2\n strd r1, r2, [r0, #8]\n ldrd r3, r4, [r0, #8]!\n bkpt #0",
    );
    assert_eq!((machine.register(3), machine.register(4)), (1, 2));
    assert_eq!(machine.register(0), 0x2000_0108);
}

#[test]
fn multiple_loads_stores_push_and_pop() {
    let machine = run(
        "movs r0, #1\n movs r1, #2\n movs r2, #3\n push {r0-r2, lr}\n movs r0, #0\n movs r1, #0\n movs r2, #0\n\
         pop {r3-r5}\n pop {r6}\n ldr r7, =0x20000200\n stmia r7!, {r3-r5}\n ldmdb r7, {r8-r10}\n\
         stmdb r7!, {r0, r1}\n bkpt #0",
    );
    assert_eq!(
        (machine.register(3), machine.register(4), machine.register(5)),
        (1, 2, 3)
    );
    assert_eq!(machine.register(6), 0xFFFF_FFFF);
    assert_eq!(
        (machine.register(8), machine.register(9), machine.register(10)),
        (1, 2, 3)
    );
    assert_eq!(machine.register(7), 0x2000_0204);
    assert_eq!(machine.cpu.sp(), 0x2001_0000);
}

#[test]
fn pop_into_pc_returns() {
    let machine = run(
        "bl function\n movs r1, #5\n bkpt #0\n.thumb_func\nfunction:\n push {r4, lr}\n movs r0, #9\n pop {r4, pc}",
    );
    assert_eq!(machine.register(0), 9);
    assert_eq!(machine.register(1), 5);
}

#[test]
fn exclusive_store_succeeds_once_after_load_exclusive() {
    let machine = run(
        "ldr r0, =0x20000100\n movs r1, #5\n ldrex r2, [r0]\n strex r3, r1, [r0]\n strex r4, r1, [r0]\n\
         ldrex r5, [r0]\n clrex\n strex r6, r1, [r0]\n ldrexb r7, [r0]\n strexb r8, r1, [r0, #0]\n bkpt #0",
    );
    assert_eq!(machine.register(3), 0);
    assert_eq!(machine.register(4), 1);
    assert_eq!(machine.register(5), 5);
    assert_eq!(machine.register(6), 1);
    assert_eq!(machine.register(8), 0);
}

#[test]
fn exception_entry_clears_the_exclusive_monitor() {
    let machine = run(
        "ldr r0, =0x20000100\n ldrex r2, [r0]\n svc #0\n strex r3, r1, [r0]\n bkpt #0\n\
         vector_11:\n bx lr",
    );
    assert_eq!(machine.register(3), 1);
}

#[test]
fn unmapped_load_raises_a_precise_bus_fault() {
    let mut machine = Machine::new(
        "ldr r0, =0x60000000\n ldr r1, [r0]\n bkpt #1\n\
         vector_3:\n ldr r0, =0xE000ED28\n ldr r5, [r0]\n ldr r0, =0xE000ED38\n ldr r6, [r0]\n bkpt #2",
    );
    machine.run_to_breakpoint();
    assert_eq!(machine.register(5), (1 << 9) | (1 << 15));
    assert_eq!(machine.register(6), 0x6000_0000);
    assert_eq!(machine.cpu.fault_status().bus_fault_address, 0x6000_0000);
}

#[test]
fn rewritten_code_in_ram_is_decoded_again() {
    let machine = run(
        "ldr r0, =0x20000101\n ldr r1, =0x47702001\n str r1, [r0, #-1]\n blx r0\n mov r2, r0\n ldr r0, =0x20000101\n\
         ldr r1, =0x47702007\n str r1, [r0, #-1]\n blx r0\n bkpt #0",
    );
    assert_eq!(machine.register(2), 1);
    assert_eq!(machine.register(0), 7);
}
