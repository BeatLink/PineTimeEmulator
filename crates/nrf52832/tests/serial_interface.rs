// SPDX-License-Identifier: GPL-3.0-only
//! The shared SPI/SPIM/TWI/TWIM blocks, ported from tests/serial_interface.robot and the chip side
//! of tests/i2c_sensors.robot.

mod common;

use common::Harness;

const SERIAL0: u32 = 0x4000_3000;
const SERIAL1: u32 = 0x4000_4000;
const SERIAL2: u32 = 0x4002_3000;
const SERIAL0_IRQ: u8 = 3;
const SERIAL1_IRQ: u8 = 4;
const DEVICE: u8 = 0x15;

fn harness_with_device() -> Harness {
    let mut harness = Harness::new();
    harness.board.add_i2c_device(DEVICE);
    harness
}

fn set_device_register(harness: &mut Harness, register: u8, value: u8) {
    harness.board.i2c_devices.get_mut(&DEVICE).unwrap().registers[register as usize] = value;
}

fn device_register(harness: &Harness, register: u8) -> u8 {
    harness.board.i2c_devices[&DEVICE].registers[register as usize]
}

/// Lets a transfer finish crossing the bus.
fn let_the_bus_run(harness: &mut Harness) {
    harness.run_for_seconds(0.001);
}

#[test]
fn legacy_spi_exchanges_bytes_through_the_double_buffer() {
    let mut harness = Harness::new();
    harness.write(SERIAL0 + 0x500, 1);
    harness.write(SERIAL0 + 0x304, 0x4);
    assert_eq!(harness.read(SERIAL0 + 0x108), 0);
    harness.write(SERIAL0 + 0x51C, 0xA5);
    harness.write(SERIAL0 + 0x51C, 0x3C);
    // A third byte waits until the receive buffer has room.
    harness.write(SERIAL0 + 0x51C, 0x01);
    assert_eq!(harness.board.spi_bytes(0), "A5 3C");
    assert_eq!(harness.read(SERIAL0 + 0x108), 1);
    assert!(harness.interrupt_line(SERIAL0_IRQ));
    harness.write(SERIAL0 + 0x108, 0);
    assert_eq!(harness.read(SERIAL0 + 0x518), 0x5A);
    assert_eq!(harness.read(SERIAL0 + 0x108), 1);
    assert_eq!(harness.board.spi_bytes(0), "A5 3C 01");
    harness.write(SERIAL0 + 0x108, 0);
    assert_eq!(harness.read(SERIAL0 + 0x518), 0xC3);
    assert_eq!(harness.read(SERIAL0 + 0x108), 1);
    assert_eq!(harness.read(SERIAL0 + 0x518), 0xFE);
}

#[test]
fn spim_runs_a_full_duplex_dma_transfer_padded_with_orc() {
    let mut harness = Harness::new();
    harness.write(0x2000_0000, 0x0033_2211);
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x304, 0x40);
    harness.write(SERIAL0 + 0x544, 0x2000_0000);
    harness.write(SERIAL0 + 0x548, 2);
    harness.write(SERIAL0 + 0x534, 0x2000_0100);
    harness.write(SERIAL0 + 0x538, 4);
    harness.write(SERIAL0 + 0x5C0, 0xEE);
    assert_eq!(harness.read(SERIAL0 + 0x118), 0);
    harness.write(SERIAL0 + 0x010, 1);
    assert_eq!(harness.board.spi_bytes(0), "11 22 EE EE");
    assert_eq!(harness.read(SERIAL0 + 0x14C), 1);
    // END comes once four bytes have crossed the bus at 250 kbit/s, 128 us.
    harness.run_for_microseconds(127.0);
    assert_eq!(harness.read(SERIAL0 + 0x118), 0);
    harness.run_for_microseconds(1.0);
    assert_eq!(harness.read(0x2000_0100), 0x1111_DDEE);
    assert_eq!(harness.read(SERIAL0 + 0x54C), 2);
    assert_eq!(harness.read(SERIAL0 + 0x53C), 4);
    assert_eq!(harness.read(SERIAL0 + 0x120), 1);
    assert_eq!(harness.read(SERIAL0 + 0x110), 1);
    assert_eq!(harness.read(SERIAL0 + 0x118), 1);
    assert!(harness.interrupt_line(SERIAL0_IRQ));
    // A longer transmit than receive drops the extra received bytes.
    harness.write(0x2000_0104, 0xAAAA_AAAA);
    harness.write(SERIAL0 + 0x548, 3);
    harness.write(SERIAL0 + 0x534, 0x2000_0104);
    harness.write(SERIAL0 + 0x538, 1);
    harness.write(SERIAL0 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.board.spi_bytes(0), "11 22 EE EE 11 22 33");
    assert_eq!(harness.read(0x2000_0104), 0xAAAA_AAEE);
}

#[test]
fn spim_start_right_after_an_empty_transfer_is_not_lost() {
    // embassy-nrf starts an empty transfer, then the real one at once, to work around an erratum.
    let mut harness = Harness::new();
    harness.write(0x2000_0000, 0x9F);
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x544, 0x2000_0000);
    harness.write(SERIAL0 + 0x548, 0);
    harness.write(SERIAL0 + 0x538, 0);
    harness.write(SERIAL0 + 0x010, 1);
    assert_eq!(harness.read(SERIAL0 + 0x118), 1);
    harness.write(SERIAL0 + 0x118, 0);
    harness.write(SERIAL0 + 0x548, 1);
    harness.write(SERIAL0 + 0x010, 1);
    assert_eq!(harness.board.spi_bytes(0), "9F");
}

#[test]
fn spim_bit_rate_follows_frequency() {
    let mut harness = Harness::new();
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x524, 0x8000_0000);
    harness.write(SERIAL0 + 0x548, 1);
    harness.write(SERIAL0 + 0x010, 1);
    // One byte at 8 Mbit/s takes 1 us.
    harness.run_for_microseconds(0.9);
    assert_eq!(harness.read(SERIAL0 + 0x118), 0);
    harness.run_for_microseconds(0.1);
    assert_eq!(harness.read(SERIAL0 + 0x118), 1);
}

#[test]
fn spim_end_start_shortcut_runs_back_to_back_transfers() {
    let mut harness = Harness::new();
    harness.write(0x2000_0000, 0x0000_00AB);
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x524, 0x8000_0000);
    harness.write(SERIAL0 + 0x544, 0x2000_0000);
    harness.write(SERIAL0 + 0x548, 1);
    harness.write(SERIAL0 + 0x200, 1 << 17);
    harness.write(SERIAL0 + 0x010, 1);
    harness.run_for_microseconds(2.5);
    assert_eq!(harness.board.spi_bytes(0), "AB AB AB");
    harness.write(SERIAL0 + 0x014, 1);
    assert_eq!(harness.read(SERIAL0 + 0x104), 1);
    harness.run_for_microseconds(5.0);
    assert_eq!(harness.board.spi_bytes(0), "AB AB AB");
}

#[test]
fn spim_array_list_moves_to_the_next_buffer_after_each_transfer() {
    let mut harness = Harness::new();
    harness.write(0x2000_0000, 0x0000_2211);
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x544, 0x2000_0000);
    harness.write(SERIAL0 + 0x548, 1);
    harness.write(SERIAL0 + 0x550, 1);
    harness.write(SERIAL0 + 0x538, 0);
    harness.write(SERIAL0 + 0x010, 1);
    let_the_bus_run(&mut harness);
    harness.write(SERIAL0 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.board.spi_bytes(0), "11 22");
    assert_eq!(harness.read(SERIAL0 + 0x544), 0x2000_0000);
}

#[test]
fn spim2_reaches_its_own_bus_instance() {
    let mut harness = Harness::new();
    harness.write(0x2000_0000, 0x0000_0042);
    harness.write(SERIAL2 + 0x500, 7);
    harness.write(SERIAL2 + 0x544, 0x2000_0000);
    harness.write(SERIAL2 + 0x548, 1);
    harness.write(SERIAL2 + 0x304, 0x40);
    harness.write(SERIAL2 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.board.spi_bytes(2), "42");
    assert!(harness.interrupt_line(35));
}

#[test]
fn twim_writes_to_a_device() {
    let mut harness = harness_with_device();
    harness.write(0x2000_0000, 0x00CD_AB10);
    harness.write(SERIAL1 + 0x500, 6);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 3);
    harness.write(SERIAL1 + 0x200, 0x200);
    harness.write(SERIAL1 + 0x304, 0x2);
    harness.write(SERIAL1 + 0x008, 1);
    assert_eq!(harness.read(SERIAL1 + 0x150), 1);
    assert_eq!(harness.read(SERIAL1 + 0x160), 0);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.board.i2c_log(), "W 10 AB CD | F");
    assert_eq!(device_register(&harness, 0x11), 0xCD);
    assert_eq!(harness.read(SERIAL1 + 0x54C), 3);
    assert_eq!(harness.read(SERIAL1 + 0x160), 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
    assert_eq!(harness.read(SERIAL1 + 0x124), 0);
    assert!(harness.interrupt_line(SERIAL1_IRQ));
}

#[test]
fn twim_write_then_read_uses_a_repeated_start() {
    let mut harness = harness_with_device();
    set_device_register(&mut harness, 0x20, 0x01);
    set_device_register(&mut harness, 0x21, 0x02);
    set_device_register(&mut harness, 0x22, 0x03);
    harness.write(0x2000_0000, 0x20);
    harness.write(0x2000_0200, 0xFFFF_FFFF);
    harness.write(SERIAL1 + 0x500, 6);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 1);
    harness.write(SERIAL1 + 0x534, 0x2000_0200);
    harness.write(SERIAL1 + 0x538, 3);
    harness.write(SERIAL1 + 0x200, 0x1080);
    harness.write(SERIAL1 + 0x008, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.board.i2c_log(), "W 20 | R 3 | F");
    assert_eq!(harness.read(0x2000_0200), 0xFF03_0201);
    assert_eq!(harness.read(SERIAL1 + 0x53C), 3);
    assert_eq!(harness.read(SERIAL1 + 0x160), 1);
    assert_eq!(harness.read(SERIAL1 + 0x14C), 1);
    assert_eq!(harness.read(SERIAL1 + 0x15C), 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
}

#[test]
fn twim_reports_a_nack_from_a_missing_address() {
    let mut harness = harness_with_device();
    harness.write(SERIAL1 + 0x500, 6);
    harness.write(SERIAL1 + 0x588, 0x33);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 1);
    harness.write(SERIAL1 + 0x200, 0x200);
    harness.write(SERIAL1 + 0x304, 0x200);
    harness.write(SERIAL1 + 0x008, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(SERIAL1 + 0x124), 1);
    assert_eq!(harness.read(SERIAL1 + 0x4C4), 0x2);
    assert_eq!(harness.read(SERIAL1 + 0x160), 0);
    assert_eq!(harness.read(SERIAL1 + 0x104), 0);
    assert!(harness.interrupt_line(SERIAL1_IRQ));
    harness.write(SERIAL1 + 0x014, 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
    harness.write(SERIAL1 + 0x4C4, 0x2);
    assert_eq!(harness.read(SERIAL1 + 0x4C4), 0);
    assert_eq!(harness.board.i2c_log(), "");
}

#[test]
fn twim_stop_during_a_transfer_waits_for_its_end() {
    let mut harness = harness_with_device();
    harness.write(0x2000_0000, 0x0000_0001);
    harness.write(SERIAL1 + 0x500, 6);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 1);
    harness.write(SERIAL1 + 0x008, 1);
    harness.write(SERIAL1 + 0x014, 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 0);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(SERIAL1 + 0x160), 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
    assert_eq!(harness.board.i2c_log(), "W 01 | F");
}

// The board's byte-wise bus reports the three single-byte reads of one segment as "R 3".
#[test]
fn legacy_twi_reads_several_bytes() {
    let mut harness = harness_with_device();
    set_device_register(&mut harness, 0x30, 0x41);
    set_device_register(&mut harness, 0x31, 0x42);
    set_device_register(&mut harness, 0x32, 0x43);
    harness.write(SERIAL1 + 0x500, 5);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x200, 0x1);
    harness.write(SERIAL1 + 0x51C, 0x30);
    harness.write(SERIAL1 + 0x008, 1);
    assert_eq!(harness.read(SERIAL1 + 0x11C), 1);
    assert_eq!(harness.read(SERIAL1 + 0x148), 1);
    harness.write(SERIAL1 + 0x148, 0);
    harness.write(SERIAL1, 1);
    assert_eq!(harness.read(SERIAL1 + 0x108), 1);
    assert_eq!(harness.read(SERIAL1 + 0x148), 1);
    harness.write(SERIAL1 + 0x108, 0);
    harness.write(SERIAL1 + 0x148, 0);
    assert_eq!(harness.read(SERIAL1 + 0x518), 0x41);
    assert_eq!(harness.read(SERIAL1 + 0x108), 0);
    harness.write(SERIAL1 + 0x020, 1);
    assert_eq!(harness.read(SERIAL1 + 0x108), 1);
    harness.write(SERIAL1 + 0x108, 0);
    assert_eq!(harness.read(SERIAL1 + 0x518), 0x42);
    // The last byte stops the bus at its byte boundary.
    harness.write(SERIAL1 + 0x200, 0x2);
    harness.write(SERIAL1 + 0x020, 1);
    assert_eq!(harness.read(SERIAL1 + 0x108), 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
    assert_eq!(harness.read(SERIAL1 + 0x518), 0x43);
    assert_eq!(harness.board.i2c_log(), "W 30 | R 3 | F");
    assert_eq!(harness.read(SERIAL1 + 0x4C4), 0);
}

#[test]
fn legacy_twi_reports_a_nack_from_a_missing_address() {
    let mut harness = harness_with_device();
    harness.write(SERIAL1 + 0x500, 5);
    harness.write(SERIAL1 + 0x588, 0x50);
    harness.write(SERIAL1, 1);
    assert_eq!(harness.read(SERIAL1 + 0x124), 1);
    assert_eq!(harness.read(SERIAL1 + 0x4C4), 0x2);
    assert_eq!(harness.read(SERIAL1 + 0x108), 0);
    harness.write(SERIAL1 + 0x014, 1);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
}

#[test]
fn enable_switches_between_the_four_modes() {
    let mut harness = harness_with_device();
    harness.write(SERIAL1 + 0x500, 1);
    assert_eq!(harness.read(SERIAL1 + 0x500), 1);
    harness.write(SERIAL1 + 0x51C, 0x42);
    assert_eq!(harness.read(SERIAL1 + 0x518), 0xBD);
    assert_eq!(harness.board.spi_bytes(1), "42");
    // SPIM tasks do nothing in legacy SPI mode.
    harness.write(0x2000_0000, 0x77);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 1);
    harness.write(SERIAL1 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(SERIAL1 + 0x118), 0);
    harness.write(SERIAL1 + 0x500, 7);
    assert_eq!(harness.read(SERIAL1 + 0x500), 7);
    harness.write(SERIAL1 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(SERIAL1 + 0x118), 1);
    assert_eq!(harness.board.spi_bytes(1), "42 77");
    harness.write(SERIAL1 + 0x500, 6);
    assert_eq!(harness.read(SERIAL1 + 0x500), 6);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x200, 0x200);
    harness.write(SERIAL1 + 0x008, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(SERIAL1 + 0x104), 1);
    assert_eq!(harness.board.i2c_log(), "W 77 | F");
    harness.write(SERIAL1 + 0x500, 5);
    assert_eq!(harness.read(SERIAL1 + 0x500), 5);
    harness.write(SERIAL1 + 0x200, 0x2);
    harness.write(SERIAL1 + 0x51C, 0x05);
    harness.write(SERIAL1 + 0x008, 1);
    assert_eq!(harness.board.i2c_log(), "W 77 | F | W 05 | F");
    assert_eq!(harness.board.spi_bytes(1), "42 77");
    harness.write(SERIAL1 + 0x500, 3);
    assert_eq!(harness.read(SERIAL1 + 0x500), 0);
    harness.write(SERIAL1 + 0x500, 0);
    assert_eq!(harness.read(SERIAL1 + 0x500), 0);
}

#[test]
fn easydma_never_writes_flash() {
    let mut harness = harness_with_device();
    harness.write(SERIAL0 + 0x500, 7);
    harness.write(SERIAL0 + 0x534, 0x0000_1000);
    harness.write(SERIAL0 + 0x538, 4);
    harness.write(SERIAL0 + 0x010, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(0x1000), 0xFFFF_FFFF);
    assert_eq!(harness.read(SERIAL0 + 0x118), 1);
}

// The chip side of tests/i2c_sensors.robot: register writes and reads through TWIM, as the sensor drivers do.
#[test]
fn twim_reads_back_a_register_it_wrote() {
    let mut harness = harness_with_device();
    harness.write(0x2000_0000, 0x0000_B67E);
    harness.write(SERIAL1 + 0x500, 6);
    harness.write(SERIAL1 + 0x588, 0x15);
    harness.write(SERIAL1 + 0x544, 0x2000_0000);
    harness.write(SERIAL1 + 0x548, 2);
    harness.write(SERIAL1 + 0x200, 0x200);
    harness.write(SERIAL1 + 0x008, 1);
    let_the_bus_run(&mut harness);
    harness.write(SERIAL1 + 0x548, 1);
    harness.write(SERIAL1 + 0x534, 0x2000_0100);
    harness.write(SERIAL1 + 0x538, 1);
    harness.write(SERIAL1 + 0x200, 0x1080);
    harness.write(SERIAL1 + 0x008, 1);
    let_the_bus_run(&mut harness);
    assert_eq!(harness.read(0x2000_0100) & 0xFF, 0xB6);
    assert_eq!(harness.board.i2c_log(), "W 7E B6 | F | W 7E | R 1 | F");
}
