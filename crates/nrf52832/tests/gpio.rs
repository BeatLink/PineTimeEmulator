// SPDX-License-Identifier: GPL-3.0-only
//! GPIO and GPIOTE, ported from tests/gpio.robot.

mod common;

use common::Harness;
use pinetime_nrf52832::ResetKind;

const GPIO: u32 = 0x5000_0000;
const GPIOTE: u32 = 0x4000_6000;
const PPI: u32 = 0x4001_F000;
const OUT: u32 = GPIO + 0x504;
const OUTSET: u32 = GPIO + 0x508;
const OUTCLR: u32 = GPIO + 0x50C;
const IN: u32 = GPIO + 0x510;
const DIR: u32 = GPIO + 0x514;
const DIRSET: u32 = GPIO + 0x518;
const DIRCLR: u32 = GPIO + 0x51C;
const LATCH: u32 = GPIO + 0x520;
const DETECTMODE: u32 = GPIO + 0x524;
const GPIOTE_INTENSET: u32 = GPIOTE + 0x304;
const GPIOTE_INTENCLR: u32 = GPIOTE + 0x308;
const EVENTS_PORT: u32 = GPIOTE + 0x17C;
const GPIOTE_IRQ: u8 = 6;
const DRIVER_PIN: u8 = 10;

fn configure_pin(harness: &mut Harness, pin: u32, value: u32) {
    harness.write(GPIO + 0x700 + 4 * pin, value);
}

fn configure_channel(harness: &mut Harness, channel: u32, value: u32) {
    harness.write(GPIOTE + 0x510 + 4 * channel, value);
}

fn trigger_task(harness: &mut Harness, base_offset: u32, channel: u32) {
    harness.write(GPIOTE + base_offset + 4 * channel, 1);
}

fn event_in(harness: &mut Harness, channel: u32) -> u32 {
    harness.read(GPIOTE + 0x100 + 4 * channel)
}

fn in_bit(harness: &mut Harness, pin: u32) -> u32 {
    harness.read(IN) >> pin & 1
}

#[test]
fn output_drives_a_connected_receiver() {
    let mut harness = Harness::new();
    assert!(!harness.board.sees_high(1));
    harness.write(DIRSET, 0x2);
    harness.write(OUTSET, 0x2);
    assert!(harness.board.sees_high(1));
    assert!(harness.pin_level(1));
    assert_eq!(harness.read(OUT), 0x2);
    assert_eq!(harness.read(DIR), 0x2);
    assert_eq!(harness.board.last_pins().1 & 0x2, 0x2);
    harness.write(OUTCLR, 0x2);
    assert!(!harness.board.sees_high(1));
    // OUT written while the pin is an input only shows once it becomes an output.
    harness.write(DIRCLR, 0x2);
    harness.write(OUT, 0x2);
    assert!(!harness.board.sees_high(1));
    configure_pin(&mut harness, 1, 0x3);
    assert!(harness.board.sees_high(1));
}

#[test]
fn switching_to_input_releases_the_line_to_the_board_pull_up() {
    let mut harness = Harness::new();
    assert!(harness.board.sees_high(5));
    assert!(harness.board.sees_high(25));
    harness.write(DIRSET, 0x0200_0020);
    harness.write(OUTCLR, 0x0200_0020);
    assert!(!harness.board.sees_high(5));
    assert!(!harness.board.sees_high(25));
    harness.write(DIRCLR, 0x0200_0020);
    assert!(harness.board.sees_high(5));
    assert!(harness.board.sees_high(25));
    harness.set_board_pull(25, None);
    assert!(!harness.board.sees_high(25));
}

#[test]
fn internal_pulls_set_the_level_of_an_undriven_input() {
    let mut harness = Harness::new();
    configure_pin(&mut harness, 2, 0xC);
    assert!(harness.board.sees_high(2));
    assert_eq!(in_bit(&mut harness, 2), 1);
    configure_pin(&mut harness, 2, 0x4);
    assert!(!harness.board.sees_high(2));
    assert_eq!(in_bit(&mut harness, 2), 0);
    // An internal pull-down beats the weaker board pull-up.
    configure_pin(&mut harness, 25, 0x4);
    assert!(!harness.board.sees_high(25));
    configure_pin(&mut harness, 25, 0x0);
    assert!(harness.board.sees_high(25));
}

#[test]
fn external_drive_reads_through_in() {
    let mut harness = Harness::new();
    configure_pin(&mut harness, 10, 0x0);
    assert_eq!(in_bit(&mut harness, 10), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(in_bit(&mut harness, 10), 1);
    harness.drive(DRIVER_PIN, Some(false));
    assert_eq!(in_bit(&mut harness, 10), 0);
    configure_pin(&mut harness, 3, 0xC);
    harness.drive(3, Some(false));
    assert_eq!(in_bit(&mut harness, 3), 0);
    harness.drive(3, None);
    assert_eq!(in_bit(&mut harness, 3), 1);
    // The chip's own output beats an outside driver.
    configure_pin(&mut harness, 3, 0x1);
    harness.write(OUTSET, 0x8);
    harness.drive(3, Some(false));
    assert_eq!(in_bit(&mut harness, 3), 1);
}

#[test]
fn disconnected_input_buffer_reads_zero() {
    let mut harness = Harness::new();
    harness.drive(DRIVER_PIN, Some(true));
    configure_pin(&mut harness, 10, 0x2);
    assert!(harness.pin_level(10));
    assert_eq!(in_bit(&mut harness, 10), 0);
    configure_pin(&mut harness, 10, 0x0);
    assert_eq!(in_bit(&mut harness, 10), 1);
}

#[test]
fn open_drain_pins_release_the_line_instead_of_driving_high() {
    let mut harness = Harness::new();
    harness.set_board_pull(3, Some(true));
    // S0D1: drives low, lets go for a one.
    configure_pin(&mut harness, 3, 0x601);
    assert!(!harness.board.sees_high(3));
    harness.write(OUTSET, 0x8);
    assert!(harness.board.sees_high(3));
    harness.drive(3, Some(false));
    assert!(!harness.board.sees_high(3));
    assert_eq!(in_bit(&mut harness, 3), 0);
    harness.drive(3, None);
    assert!(harness.board.sees_high(3));
    // H0D1 behaves the same way.
    configure_pin(&mut harness, 3, 0x701);
    assert!(harness.board.sees_high(3));
    harness.write(OUTCLR, 0x8);
    assert!(!harness.board.sees_high(3));
    // D0S1: lets go for a zero, so the pull decides.
    harness.set_board_pull(3, None);
    configure_pin(&mut harness, 3, 0x40D);
    assert!(harness.board.sees_high(3));
    configure_pin(&mut harness, 3, 0x405);
    assert!(!harness.board.sees_high(3));
    harness.write(OUTSET, 0x8);
    assert!(harness.board.sees_high(3));
}

#[test]
fn sense_sets_latch_and_default_detect_follows_the_pins() {
    let mut harness = Harness::new();
    configure_pin(&mut harness, 10, 0x20000);
    assert_eq!(harness.read(LATCH), 0);
    assert_eq!(harness.chip.detect_rise_count(), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.read(LATCH), 0x400);
    assert_eq!(harness.chip.detect_rise_count(), 1);
    harness.drive(DRIVER_PIN, Some(false));
    assert_eq!(harness.read(LATCH), 0x400);
    harness.write(LATCH, 0x400);
    assert_eq!(harness.read(LATCH), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.chip.detect_rise_count(), 2);
    // A latch bit cleared while its condition still holds sets again at once.
    harness.write(LATCH, 0x400);
    assert_eq!(harness.read(LATCH), 0x400);
    assert_eq!(harness.chip.detect_rise_count(), 2);
    // Sensing looks through the input buffer.
    harness.drive(DRIVER_PIN, Some(false));
    harness.write(LATCH, 0x400);
    configure_pin(&mut harness, 10, 0x20002);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.read(LATCH), 0);
    assert_eq!(harness.chip.detect_rise_count(), 2);
}

#[test]
fn ldetect_stays_high_while_any_latch_bit_is_set() {
    let mut harness = Harness::new();
    harness.write(DETECTMODE, 1);
    assert_eq!(harness.read(DETECTMODE), 1);
    configure_pin(&mut harness, 10, 0x20000);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.chip.detect_rise_count(), 1);
    harness.drive(DRIVER_PIN, Some(false));
    harness.drive(DRIVER_PIN, Some(true));
    // DETECT never fell because the latch held it, so no new edge.
    assert_eq!(harness.chip.detect_rise_count(), 1);
    harness.drive(DRIVER_PIN, Some(false));
    harness.write(LATCH, 0x400);
    assert_eq!(harness.read(LATCH), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.chip.detect_rise_count(), 2);
    // Clearing a latch whose pin still meets its condition gives a fresh edge.
    harness.write(LATCH, 0x400);
    assert_eq!(harness.read(LATCH), 0x400);
    assert_eq!(harness.chip.detect_rise_count(), 3);
}

#[test]
fn detect_event_fires_on_a_sense_low_pin() {
    let mut harness = Harness::new();
    harness.set_board_pull(7, Some(true));
    configure_pin(&mut harness, 7, 0x30000);
    assert_eq!(harness.chip.detect_rise_count(), 0);
    harness.drive(7, Some(false));
    assert_eq!(harness.chip.detect_rise_count(), 1);
    assert_eq!(harness.read(LATCH), 0x80);
    harness.drive(7, None);
    harness.write(LATCH, 0x80);
    harness.drive(7, Some(false));
    assert_eq!(harness.chip.detect_rise_count(), 2);
}

#[test]
fn gpiote_event_mode_raises_in_on_a_rising_edge() {
    let mut harness = Harness::new();
    configure_channel(&mut harness, 0, 0x10A01);
    harness.write(GPIOTE_INTENSET, 0x1);
    assert_eq!(harness.read(GPIOTE + 0x510), 0x10A01);
    assert_eq!(event_in(&mut harness, 0), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(event_in(&mut harness, 0), 1);
    assert!(harness.interrupt_line(GPIOTE_IRQ));
    harness.write(GPIOTE + 0x100, 0);
    assert!(!harness.interrupt_line(GPIOTE_IRQ));
    harness.drive(DRIVER_PIN, Some(false));
    assert_eq!(event_in(&mut harness, 0), 0);
}

#[test]
fn gpiote_event_mode_raises_in_on_falling_and_toggle() {
    let mut harness = Harness::new();
    harness.drive(DRIVER_PIN, Some(true));
    configure_channel(&mut harness, 2, 0x20A01);
    harness.drive(DRIVER_PIN, Some(false));
    assert_eq!(event_in(&mut harness, 2), 1);
    assert!(!harness.interrupt_line(GPIOTE_IRQ));
    harness.write(GPIOTE_INTENSET, 0x4);
    assert!(harness.interrupt_line(GPIOTE_IRQ));
    harness.write(GPIOTE_INTENCLR, 0x4);
    assert!(!harness.interrupt_line(GPIOTE_IRQ));
    harness.write(GPIOTE + 0x108, 0);
    configure_channel(&mut harness, 2, 0x30A01);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(event_in(&mut harness, 2), 1);
    harness.write(GPIOTE + 0x108, 0);
    harness.drive(DRIVER_PIN, Some(false));
    assert_eq!(event_in(&mut harness, 2), 1);
}

#[test]
fn gpiote_event_mode_makes_an_output_pin_an_input() {
    let mut harness = Harness::new();
    harness.write(DIRSET, 0x20);
    harness.write(OUTCLR, 0x20);
    assert!(!harness.board.sees_high(5));
    // Taking the pin over releases it to the board pull-up without raising an event.
    configure_channel(&mut harness, 1, 0x10501);
    assert!(harness.board.sees_high(5));
    assert_eq!(event_in(&mut harness, 1), 0);
    configure_channel(&mut harness, 1, 0x0);
    assert!(!harness.board.sees_high(5));
}

#[test]
fn gpiote_task_mode_drives_a_pin() {
    let mut harness = Harness::new();
    configure_channel(&mut harness, 1, 0x130103);
    assert!(harness.board.sees_high(1));
    trigger_task(&mut harness, 0x60, 1);
    assert!(!harness.board.sees_high(1));
    trigger_task(&mut harness, 0x30, 1);
    assert!(harness.board.sees_high(1));
    trigger_task(&mut harness, 0x00, 1);
    assert!(!harness.board.sees_high(1));
    trigger_task(&mut harness, 0x00, 1);
    assert!(harness.board.sees_high(1));
    // OUT and DIR have no say while the channel owns the pin.
    harness.write(DIRSET, 0x2);
    harness.write(OUTCLR, 0x2);
    assert!(harness.board.sees_high(1));
    // LoToHi makes OUT a set task.
    configure_channel(&mut harness, 1, 0x010103);
    assert!(!harness.board.sees_high(1));
    trigger_task(&mut harness, 0x00, 1);
    assert!(harness.board.sees_high(1));
    configure_channel(&mut harness, 1, 0x0);
    assert!(!harness.board.sees_high(1));
    // Tasks on a channel that is not in task mode do nothing.
    harness.write(DIRCLR, 0x2);
    trigger_task(&mut harness, 0x30, 1);
    assert!(!harness.board.sees_high(1));
}

#[test]
fn gpiote_port_event_follows_detect() {
    let mut harness = Harness::new();
    configure_pin(&mut harness, 10, 0x20000);
    harness.write(GPIOTE_INTENSET, 0x8000_0000);
    assert_eq!(harness.read(GPIOTE_INTENSET), 0x8000_0000);
    assert_eq!(harness.read(EVENTS_PORT), 0);
    harness.drive(DRIVER_PIN, Some(true));
    assert_eq!(harness.read(EVENTS_PORT), 1);
    assert!(harness.interrupt_line(GPIOTE_IRQ));
    harness.write(EVENTS_PORT, 0);
    assert!(!harness.interrupt_line(GPIOTE_IRQ));
}

// The PPI runs the task at once, without waiting for emulated time to move.
#[test]
fn ppi_links_a_gpiote_event_to_a_gpiote_task() {
    let mut harness = Harness::new();
    configure_channel(&mut harness, 0, 0x10A01);
    configure_channel(&mut harness, 1, 0x000103);
    harness.write(PPI + 0x510, 0x4000_6100);
    harness.write(PPI + 0x514, 0x4000_6034);
    harness.write(PPI + 0x504, 0x1);
    assert!(!harness.board.sees_high(1));
    harness.drive(DRIVER_PIN, Some(true));
    assert!(harness.board.sees_high(1));
}

#[test]
fn reset_returns_every_pin_to_a_disconnected_input() {
    let mut harness = Harness::new();
    harness.write(DIRSET, 0x0200_0022);
    harness.write(OUTCLR, 0x0200_0020);
    harness.write(OUTSET, 0x2);
    configure_pin(&mut harness, 10, 0x2000C);
    configure_channel(&mut harness, 3, 0x130303);
    harness.write(DETECTMODE, 1);
    assert!(!harness.board.sees_high(25));
    assert!(harness.board.sees_high(1));
    let announcements = harness.board.pin_announcements.len();
    harness.with_ports(|chip, ports| chip.reset(ResetKind::Pin, ports));
    // Parts wired to pins hear the levels again after a reset.
    assert!(harness.board.pin_announcements.len() > announcements);
    assert_eq!(harness.read(DIR), 0);
    assert_eq!(harness.read(OUT), 0);
    assert_eq!(harness.read(GPIO + 0x700), 0x2);
    assert_eq!(harness.read(GPIO + 0x728), 0x2);
    assert_eq!(harness.read(DETECTMODE), 0);
    assert_eq!(harness.read(LATCH), 0);
    assert_eq!(harness.read(GPIOTE + 0x51C), 0);
    assert_eq!(harness.read(IN), 0);
    assert!(!harness.board.sees_high(1));
    assert!(!harness.board.sees_high(3));
    assert!(harness.board.sees_high(5));
    assert!(harness.board.sees_high(25));
}

#[test]
fn detect_in_system_off_asks_for_a_wake_reset() {
    let mut harness = Harness::new();
    configure_pin(&mut harness, 13, 0x20000);
    harness.write(0x4000_0500, 1);
    assert!(harness.chip.system_off());
    harness.drive(13, Some(true));
    assert_eq!(
        harness.chip.take_reset_request(),
        Some(ResetKind::WakeFromSystemOff)
    );
    harness.with_ports(|chip, ports| chip.reset(ResetKind::WakeFromSystemOff, ports));
    assert!(!harness.chip.system_off());
    assert_eq!(harness.read(0x4000_0400), 0x1_0000);
}
