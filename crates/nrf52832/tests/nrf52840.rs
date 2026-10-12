// SPDX-License-Identifier: GPL-3.0-only
//! The nRF52840 variant the Bluetooth bridge runs: its memory map, FICR, UARTE1, GPIO port P1 and
//! the radio's extra events, next to the nRF52832 left as it was.

mod common;

use common::{microseconds, Harness};
use pinetime_core::{AccessSize, BusError};
use pinetime_nrf52832::{SerialPort, Variant};

const FICR: u32 = 0x1000_0000;
const UARTE1: u32 = 0x4002_8000;
const UARTE1_INTERRUPT: u8 = 40;
const GPIO_PORT0_OUT: u32 = 0x5000_0504;
const GPIO_PORT1_OUT: u32 = 0x5000_0804;
const GPIO_PORT1_DIR: u32 = 0x5000_0814;
const RADIO: u32 = 0x4000_1000;

fn nrf52840() -> Harness {
    Harness::new_variant(Variant::Nrf52840)
}

fn try_read(harness: &mut Harness, address: u32) -> Result<u32, BusError> {
    harness.with_ports(|chip, ports| chip.read(address, AccessSize::Word, ports))
}

// ============================================================================
// Memory map and FICR
// ============================================================================

#[test]
fn the_nrf52840_has_a_megabyte_of_flash_and_256_kilobytes_of_ram() {
    let mut harness = nrf52840();
    assert_eq!(harness.chip.flash().len(), 0x10_0000);
    assert_eq!(harness.chip.ram().len(), 0x4_0000);
    assert_eq!(try_read(&mut harness, 0x000F_FFFC), Ok(0xFFFF_FFFF));
    assert_eq!(try_read(&mut harness, 0x0010_0000), Err(BusError::Unmapped));

    harness.write(0x2003_FFFC, 0x1234_5678);
    assert_eq!(harness.read(0x2003_FFFC), 0x1234_5678);
    assert_eq!(try_read(&mut harness, 0x0083_FFFC), Ok(0x1234_5678));
    assert_eq!(try_read(&mut harness, 0x2004_0000), Err(BusError::Unmapped));
}

#[test]
fn the_nrf52832_keeps_its_own_sizes() {
    let mut harness = Harness::new();
    assert_eq!(harness.chip.variant(), Variant::Nrf52832);
    assert_eq!(try_read(&mut harness, 0x0008_0000), Err(BusError::Unmapped));
    assert_eq!(try_read(&mut harness, 0x2001_0000), Err(BusError::Unmapped));
    assert_eq!(harness.read(FICR + 0x100), 0x52832);
}

#[test]
fn the_ficr_describes_an_nrf52840_with_its_own_address() {
    let mut harness = nrf52840();
    assert_eq!(harness.read(FICR + 0x010), 0x1000);
    assert_eq!(harness.read(FICR + 0x014), 0x100);
    assert_eq!(harness.read(FICR + 0x100), 0x52840);
    assert_eq!(harness.read(FICR + 0x10C), 256);
    assert_eq!(harness.read(FICR + 0x110), 1024);
    assert_eq!(harness.read(FICR + 0x0A0), 1);
    assert_eq!(harness.read(FICR + 0x0A4), 0xAABB_CCDD);
    assert_ne!(harness.read(FICR + 0x0A8), Harness::new().read(FICR + 0x0A8));
    assert_eq!(harness.read(0xF000_0FE0), 0x08);
}

#[test]
fn the_nrf52840_has_no_pinetime_board_pull_ups() {
    let harness = nrf52840();
    assert_eq!(harness.chip.pin_input_levels(), 0);
}

// ============================================================================
// UARTE1
// ============================================================================

#[test]
fn uarte1_sends_a_buffer_and_raises_its_own_interrupt() {
    let mut harness = nrf52840();
    harness.write(0x2003_0000, u32::from_le_bytes(*b"\x04\x0e\x04\x01"));
    harness.write(UARTE1 + 0x500, 8);
    harness.write(UARTE1 + 0x304, 1 << 8);
    harness.write(UARTE1 + 0x544, 0x2003_0000);
    harness.write(UARTE1 + 0x548, 4);
    harness.write(UARTE1 + 0x008, 1);

    assert_eq!(harness.read(UARTE1 + 0x120), 1);
    assert!(harness.interrupt_line(UARTE1_INTERRUPT));
    assert_eq!(
        harness.chip.take_serial_port_output(SerialPort::Uarte1),
        b"\x04\x0e\x04\x01"
    );
    assert!(harness.chip.take_uart_output().is_empty());
}

#[test]
fn uarte1_holds_received_bytes_until_a_buffer_is_armed() {
    let mut harness = nrf52840();
    harness.write(UARTE1 + 0x500, 8);
    harness
        .with_ports(|chip, ports| chip.serial_port_receive(SerialPort::Uarte1, b"\x01\x03\x0c\x00", ports));
    assert_eq!(harness.chip.serial_port_waiting_bytes(SerialPort::Uarte1), 4);

    // One byte at a time, as Zephyr's interrupt-driven UARTE driver receives.
    let mut received = Vec::new();
    for _ in 0..4 {
        harness.write(UARTE1 + 0x534, 0x2003_0100);
        harness.write(UARTE1 + 0x538, 1);
        harness.write(UARTE1 + 0x110, 0);
        harness.write(UARTE1, 1);
        assert_eq!(harness.read(UARTE1 + 0x110), 1);
        received.push(harness.read(0x2003_0100) as u8);
    }
    assert_eq!(received, b"\x01\x03\x0c\x00");
    assert_eq!(harness.chip.serial_port_waiting_bytes(SerialPort::Uarte1), 0);
}

#[test]
fn uarte1_counts_buffers_longer_than_255_bytes() {
    let mut harness = nrf52840();
    harness.write(UARTE1 + 0x548, 0x1234);
    assert_eq!(harness.read(UARTE1 + 0x548), 0x1234);
    harness.write(0x4000_2548, 0x1234);
    assert_eq!(harness.read(0x4000_2548), 0x1234);
    assert_eq!(Harness::new().read(0x4000_2548), 0);
}

#[test]
fn the_nrf52832_has_no_uarte1() {
    let mut harness = Harness::new();
    harness.write(UARTE1 + 0x500, 8);
    harness.write(UARTE1 + 0x008, 1);
    assert!(harness
        .chip
        .take_serial_port_output(SerialPort::Uarte1)
        .is_empty());
    assert!(harness.chip.unhandled_access_count() > 0);
}

// ============================================================================
// GPIO port P1
// ============================================================================

#[test]
fn port_p1_keeps_its_own_registers() {
    let mut harness = nrf52840();
    harness.write(GPIO_PORT1_DIR, 0x0000_0300);
    harness.write(GPIO_PORT1_OUT, 0x0000_0100);
    assert_eq!(harness.read(GPIO_PORT1_OUT), 0x0000_0100);
    assert_eq!(harness.read(GPIO_PORT0_OUT), 0);
    harness.write(GPIO_PORT0_OUT, 0x8000_0000);
    assert_eq!(harness.read(GPIO_PORT1_OUT), 0x0000_0100);
    assert_eq!(harness.chip.unhandled_access_count(), 0);
}

// ============================================================================
// Radio
// ============================================================================

// The bridge's radio adds TXREADY and PHYEND; TXREADY_START and PHYEND_DISABLE stand in for READY_START and END_DISABLE.
#[test]
fn the_radio_adds_the_nrf52840_ready_and_phyend_events() {
    let mut harness = nrf52840();
    for (offset, value) in [
        (0x510, 3),
        (0x514, 0x0010_0108),
        (0x518, 0x0203_00FF),
        (0x51C, 0x89BE_D600),
        (0x524, 0x8E),
        (0x534, 0x103),
        (0x508, 2),
        (0x650, 0x201),
    ] {
        harness.write(RADIO + offset, value);
    }
    harness.write_words(0x2000_1000, &[0x3D00_0940, 0xDE0A_1B2C, 0x0601_02C0]);
    harness.write(RADIO + 0x504, 0x2000_1000);
    harness.chip.take_event_trace();
    harness.write(RADIO + 0x200, 0x14_0000);
    harness.write(RADIO, 1);
    harness.run_for(microseconds(1000.0));

    let events: Vec<(u64, u32)> = harness
        .chip
        .take_event_trace()
        .into_iter()
        .filter(|(_, address)| address & !0xFFF == RADIO)
        .map(|(cycle, address)| (cycle / 64, address & 0xFFF))
        .collect();
    assert_eq!(
        events,
        [
            (40, 0x100),
            (40, 0x154),
            (80, 0x104),
            (168, 0x108),
            (192, 0x10C),
            (192, 0x16C),
            (198, 0x110)
        ]
    );
}
