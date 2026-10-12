// SPDX-License-Identifier: GPL-3.0-only
//! RTC and TIMER, ported from tests/rtc_and_timer.robot.

mod common;

use common::Harness;

const RTC: u32 = 0x4000_B000;
const TIMER: u32 = 0x4000_8000;
const TIMER3: u32 = 0x4001_A000;
const PPI: u32 = 0x4001_F000;
const RTC_IRQ: u8 = 11;
const TIMER_IRQ: u8 = 8;
const TIMER3_IRQ: u8 = 26;
/// One tick of the RTC with PRESCALER 255 lasts 1/128 s.
const RTC_TICK: f64 = 0.0078125;

fn start_slow_rtc(harness: &mut Harness) {
    harness.write(RTC + 0x508, 255);
    harness.write(RTC, 1);
}

fn capture(harness: &mut Harness, base: u32, channel: u32) -> u32 {
    harness.write(base + 0x40 + 4 * channel, 1);
    harness.read(base + 0x540 + 4 * channel)
}

#[test]
fn rtc_counts_at_32768_hz() {
    let mut harness = Harness::new();
    harness.write(RTC, 1);
    harness.run_for_seconds(1.0);
    assert_eq!(harness.read(RTC + 0x504), 32768);
}

#[test]
fn rtc_counts_at_the_prescaled_rate() {
    let mut harness = Harness::new();
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.5);
    assert_eq!(harness.read(RTC + 0x504), 64);
    harness.run_for_seconds(0.0078);
    assert_eq!(harness.read(RTC + 0x504), 64);
    harness.run_for_seconds(0.00002);
    assert_eq!(harness.read(RTC + 0x504), 65);
}

#[test]
fn rtc_ignores_a_prescaler_write_while_running() {
    let mut harness = Harness::new();
    start_slow_rtc(&mut harness);
    harness.write(RTC + 0x508, 0);
    assert_eq!(harness.read(RTC + 0x508), 255);
    harness.write(RTC + 0x004, 1);
    harness.write(RTC + 0x508, 0);
    assert_eq!(harness.read(RTC + 0x508), 0);
}

#[test]
fn rtc_compare_raises_its_event_and_interrupt_on_time() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x540, 10);
    harness.write(RTC + 0x304, 0x10000);
    start_slow_rtc(&mut harness);
    // Ten ticks of 1/128 s is 78.125 ms.
    harness.run_for_seconds(0.078);
    assert_eq!(harness.read(RTC + 0x140), 0);
    assert!(!harness.interrupt_line(RTC_IRQ));
    harness.run_for_seconds(0.0002);
    assert_eq!(harness.read(RTC + 0x140), 1);
    assert_eq!(harness.read(RTC + 0x504), 10);
    assert!(harness.interrupt_line(RTC_IRQ));
    harness.write(RTC + 0x140, 0);
    assert!(!harness.interrupt_line(RTC_IRQ));
}

#[test]
fn rtc_compare_needs_inten_or_evten() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x544, 5);
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.1);
    assert_eq!(harness.read(RTC + 0x144), 0);
    assert!(!harness.interrupt_line(RTC_IRQ));
}

#[test]
fn rtc_compare_one_ahead_fires_and_compare_equal_waits_for_a_wrap() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x304, 0x30000);
    start_slow_rtc(&mut harness);
    // Half way through the tick where COUNTER is 64.
    harness.run_for_seconds(0.50390625);
    harness.write(RTC + 0x540, 65);
    harness.write(RTC + 0x544, 64);
    harness.run_for_seconds(RTC_TICK);
    assert_eq!(harness.read(RTC + 0x140), 1);
    assert_eq!(harness.read(RTC + 0x144), 0);
    harness.run_for_seconds(0.1);
    assert_eq!(harness.read(RTC + 0x144), 0);
}

#[test]
fn rtc_tick_event_only_when_enabled() {
    let mut harness = Harness::new();
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.1);
    assert_eq!(harness.read(RTC + 0x100), 0);
    // EVTEN alone sets the event register but raises no interrupt.
    harness.write(RTC + 0x344, 0x1);
    harness.run_for_seconds(0.01);
    assert_eq!(harness.read(RTC + 0x100), 1);
    assert!(!harness.interrupt_line(RTC_IRQ));
    harness.write(RTC + 0x348, 0x1);
    harness.write(RTC + 0x100, 0);
    harness.run_for_seconds(0.05);
    assert_eq!(harness.read(RTC + 0x100), 0);
    // INTEN alone sets the event register and raises the interrupt.
    harness.write(RTC + 0x304, 0x1);
    harness.run_for_seconds(0.01);
    assert_eq!(harness.read(RTC + 0x100), 1);
    assert!(harness.interrupt_line(RTC_IRQ));
    harness.write(RTC + 0x100, 0);
    assert!(!harness.interrupt_line(RTC_IRQ));
    harness.run_for_seconds(0.01);
    assert!(harness.interrupt_line(RTC_IRQ));
}

#[test]
fn rtc_trigger_overflow_sets_the_counter_near_the_top() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x00C, 1);
    assert_eq!(harness.read(RTC + 0x504), 0xFF_FFF0);
}

#[test]
fn rtc_overflows_sixteen_ticks_after_trigovrflw() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x304, 0x2);
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.5);
    harness.write(RTC + 0x00C, 1);
    assert_eq!(harness.read(RTC + 0x504), 0xFF_FFF0);
    // 14.5 ticks, then 15.5, then 16.5.
    harness.run_for_seconds(0.11328125);
    assert_eq!(harness.read(RTC + 0x504), 0xFF_FFFE);
    harness.run_for_seconds(RTC_TICK);
    assert_eq!(harness.read(RTC + 0x504), 0xFF_FFFF);
    assert_eq!(harness.read(RTC + 0x104), 0);
    assert!(!harness.interrupt_line(RTC_IRQ));
    harness.run_for_seconds(RTC_TICK);
    assert_eq!(harness.read(RTC + 0x504), 0);
    assert_eq!(harness.read(RTC + 0x104), 1);
    assert!(harness.interrupt_line(RTC_IRQ));
}

#[test]
fn rtc_clear_resets_the_counter_without_a_compare_at_zero() {
    let mut harness = Harness::new();
    harness.write(RTC + 0x540, 0);
    harness.write(RTC + 0x304, 0x10000);
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.5);
    assert_eq!(harness.read(RTC + 0x504), 64);
    harness.write(RTC + 0x008, 1);
    assert_eq!(harness.read(RTC + 0x504), 0);
    assert_eq!(harness.read(RTC + 0x140), 0);
    harness.run_for_seconds(0.04296875);
    assert_eq!(harness.read(RTC + 0x504), 5);
}

#[test]
fn rtc_stop_freezes_the_counter() {
    let mut harness = Harness::new();
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.25);
    harness.write(RTC + 0x004, 1);
    assert_eq!(harness.read(RTC + 0x504), 32);
    harness.run_for_seconds(1.0);
    assert_eq!(harness.read(RTC + 0x504), 32);
    harness.write(RTC, 1);
    harness.run_for_seconds(0.04296875);
    assert_eq!(harness.read(RTC + 0x504), 37);
}

#[test]
fn an_idle_rtc_schedules_nothing() {
    let mut harness = Harness::new();
    harness.write(RTC, 1);
    assert_eq!(harness.next_deadline(), None);
    harness.write(RTC + 0x304, 0x1);
    assert!(harness.next_deadline().is_some());
}

// The PPI runs its task at the instant of the event, so the capture is exact.
#[test]
fn rtc_tick_through_ppi_captures_the_timer() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER, 1);
    harness.write(PPI + 0x510, 0x4000_B100);
    harness.write(PPI + 0x514, 0x4000_8040);
    harness.write(PPI + 0x504, 0x1);
    harness.write(RTC + 0x344, 0x1);
    start_slow_rtc(&mut harness);
    // The first tick is at 7812.5 us.
    harness.run_for_seconds(0.005);
    assert_eq!(harness.read(TIMER + 0x540), 0);
    harness.run_for_seconds(0.005);
    assert_eq!(harness.read(TIMER + 0x540), 7812);
    assert_eq!(harness.read(RTC + 0x100), 1);
    assert!(!harness.interrupt_line(RTC_IRQ));
}

#[test]
fn rtc_compare_through_ppi_starts_the_timer() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(PPI + 0x510, 0x4000_B144);
    harness.write(PPI + 0x514, 0x4000_8000);
    harness.write(PPI + 0x504, 0x1);
    harness.write(RTC + 0x544, 4);
    harness.write(RTC + 0x344, 0x20000);
    start_slow_rtc(&mut harness);
    harness.run_for_seconds(0.03);
    assert_eq!(capture(&mut harness, TIMER, 1), 0);
    harness.run_for_seconds(0.01);
    // Four ticks is 31.25 ms, so 8.75 ms have been counted.
    assert_eq!(capture(&mut harness, TIMER, 1), 8750);
}

#[test]
fn timer_compare_at_the_exact_microsecond() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x540, 1000);
    harness.write(TIMER + 0x304, 0x10000);
    assert_eq!(harness.read(TIMER + 0x510), 4);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.000999);
    assert_eq!(harness.read(TIMER + 0x140), 0);
    assert!(!harness.interrupt_line(TIMER_IRQ));
    harness.run_for_seconds(0.000001);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    assert!(harness.interrupt_line(TIMER_IRQ));
    harness.write(TIMER + 0x308, 0x10000);
    assert!(!harness.interrupt_line(TIMER_IRQ));
}

#[test]
fn timer_capture_reads_the_exact_count() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x510, 0);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.001);
    assert_eq!(capture(&mut harness, TIMER, 2), 16000);
    harness.run_for_seconds(0.000_000_1);
    assert_eq!(capture(&mut harness, TIMER, 2), 16001);
}

#[test]
fn timer_compare_clear_shortcut_gives_periodic_events() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x540, 100);
    harness.write(TIMER + 0x200, 0x1);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.0000995);
    assert_eq!(harness.read(TIMER + 0x140), 0);
    harness.run_for_seconds(0.000001);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    harness.write(TIMER + 0x140, 0);
    harness.run_for_seconds(0.000099);
    assert_eq!(harness.read(TIMER + 0x140), 0);
    harness.run_for_seconds(0.000001);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 0);
    harness.run_for_seconds(0.00005);
    assert_eq!(capture(&mut harness, TIMER, 1), 50);
    // A hundred more periods still land on whole multiples of 100 us.
    harness.run_for_seconds(0.01);
    assert_eq!(capture(&mut harness, TIMER, 1), 50);
}

#[test]
fn timer_compare_stop_shortcut_freezes_the_counter() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x540, 100);
    harness.write(TIMER + 0x200, 0x100);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.001);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 100);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.00001);
    assert_eq!(capture(&mut harness, TIMER, 1), 110);
}

#[test]
fn timer_stop_and_clear_tasks() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.0002);
    harness.write(TIMER + 0x004, 1);
    harness.run_for_seconds(0.001);
    assert_eq!(capture(&mut harness, TIMER, 0), 200);
    harness.write(TIMER + 0x00C, 1);
    assert_eq!(capture(&mut harness, TIMER, 0), 0);
}

#[test]
fn timer_counter_mode_counts_count_tasks() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x504, 1);
    harness.write(TIMER + 0x540, 3);
    harness.write(TIMER + 0x008, 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 0);
    harness.write(TIMER, 1);
    harness.write(TIMER + 0x008, 1);
    harness.write(TIMER + 0x008, 1);
    harness.run_for_seconds(0.01);
    assert_eq!(harness.read(TIMER + 0x140), 0);
    assert_eq!(capture(&mut harness, TIMER, 1), 2);
    harness.write(TIMER + 0x540, 3);
    harness.write(TIMER + 0x008, 1);
    assert_eq!(harness.read(TIMER + 0x140), 1);
}

#[test]
fn timer_counter_mode_clear_shortcut() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x504, 2);
    harness.write(TIMER + 0x540, 2);
    harness.write(TIMER + 0x200, 0x1);
    harness.write(TIMER, 1);
    harness.write(TIMER + 0x008, 1);
    harness.write(TIMER + 0x008, 1);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 0);
    harness.write(TIMER + 0x008, 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 1);
}

#[test]
fn timer_wraps_at_8_bits() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 1);
    harness.write(TIMER + 0x540, 0);
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.000255);
    assert_eq!(harness.read(TIMER + 0x140), 0);
    harness.run_for_seconds(0.000045);
    assert_eq!(harness.read(TIMER + 0x140), 1);
    assert_eq!(capture(&mut harness, TIMER, 1), 44);
}

#[test]
fn timer_wraps_at_16_bits() {
    let mut harness = Harness::new();
    harness.write(TIMER, 1);
    harness.run_for_seconds(0.07);
    assert_eq!(capture(&mut harness, TIMER, 0), 4464);
}

#[test]
fn timer_wraps_at_24_bits() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 2);
    harness.write(TIMER + 0x510, 0);
    harness.write(TIMER, 1);
    harness.run_for_seconds(1.1);
    assert_eq!(capture(&mut harness, TIMER, 0), 822_784);
}

#[test]
fn timer_wraps_at_32_bits() {
    let mut harness = Harness::new();
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x510, 0);
    harness.write(TIMER, 1);
    harness.run_for_seconds(1.1);
    assert_eq!(capture(&mut harness, TIMER, 0), 17_600_000);
    harness.run_for_seconds(268.9);
    assert_eq!(capture(&mut harness, TIMER, 0), 25_032_704);
}

#[test]
fn timer_with_six_channels_captures_on_channel_5() {
    let mut harness = Harness::new();
    harness.write(TIMER3 + 0x508, 3);
    harness.write(TIMER3 + 0x554, 20);
    harness.write(TIMER3 + 0x200, 0x2020);
    harness.write(TIMER3 + 0x304, 0x20_0000);
    harness.write(TIMER3, 1);
    harness.run_for_seconds(0.0001);
    assert_eq!(harness.read(TIMER3 + 0x154), 1);
    assert!(harness.interrupt_line(TIMER3_IRQ));
    assert_eq!(capture(&mut harness, TIMER3, 5), 0);
}
