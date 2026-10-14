// SPDX-License-Identifier: GPL-3.0-only
//! POWER, CLOCK, the watchdog and the NVMC, ported from tests/power_and_watchdog.robot and tests/nvmc.robot.

mod common;

use common::Harness;
use pinetime_core::AccessSize;
use pinetime_nrf52832::ResetKind;

const GPREGRET: u32 = 0x4000_051C;
const RESETREAS: u32 = 0x4000_0400;
const WDT: u32 = 0x4001_0000;
const NVMC_CONFIG: u32 = 0x4001_E504;
const NVMC_ERASEPAGE: u32 = 0x4001_E508;
const NVMC_ICACHECNF: u32 = 0x4001_E540;
const NVMC_IHIT: u32 = 0x4001_E548;
const NVMC_IMISS: u32 = 0x4001_E54C;
const RELOAD_MAGIC: u32 = 0x6E52_4635;

fn reset(harness: &mut Harness, kind: ResetKind) {
    harness.with_ports(|chip, ports| chip.reset(kind, ports));
}

fn debugger_write(harness: &mut Harness, address: u32, value: u32) {
    harness
        .with_ports(|chip, ports| chip.write_for_debugger(address, value, AccessSize::Word, ports))
        .unwrap();
}

// ============================================================================
// Resets
// ============================================================================

#[test]
fn starts_with_no_reset_reason() {
    let mut harness = Harness::new();
    assert_eq!(harness.read(RESETREAS), 0);
}

#[test]
fn keeps_gpregret_across_a_soft_reset_and_records_it() {
    let mut harness = Harness::new();
    harness.write(GPREGRET, 0xB1);
    reset(&mut harness, ResetKind::SoftReset);
    assert_eq!(harness.read(GPREGRET), 0xB1);
    assert_eq!(harness.read(RESETREAS), 0x4);
}

#[test]
fn clears_a_reset_reason_when_written_with_one() {
    let mut harness = Harness::new();
    reset(&mut harness, ResetKind::SoftReset);
    harness.write(RESETREAS, 0x4);
    assert_eq!(harness.read(RESETREAS), 0);
}

#[test]
fn power_cycle_clears_gpregret_and_ram() {
    let mut harness = Harness::new();
    harness.write(0x2000_FFF0, 0x4E45_4F54);
    harness.write(GPREGRET, 0xB1);
    reset(&mut harness, ResetKind::PowerOn);
    assert_ne!(harness.read(0x2000_FFF0), 0x4E45_4F54);
    assert_eq!(harness.read(GPREGRET), 0);
    assert_eq!(harness.read(RESETREAS), 0);
}

#[test]
fn ram_survives_every_reset_but_a_power_cycle() {
    let mut harness = Harness::new();
    harness.write(0x2000_1000, 0x1234_5678);
    for kind in [
        ResetKind::Pin,
        ResetKind::Watchdog,
        ResetKind::SoftReset,
        ResetKind::Lockup,
        ResetKind::WakeFromSystemOff,
    ] {
        reset(&mut harness, kind);
        assert_eq!(harness.read(0x2000_1000), 0x1234_5678);
    }
    assert_eq!(harness.read(RESETREAS), 0x1_000F);
}

// ============================================================================
// Watchdog
// ============================================================================

fn start_half_second_watchdog(harness: &mut Harness) {
    // Half a second: 16384 ticks of 32768 Hz.
    harness.write(WDT + 0x504, 16383);
    harness.write(WDT, 1);
}

#[test]
fn watchdog_resets_the_chip_when_not_fed() {
    let mut harness = Harness::new();
    start_half_second_watchdog(&mut harness);
    assert_eq!(harness.read(WDT + 0x400), 1);
    harness.run_for_seconds(0.4);
    assert_eq!(harness.read(RESETREAS), 0);
    harness.run_for_seconds(0.2);
    assert_eq!(harness.resets, vec![ResetKind::Watchdog]);
    assert_eq!(harness.read(RESETREAS), 0x2);
    assert_eq!(harness.read(WDT + 0x400), 0);
}

#[test]
fn watchdog_feeding_keeps_the_chip_running() {
    let mut harness = Harness::new();
    start_half_second_watchdog(&mut harness);
    for _ in 0..4 {
        harness.run_for_seconds(0.3);
        harness.write(WDT + 0x600, RELOAD_MAGIC);
    }
    assert_eq!(harness.read(RESETREAS), 0);
    assert_eq!(harness.read(WDT + 0x400), 1);
}

#[test]
fn watchdog_needs_every_enabled_reload_register() {
    let mut harness = Harness::new();
    harness.write(WDT + 0x504, 16383);
    harness.write(WDT + 0x508, 0x3);
    harness.write(WDT, 1);
    harness.run_for_seconds(0.3);
    harness.write(WDT + 0x600, RELOAD_MAGIC);
    harness.run_for_seconds(0.3);
    assert_eq!(harness.read(RESETREAS), 0x2);
}

#[test]
fn watchdog_interrupt_gives_two_ticks_before_the_reset() {
    let mut harness = Harness::new();
    harness.write(WDT + 0x304, 1);
    harness.write(WDT + 0x504, 31);
    harness.write(WDT, 1);
    harness.run_for_seconds(32.0 / 32768.0);
    assert_eq!(harness.read(WDT + 0x100), 1);
    assert!(harness.interrupt_line(16));
    assert!(harness.resets.is_empty());
    harness.run_for_seconds(1.9 / 32768.0);
    assert!(harness.resets.is_empty());
    harness.run_for_seconds(0.2 / 32768.0);
    assert_eq!(harness.resets, vec![ResetKind::Watchdog]);
}

#[test]
fn watchdog_keeps_running_through_a_soft_reset() {
    let mut harness = Harness::new();
    start_half_second_watchdog(&mut harness);
    harness.run_for_seconds(0.3);
    reset(&mut harness, ResetKind::SoftReset);
    assert_eq!(harness.read(WDT + 0x400), 1);
    harness.run_for_seconds(0.3);
    assert_eq!(harness.resets, vec![ResetKind::Watchdog]);
}

#[test]
fn watchdog_ignores_new_settings_while_running() {
    let mut harness = Harness::new();
    start_half_second_watchdog(&mut harness);
    harness.write(WDT + 0x504, 5);
    assert_eq!(harness.read(WDT + 0x504), 16383);
}

// ============================================================================
// Clocks
// ============================================================================

#[test]
fn high_frequency_clock_start_raises_its_event() {
    let mut harness = Harness::new();
    harness.write(0x4000_0000, 1);
    assert_eq!(harness.read(0x4000_0100), 1);
    assert_eq!(harness.read(0x4000_040C), 0x1_0001);
}

#[test]
fn running_watchdog_keeps_the_low_frequency_clock_on() {
    let mut harness = Harness::new();
    harness.write(0x4000_0008, 1);
    assert_eq!(harness.read(0x4000_0418), 0x1_0000);
    start_half_second_watchdog(&mut harness);
    harness.write(0x4000_000C, 1);
    assert_eq!(harness.read(0x4000_0418), 0x1_0000);
}

#[test]
fn low_frequency_clock_stops_without_the_watchdog() {
    let mut harness = Harness::new();
    harness.write(0x4000_0008, 1);
    harness.write(0x4000_000C, 1);
    assert_eq!(harness.read(0x4000_0418), 0);
}

#[test]
fn calibration_timer_times_out_after_ctiv_quarter_seconds() {
    let mut harness = Harness::new();
    harness.write(0x4000_0538, 2);
    harness.write(0x4000_0304, 0x10);
    harness.write(0x4000_0014, 1);
    harness.run_for_seconds(0.49);
    assert_eq!(harness.read(0x4000_0110), 0);
    harness.run_for_seconds(0.02);
    assert_eq!(harness.read(0x4000_0110), 1);
    assert!(harness.interrupt_line(0));
}

#[test]
fn system_off_stops_the_chip() {
    let mut harness = Harness::new();
    harness.write(0x4000_0500, 1);
    assert!(harness.chip.system_off());
}

// ============================================================================
// NVMC
// ============================================================================

#[test]
fn erases_a_page_only_with_erase_enabled() {
    let mut harness = Harness::new();
    debugger_write(&mut harness, 0x2000, 0x1234_5678);
    harness.write(NVMC_ERASEPAGE, 0x2000);
    assert_eq!(harness.read(0x2000), 0x1234_5678);
    harness.write(NVMC_CONFIG, 0x2);
    harness.write(NVMC_ERASEPAGE, 0x2000);
    assert_eq!(harness.read(0x2000), 0xFFFF_FFFF);
}

#[test]
fn ignores_a_cpu_write_to_flash_without_write_enabled() {
    let mut harness = Harness::new();
    debugger_write(&mut harness, 0x2000, 0x1234_5678);
    harness.write(0x2000, 0x0000_FFFF);
    assert_eq!(harness.read(0x2000), 0x1234_5678);
    assert_eq!(harness.chip.ignored_flash_write_count(), 1);
}

#[test]
fn lets_a_cpu_write_only_clear_bits_with_write_enabled() {
    let mut harness = Harness::new();
    debugger_write(&mut harness, 0x2000, 0x1234_5678);
    harness.write(NVMC_CONFIG, 0x1);
    harness.write(0x2000, 0x0000_FFFF);
    assert_eq!(harness.read(0x2000), 0x0000_5678);
}

#[test]
fn erases_uicr_and_everything() {
    let mut harness = Harness::new();
    harness.write(NVMC_CONFIG, 0x1);
    harness.write(0x1000_1080, 0x1234_5678);
    assert_eq!(harness.read(0x1000_1080), 0x1234_5678);
    harness.write(0x100, 0);
    harness.write(NVMC_CONFIG, 0x2);
    harness.write(0x4001_E514, 1);
    assert_eq!(harness.read(0x1000_1080), 0xFFFF_FFFF);
    assert_eq!(harness.read(0x100), 0);
    harness.write(0x4001_E50C, 1);
    assert_eq!(harness.read(0x100), 0xFFFF_FFFF);
    assert_eq!(harness.read(0x4001_E400), 1);
}

#[test]
fn starts_with_erased_flash_and_the_pinetime_ficr() {
    let mut harness = Harness::new();
    assert_eq!(harness.read(0x0), 0xFFFF_FFFF);
    assert_eq!(harness.read(0x1000_1000), 0xFFFF_FFFF);
    assert_eq!(harness.read(0x1000_0010), 0x1000);
    assert_eq!(harness.read(0x1000_0014), 0x80);
    assert_eq!(harness.read(0x1000_0100), 0x52832);
    assert_eq!(harness.read(0x1000_010C), 0x40);
    assert_eq!(harness.read(0xF000_0FE0), 0x06);
    assert_eq!(harness.read(0xF000_0FE8), 0x50);
    // The CPU cannot write the FICR.
    harness.write(0x1000_0010, 0);
    assert_eq!(harness.read(0x1000_0010), 0x1000);
}

// ============================================================================
// Flash wait states and the instruction cache
// ============================================================================

#[test]
fn flash_makes_the_cpu_wait_and_ram_does_not() {
    let mut harness = Harness::new();
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 2);
    assert_eq!(harness.chip.data_wait_cycles(0x100), 2);
    assert_eq!(harness.chip.fetch_wait_cycles(0x2000_0100), 0);
    assert_eq!(harness.chip.data_wait_cycles(0x2000_0100), 0);
    assert_eq!(harness.chip.data_wait_cycles(0x4000_0000), 0);
}

#[test]
fn the_cache_saves_fetch_waits_and_counts_hits_and_misses() {
    let mut harness = Harness::new();
    harness.write(NVMC_ICACHECNF, 0x101);
    assert_eq!(harness.read(NVMC_ICACHECNF), 0x101);
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 3);
    assert_eq!(harness.chip.fetch_wait_cycles(0x104), 0);
    assert_eq!((harness.read(NVMC_IHIT), harness.read(NVMC_IMISS)), (1, 1));
    // Data reads go round the cache.
    assert_eq!(harness.chip.data_wait_cycles(0x100), 2);
    harness.write(NVMC_IHIT, 0);
    harness.write(NVMC_IMISS, 0);
    assert_eq!((harness.read(NVMC_IHIT), harness.read(NVMC_IMISS)), (0, 0));
}

#[test]
fn writing_flash_or_turning_the_cache_off_empties_it() {
    let mut harness = Harness::new();
    harness.write(NVMC_ICACHECNF, 1);
    harness.chip.fetch_wait_cycles(0x100);
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 0);
    harness.write(NVMC_CONFIG, 1);
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 2);
    harness.write(NVMC_CONFIG, 0);
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 3);
    harness.write(NVMC_ICACHECNF, 0);
    harness.write(NVMC_ICACHECNF, 1);
    assert_eq!(harness.chip.fetch_wait_cycles(0x100), 3);
}
