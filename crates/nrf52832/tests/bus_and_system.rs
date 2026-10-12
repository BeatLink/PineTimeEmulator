// SPDX-License-Identifier: GPL-3.0-only
//! The memory bus, watchpoints, interrupt lines, PPI groups and forks, and the peripherals Renode
//! supplied itself: UART(E), EGU, RNG and ECB.

mod common;

use common::Harness;
use pinetime_core::{AccessSize, BusError};

const UART: u32 = 0x4000_2000;
const EGU0: u32 = 0x4001_4000;
const EGU1: u32 = 0x4001_5000;
const RNG: u32 = 0x4000_D000;
const ECB: u32 = 0x4000_E000;
const PPI: u32 = 0x4001_F000;

// ============================================================================
// Memory bus
// ============================================================================

#[test]
fn ram_answers_every_access_size_and_its_code_alias() {
    let mut harness = Harness::new();
    harness.write(0x2000_0010, 0x1122_3344);
    assert_eq!(harness.read(0x2000_0010), 0x1122_3344);
    assert_eq!(harness.read_halfword(0x2000_0012), 0x1122);
    let byte = harness
        .with_ports(|chip, ports| chip.read(0x2000_0011, AccessSize::Byte, ports))
        .unwrap();
    assert_eq!(byte, 0x33);
    assert_eq!(harness.read(0x0080_0010), 0x1122_3344);
    assert_eq!(harness.chip.fetch(0x0080_0010), Ok(0x3344));
    assert_eq!(harness.chip.ram()[0x10], 0x44);
}

#[test]
fn fetch_reads_flash_and_ram_only() {
    let mut harness = Harness::new();
    harness.chip.flash_mut()[0x100..0x102].copy_from_slice(&[0xFE, 0xE7]);
    assert_eq!(harness.chip.fetch(0x100), Ok(0xE7FE));
    assert_eq!(harness.chip.fetch(0x4000_0000), Err(BusError::Unmapped));
    assert_eq!(harness.chip.fetch(0x1000_0000), Err(BusError::Unmapped));
}

#[test]
fn unmapped_addresses_fault() {
    let mut harness = Harness::new();
    let read = harness.with_ports(|chip, ports| chip.read(0x3000_0000, AccessSize::Word, ports));
    assert_eq!(read, Err(BusError::Unmapped));
    let write = harness.with_ports(|chip, ports| chip.write(0x0008_0000, 0, AccessSize::Word, ports));
    assert_eq!(write, Err(BusError::Unmapped));
}

#[test]
fn undocumented_peripheral_registers_keep_what_is_written() {
    let mut harness = Harness::new();
    harness.write(0x4006_EC00, 0x9375);
    assert_eq!(harness.read(0x4006_EC00), 0x9375);
    harness.write(0x4000_0EE4, 0x4F);
    assert_eq!(harness.read(0x4000_0EE4), 0x4F);
    assert!(harness.chip.unhandled_access_count() >= 2);
}

#[test]
fn byte_writes_reach_their_part_of_a_register() {
    let mut harness = Harness::new();
    harness
        .with_ports(|chip, ports| chip.write(0x4000_051D, 0, AccessSize::Byte, ports))
        .unwrap();
    harness
        .with_ports(|chip, ports| chip.write(0x4000_051C, 0xA5, AccessSize::Byte, ports))
        .unwrap();
    assert_eq!(harness.read(0x4000_051C), 0xA5);
}

#[test]
fn debugger_writes_reach_flash_and_ficr_and_skip_watchpoints() {
    let mut harness = Harness::new();
    harness.chip.add_watchpoint(0x1000, 4);
    harness
        .with_ports(|chip, ports| chip.write_for_debugger(0x1000, 0x1234, AccessSize::Word, ports))
        .unwrap();
    harness
        .with_ports(|chip, ports| chip.write_for_debugger(0x1000_0010, 7, AccessSize::Word, ports))
        .unwrap();
    let value = harness
        .with_ports(|chip, ports| chip.read_for_debugger(0x1000, AccessSize::Word, ports))
        .unwrap();
    assert_eq!(value, 0x1234);
    assert_eq!(harness.chip.ficr_mut()[0x10], 7);
    assert_eq!(harness.chip.take_watchpoint_hit(), None);
}

#[test]
fn watchpoints_report_the_first_access_and_ask_the_cpu_to_stop() {
    let mut harness = Harness::new();
    harness.chip.add_watchpoint(0x2000_0100, 4);
    harness.write(0x2000_0200, 1);
    assert!(!harness.chip.stop_requested());
    harness.write(0x2000_0102, 0xBEEF);
    assert!(harness.chip.stop_requested());
    harness.read(0x2000_0100);
    assert_eq!(
        harness.chip.take_watchpoint_hit(),
        Some((0x2000_0102, 0xBEEF, true))
    );
    assert!(!harness.chip.stop_requested());
    harness.read(0x2000_0100);
    assert_eq!(
        harness.chip.take_watchpoint_hit(),
        Some((0x2000_0100, 0xBEEF << 16, false))
    );
    harness.chip.remove_watchpoint(0x2000_0100);
    harness.read(0x2000_0100);
    assert_eq!(harness.chip.take_watchpoint_hit(), None);
}

#[test]
fn interrupt_lines_are_reported_only_when_they_change() {
    let mut harness = Harness::new();
    assert_eq!(harness.chip.take_interrupt_lines(), None);
    harness.write(EGU0 + 0x304, 0x1);
    assert_eq!(harness.chip.take_interrupt_lines(), None);
    harness.write(EGU0, 1);
    assert_eq!(harness.chip.take_interrupt_lines(), Some(1 << 20));
    harness.write(EGU0, 1);
    assert_eq!(harness.chip.take_interrupt_lines(), None);
    harness.write(EGU0 + 0x100, 0);
    assert_eq!(harness.chip.take_interrupt_lines(), Some(0));
}

// ============================================================================
// PPI
// ============================================================================

#[test]
fn ppi_fork_runs_a_second_task() {
    let mut harness = Harness::new();
    harness.write(PPI + 0x510, EGU0 + 0x100);
    harness.write(PPI + 0x514, EGU1);
    harness.write(PPI + 0x910, EGU1 + 0x004);
    harness.write(PPI + 0x504, 1);
    harness.write(EGU0, 1);
    assert_eq!(harness.read(EGU1 + 0x100), 1);
    assert_eq!(harness.read(EGU1 + 0x104), 1);
}

#[test]
fn ppi_channel_groups_enable_and_disable_channels() {
    let mut harness = Harness::new();
    harness.write(PPI + 0x800, 0b101);
    harness.write(PPI, 1);
    assert_eq!(harness.read(PPI + 0x500), 0b101);
    harness.write(PPI + 0x004, 1);
    assert_eq!(harness.read(PPI + 0x500), 0);
}

#[test]
fn ppi_fixed_channels_read_their_endpoints() {
    let mut harness = Harness::new();
    // Channel 20 runs RADIO TXEN on TIMER0 COMPARE[0]; its endpoints sit past the programmable ones.
    harness.write(PPI + 0x504, 1 << 31);
    assert_eq!(harness.read(PPI + 0x500), 1 << 31);
}

#[test]
fn a_ppi_loop_is_cut_off() {
    let mut harness = Harness::new();
    harness.write(PPI + 0x510, EGU0 + 0x100);
    harness.write(PPI + 0x514, EGU0);
    harness.write(PPI + 0x504, 1);
    harness.write(EGU0, 1);
    assert_eq!(harness.chip.cut_off_chain_count(), 1);
}

// ============================================================================
// UART and UARTE
// ============================================================================

#[test]
fn uart_sends_and_receives_bytes() {
    let mut harness = Harness::new();
    harness.write(UART + 0x500, 4);
    harness.write(UART + 0x008, 1);
    harness.write(UART + 0x51C, b'h' as u32);
    harness.write(UART + 0x51C, b'i' as u32);
    assert_eq!(harness.read(UART + 0x11C), 1);
    assert_eq!(harness.chip.take_uart_output(), b"hi");
    harness.write(UART + 0x304, 0x4);
    harness.write(UART, 1);
    harness.with_ports(|chip, ports| chip.uart_receive(b"ok", ports));
    assert!(harness.interrupt_line(2));
    assert_eq!(harness.read(UART + 0x518), b'o' as u32);
    assert_eq!(harness.read(UART + 0x518), b'k' as u32);
}

#[test]
fn uarte_moves_buffers_through_easydma() {
    let mut harness = Harness::new();
    harness.write(0x2000_0000, u32::from_le_bytes(*b"ping"));
    harness.write(UART + 0x500, 8);
    harness.write(UART + 0x544, 0x2000_0000);
    harness.write(UART + 0x548, 4);
    harness.write(UART + 0x008, 1);
    assert_eq!(harness.read(UART + 0x120), 1);
    assert_eq!(harness.read(UART + 0x54C), 4);
    assert_eq!(harness.chip.take_uart_output(), b"ping");
    harness.write(UART + 0x534, 0x2000_0100);
    harness.write(UART + 0x538, 3);
    harness.write(UART, 1);
    harness.with_ports(|chip, ports| chip.uart_receive(b"po", ports));
    assert_eq!(harness.read(UART + 0x110), 0);
    harness.with_ports(|chip, ports| chip.uart_receive(b"ng", ports));
    assert_eq!(harness.read(UART + 0x110), 1);
    assert_eq!(harness.read(UART + 0x53C), 3);
    assert_eq!(
        harness.read(0x2000_0100) & 0xFF_FFFF,
        u32::from_le_bytes(*b"pon\0")
    );
    // The fourth byte waits for the next buffer.
    harness.write(UART, 1);
    assert_eq!(harness.read(0x2000_0100) & 0xFF, b'g' as u32);
}

// ============================================================================
// EGU, RNG and ECB
// ============================================================================

#[test]
fn egu_trigger_raises_its_event_and_interrupt() {
    let mut harness = Harness::new();
    harness.write(EGU1 + 0x304, 1 << 15);
    harness.write(EGU1 + 0x03C, 1);
    assert_eq!(harness.read(EGU1 + 0x13C), 1);
    assert!(harness.interrupt_line(21));
}

#[test]
fn rng_produces_values_while_started() {
    let mut harness = Harness::new();
    harness.write(RNG + 0x304, 1);
    harness.write(RNG, 1);
    assert_eq!(harness.read(RNG + 0x100), 0);
    harness.run_for_microseconds(30.0);
    assert_eq!(harness.read(RNG + 0x100), 1);
    assert!(harness.interrupt_line(13));
    let mut values = Vec::new();
    for _ in 0..8 {
        values.push(harness.read(RNG + 0x508));
        harness.run_for_microseconds(30.0);
    }
    assert!(values.iter().any(|&value| value != values[0]));
    assert!(values.iter().all(|&value| value <= 0xFF));
    harness.write(RNG + 0x004, 1);
    assert_eq!(harness.next_deadline(), None);
}

#[test]
fn rng_valrdy_stop_shortcut_gives_one_value() {
    let mut harness = Harness::new();
    harness.write(RNG + 0x200, 1);
    harness.write(RNG, 1);
    harness.run_for_microseconds(100.0);
    assert_eq!(harness.read(RNG + 0x100), 1);
    assert_eq!(harness.next_deadline(), None);
}

#[test]
fn ecb_encrypts_the_fips_197_example() {
    let mut harness = Harness::new();
    harness.write_words(0x2000_0000, &[0x0302_0100, 0x0706_0504, 0x0B0A_0908, 0x0F0E_0D0C]);
    harness.write_words(0x2000_0010, &[0x3322_1100, 0x7766_5544, 0xBBAA_9988, 0xFFEE_DDCC]);
    harness.write(ECB + 0x504, 0x2000_0000);
    harness.write(ECB + 0x304, 1);
    harness.write(ECB, 1);
    assert_eq!(harness.read(ECB + 0x100), 1);
    assert!(harness.interrupt_line(14));
    assert_eq!(harness.read(0x2000_0020), 0xD8E0_C469);
    assert_eq!(harness.read(0x2000_002C), 0x5AC5_B470);
}
