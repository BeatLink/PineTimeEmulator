// SPDX-License-Identifier: GPL-3.0-only
//! SAADC, TEMP, CCM, AAR, MWU, PWM, COMP and QDEC, ported from tests/chip_peripherals.robot.

mod common;

use common::Harness;

const SAADC: u32 = 0x4000_7000;
const TEMP: u32 = 0x4000_C000;
const CCM: u32 = 0x4000_F000;
const MWU: u32 = 0x4002_0000;
const PWM: u32 = 0x4001_C000;
const COMP: u32 = 0x4001_3000;
const QDEC: u32 = 0x4001_2000;

fn duty(harness: &Harness, channel: usize) -> f64 {
    harness.chip.pwm_duty_cycle(0, channel)
}

fn assert_duty(harness: &Harness, channel: usize, expected: f64) {
    let actual = duty(harness, channel);
    assert!(
        (actual - expected).abs() < 1e-4,
        "channel {channel}: {actual} instead of {expected}"
    );
}

// ============================================================================
// SAADC
// ============================================================================

#[test]
fn saadc_converts_a_single_ended_input_at_10_bits() {
    let mut harness = Harness::new();
    harness.board.analog_volts[0] = Some(1.65);
    harness.write(SAADC + 0x500, 1);
    // AIN0, gain 1/6, internal 0.6 V reference (the CONFIG reset value), 10 bits.
    harness.write(SAADC + 0x510, 1);
    harness.write(SAADC + 0x5F0, 1);
    harness.write(SAADC + 0x62C, 0x2000_0400);
    harness.write(SAADC + 0x630, 1);
    harness.write(SAADC + 0x304, 0x2);
    harness.write(SAADC, 1);
    assert_eq!(harness.read(SAADC + 0x100), 1);
    harness.write(SAADC + 0x004, 1);
    // 1.65 V * (1/6) / 0.6 V * 1024 = 469.33, so 469.
    assert_eq!(harness.read_halfword(0x2000_0400), 469);
    assert_eq!(harness.read(SAADC + 0x634), 1);
    assert_eq!(harness.read(SAADC + 0x104), 1);
    assert_eq!(harness.read(SAADC + 0x108), 1);
    assert_eq!(harness.read(SAADC + 0x10C), 1);
    // END is enabled, so SAADC_IRQn (7) is raised.
    assert!(harness.interrupt_line(7));
}

#[test]
fn saadc_converts_two_channels_at_12_bits_into_ram() {
    let mut harness = Harness::new();
    harness.board.analog_volts[7] = Some(2.0);
    harness.write(SAADC + 0x500, 1);
    // Channel 0 reads VDD (3.3 V by default), channel 1 reads AIN7, both at gain 1/6 and the internal reference.
    harness.write(SAADC + 0x510, 9);
    harness.write(SAADC + 0x520, 8);
    harness.write(SAADC + 0x5F0, 2);
    harness.write(SAADC + 0x62C, 0x2000_0400);
    harness.write(SAADC + 0x630, 2);
    harness.write(SAADC, 1);
    harness.write(SAADC + 0x004, 1);
    // 3.3 / 6 / 0.6 * 4096 = 3754.67 and 2.0 / 6 / 0.6 * 4096 = 2275.56.
    assert_eq!(harness.read_halfword(0x2000_0400), 3754);
    assert_eq!(harness.read_halfword(0x2000_0402), 2275);
    assert_eq!(harness.read(SAADC + 0x634), 2);
    assert_eq!(harness.read(SAADC + 0x104), 1);
}

#[test]
fn saadc_uses_the_vdd_quarter_reference_gain_and_limits() {
    let mut harness = Harness::new();
    harness.board.analog_volts[2] = Some(0.5);
    harness.chip.set_supply_volts(3.0);
    harness.write(SAADC + 0x500, 1);
    harness.write(SAADC + 0x510, 3);
    // Gain 1/4 (2), VDD/4 reference, 10 bits: 0.5 * 0.25 / 0.75 * 1024 = 170.67.
    harness.write(SAADC + 0x518, 0x0002_1200);
    // A high limit of 100 makes the result raise CH[0].LIMITH.
    harness.write(SAADC + 0x51C, 0x0064_8000);
    harness.write(SAADC + 0x62C, 0x2000_0400);
    harness.write(SAADC + 0x630, 1);
    harness.write(SAADC, 1);
    harness.write(SAADC + 0x004, 1);
    assert_eq!(harness.read_halfword(0x2000_0400), 170);
    assert_eq!(harness.read(SAADC + 0x118), 1);
    assert_eq!(harness.read(SAADC + 0x11C), 0);
    harness.write(SAADC + 0x00C, 1);
    assert_eq!(harness.read(SAADC + 0x110), 1);
    harness.write(SAADC + 0x008, 1);
    assert_eq!(harness.read(SAADC + 0x114), 1);
}

#[test]
fn saadc_samples_continuously_from_its_timer() {
    let mut harness = Harness::new();
    harness.board.analog_volts[0] = Some(0.3);
    harness.write(SAADC + 0x500, 1);
    harness.write(SAADC + 0x510, 1);
    harness.write(SAADC + 0x62C, 0x2000_0400);
    harness.write(SAADC + 0x630, 4);
    // 16 MHz / 1600 is one sample every 100 us.
    harness.write(SAADC + 0x5F8, 0x1000 | 1600);
    harness.write(SAADC, 1);
    harness.write(SAADC + 0x004, 1);
    assert_eq!(harness.read(SAADC + 0x634), 1);
    harness.run_for_microseconds(250.0);
    assert_eq!(harness.read(SAADC + 0x634), 3);
    harness.run_for_microseconds(100.0);
    assert_eq!(harness.read(SAADC + 0x104), 1);
    harness.write(SAADC + 0x008, 1);
    assert_eq!(harness.next_deadline(), None);
}

// ============================================================================
// TEMP
// ============================================================================

#[test]
fn temp_reports_the_set_temperature_in_quarter_degrees() {
    let mut harness = Harness::new();
    harness.chip.set_die_temperature(31.5);
    harness.write(TEMP, 1);
    assert_eq!(harness.read(TEMP + 0x100), 1);
    assert_eq!(harness.read(TEMP + 0x508), 126);
    harness.chip.set_die_temperature(-10.25);
    harness.write(TEMP + 0x100, 0);
    harness.write(TEMP, 1);
    assert_eq!(harness.read(TEMP + 0x508), 0xFFFF_FFD7);
    assert_eq!(harness.read(TEMP + 0x520), 0x320);
    assert_eq!(harness.read(TEMP + 0x554), 0x3DD0);
    assert_eq!(harness.read(TEMP + 0x570), 0x50);
    harness.write(TEMP + 0x520, 0x123);
    assert_eq!(harness.read(TEMP + 0x520), 0x123);
}

// ============================================================================
// CCM and AAR
// ============================================================================

fn configure_ccm(harness: &mut Harness, counter_low: u32, counter_high: u32, direction: u32) {
    // The session key from the Bluetooth Core sample data, MSB first.
    harness.write_words(0x2000_1000, &[0x521B_AD99, 0x3E7E_A326, 0x8E3B_8E05, 0x66C6_C227]);
    harness.write_words(0x2000_1010, &[counter_low, counter_high]);
    // The direction byte, then the IV 0xDEAFBABEBADCAB24 least significant byte first.
    harness.write_words(0x2000_1018, &[0xDCAB_2400 | direction, 0xAFBA_BEBA, 0x0000_00DE]);
    harness.write(CCM + 0x500, 2);
    harness.write(CCM + 0x508, 0x2000_1000);
}

fn run_ccm(harness: &mut Harness, mode: u32, input: u32, output: u32) {
    harness.write(CCM + 0x504, mode);
    harness.write(CCM + 0x50C, input);
    harness.write(CCM + 0x510, output);
    harness.write(CCM + 0x104, 0);
    harness.write(CCM + 0x200, 1);
    harness.write(CCM, 1);
    assert_eq!(harness.read(CCM + 0x100), 1);
    assert_eq!(harness.read(CCM + 0x104), 1);
}

fn configure_aar(harness: &mut Harness) {
    // A wrong IRK first, then the Bluetooth Core sample IRK 0xEC0234A357C8AD05341010A60A397D9B, MSB first.
    harness.write_words(0x2000_1FF0, &[0x1111_1111, 0x2222_2222, 0x3333_3333, 0x4444_4444]);
    harness.write_words(0x2000_2000, &[0xA334_02EC, 0x05AD_C857, 0xA610_1034, 0x9B7D_390A]);
    harness.write(CCM + 0x500, 3);
    harness.write(CCM + 0x504, 2);
    harness.write(CCM + 0x508, 0x2000_1FF0);
    harness.write(CCM + 0x510, 0x2000_2100);
}

#[test]
fn ccm_encrypts_the_bluetooth_core_sample_packet() {
    let mut harness = Harness::new();
    configure_ccm(&mut harness, 0, 0, 1);
    // LL_START_ENC_RSP, master to slave: header 0x0F, length 1, payload 0x06.
    harness.write(0x2000_1100, 0x0600_010F);
    run_ccm(&mut harness, 0, 0x2000_1100, 0x2000_1200);
    // 0F 05 00 9F CD A7 F4 48: the encrypted opcode and its MIC from the specification.
    assert_eq!(harness.read(0x2000_1200), 0x9F00_050F);
    assert_eq!(harness.read(0x2000_1204), 0x48F4_A7CD);
}

#[test]
fn ccm_encrypts_the_slave_to_master_sample_packet() {
    let mut harness = Harness::new();
    configure_ccm(&mut harness, 0, 0, 0);
    harness.write(0x2000_1100, 0x0600_010F);
    run_ccm(&mut harness, 0, 0x2000_1100, 0x2000_1200);
    assert_eq!(harness.read(0x2000_1200), 0xA300_050F);
    assert_eq!(harness.read(0x2000_1204), 0x15A4_134C);
}

#[test]
fn ccm_round_trips_a_27_byte_payload_and_detects_a_bad_mic() {
    let mut harness = Harness::new();
    configure_ccm(&mut harness, 0x3456_789A, 0x0000_0012, 0);
    harness.write_words(
        0x2000_1100,
        &[
            0x0100_1B02,
            0x0504_0302,
            0x0908_0706,
            0x0D0C_0B0A,
            0x1110_0F0E,
            0x1514_1312,
            0x1918_1716,
            0x0000_1B1A,
        ],
    );
    run_ccm(&mut harness, 0, 0x2000_1100, 0x2000_1200);
    // The expected ciphertext and MIC come from an independent AES-CCM implementation.
    assert_eq!(harness.read(0x2000_1200), 0x8D00_1F02);
    assert_eq!(harness.read(0x2000_1204), 0xC41D_1825);
    assert_eq!(harness.read(0x2000_1210), 0x6E08_490B);
    assert_eq!(harness.read(0x2000_121C), 0xC5FD_DBAA);
    assert_eq!(harness.read(0x2000_1220), 0x0000_22B2);
    run_ccm(&mut harness, 1, 0x2000_1200, 0x2000_1300);
    assert_eq!(harness.read(CCM + 0x400), 1);
    assert_eq!(harness.read(0x2000_1300), 0x0100_1B02);
    assert_eq!(harness.read(0x2000_1304), 0x0504_0302);
    assert_eq!(harness.read(0x2000_1318), 0x1918_1716);
    assert_eq!(harness.read_halfword(0x2000_131C), 0x1B1A);
    // Flip one MIC bit: the payload still decrypts but MICSTATUS reports the failure.
    harness.write(0x2000_1220, 0x0000_22B3);
    run_ccm(&mut harness, 1, 0x2000_1200, 0x2000_1400);
    assert_eq!(harness.read(CCM + 0x400), 0);
    assert_eq!(harness.read(0x2000_1404), 0x0504_0302);
}

#[test]
fn aar_resolves_the_bluetooth_core_sample_address() {
    let mut harness = Harness::new();
    configure_aar(&mut harness);
    // A 3-byte header, then the address 0x708194_0DFBAA (prand 0x708194, hash 0x0DFBAA) LSB first.
    harness.write_words(0x2000_2100, &[0xAA00_0000, 0x8194_0DFB, 0x0000_0070]);
    harness.write(CCM, 1);
    assert_eq!(harness.read(CCM + 0x100), 1);
    assert_eq!(harness.read(CCM + 0x104), 1);
    assert_eq!(harness.read(CCM + 0x108), 0);
    assert_eq!(harness.read(CCM + 0x400), 1);
}

#[test]
fn aar_rejects_an_address_no_irk_resolves() {
    let mut harness = Harness::new();
    configure_aar(&mut harness);
    harness.write_words(0x2000_2100, &[0xAB00_0000, 0x8194_0DFB, 0x0000_0070]);
    harness.write(CCM, 1);
    assert_eq!(harness.read(CCM + 0x100), 1);
    assert_eq!(harness.read(CCM + 0x104), 0);
    assert_eq!(harness.read(CCM + 0x108), 1);
}

// ============================================================================
// MWU
// ============================================================================

#[test]
fn mwu_registers_read_back() {
    let mut harness = Harness::new();
    harness.write(MWU + 0x620, 0x2000_1000);
    harness.write(MWU + 0x624, 0x2000_1FFF);
    assert_eq!(harness.read(MWU + 0x620), 0x2000_1000);
    assert_eq!(harness.read(MWU + 0x624), 0x2000_1FFF);
    harness.write(MWU + 0x514, 0xFFFF_FFFF);
    assert_eq!(harness.read(MWU + 0x510), 0x0F00_00FF);
    harness.write(MWU + 0x518, 0x0F00_0000);
    assert_eq!(harness.read(MWU + 0x510), 0x0000_00FF);
    harness.write(MWU + 0x324, 0x3);
    assert_eq!(harness.read(MWU + 0x320), 0x3);
    harness.write(MWU + 0x304, 0x0100_0000);
    assert_eq!(harness.read(MWU + 0x300), 0x0100_0000);
    harness.write(MWU + 0x6D8, 0x00FF_00FF);
    assert_eq!(harness.read(MWU + 0x6D8), 0x00FF_00FF);
    assert_eq!(harness.read(MWU + 0x6D0), 0);
    harness.write(MWU + 0x160, 1);
    assert_eq!(harness.read(MWU + 0x160), 1);
    assert!(harness.interrupt_line(32));
}

#[test]
fn mwu_event_can_be_raised_by_hand() {
    let mut harness = Harness::new();
    harness.write(MWU + 0x304, 0x1);
    harness.with_ports(|chip, ports| chip.trigger_mwu_event(0, ports));
    assert_eq!(harness.read(MWU + 0x100), 1);
    assert!(harness.interrupt_line(32));
}

// ============================================================================
// PWM
// ============================================================================

fn configure_pwm_with_a_one_millisecond_period(harness: &mut Harness) {
    harness.write(PWM + 0x500, 1);
    harness.write(PWM + 0x508, 16000);
    harness.write(PWM + 0x50C, 0);
    harness.write(PWM + 0x510, 0);
}

fn start_pwm_sequence(harness: &mut Harness, sequence: u32, pointer: u32, count: u32) {
    let base = PWM + 0x520 + 0x20 * sequence;
    harness.write(base, pointer);
    harness.write(base + 4, count);
    harness.write(base + 8, 0);
    harness.write(PWM + 0x008 + 4 * sequence, 1);
}

#[test]
fn pwm_plays_a_sequence_in_emulated_time() {
    let mut harness = Harness::new();
    configure_pwm_with_a_one_millisecond_period(&mut harness);
    // Four steps of one period each: 25 %, 50 %, 75 %, then 100 % high.
    harness.write_words(0x2000_3000, &[0x9F40_8FA0, 0xBE80_AEE0]);
    harness.write(PWM + 0x304, 0x10);
    start_pwm_sequence(&mut harness, 0, 0x2000_3000, 4);
    assert_eq!(harness.read(PWM + 0x108), 1);
    assert_duty(&harness, 0, 0.25);
    harness.run_for_seconds(0.0015);
    assert_duty(&harness, 0, 0.5);
    assert_duty(&harness, 3, 0.5);
    harness.run_for_seconds(0.0014);
    assert_duty(&harness, 0, 0.75);
    assert_eq!(harness.read(PWM + 0x110), 0);
    // The last value is applied, and SEQEND raised, 3 ms after the start.
    harness.run_for_seconds(0.0002);
    assert_eq!(harness.read(PWM + 0x110), 1);
    assert_duty(&harness, 0, 1.0);
    assert!(harness.interrupt_line(28));
}

#[test]
fn pwm_inverts_polarity_without_bit_15() {
    let mut harness = Harness::new();
    configure_pwm_with_a_one_millisecond_period(&mut harness);
    // Individual load: 4000 without the polarity bit is 75 % high; 0x8000 | 4000 is 25 %.
    harness.write(PWM + 0x510, 2);
    harness.write_words(0x2000_3000, &[0x8FA0_0FA0, 0x0000_8000]);
    start_pwm_sequence(&mut harness, 0, 0x2000_3000, 4);
    assert_duty(&harness, 0, 0.75);
    assert_duty(&harness, 1, 0.25);
    assert_duty(&harness, 2, 0.0);
    assert_duty(&harness, 3, 1.0);
}

#[test]
fn pwm_loops_both_sequences_and_stops_on_loopsdone() {
    let mut harness = Harness::new();
    configure_pwm_with_a_one_millisecond_period(&mut harness);
    harness.write_words(0x2000_3000, &[0x8000_AEE0]);
    harness.write(PWM + 0x514, 2);
    // LOOPSDONE_STOP.
    harness.write(PWM + 0x200, 0x10);
    harness.write(PWM + 0x540, 0x2000_3002);
    harness.write(PWM + 0x544, 1);
    harness.write(PWM + 0x548, 0);
    start_pwm_sequence(&mut harness, 0, 0x2000_3000, 1);
    assert_duty(&harness, 0, 0.75);
    harness.run_for_seconds(0.0015);
    assert_duty(&harness, 0, 0.0);
    assert_eq!(harness.read(PWM + 0x10C), 1);
    // Two loops of one 1 ms step in each sequence end after 4 ms.
    harness.run_for_seconds(0.0024);
    assert_eq!(harness.read(PWM + 0x11C), 0);
    harness.run_for_seconds(0.0002);
    assert_eq!(harness.read(PWM + 0x11C), 1);
    assert_eq!(harness.read(PWM + 0x104), 1);
    assert!(!harness.chip.pwm_running(0));
}

#[test]
fn pwm_stops_on_the_seqend_short() {
    let mut harness = Harness::new();
    configure_pwm_with_a_one_millisecond_period(&mut harness);
    harness.write_words(0x2000_3000, &[0x8000_8000]);
    harness.write(PWM + 0x200, 0x1);
    start_pwm_sequence(&mut harness, 0, 0x2000_3000, 2);
    assert_eq!(harness.read(PWM + 0x104), 0);
    harness.run_for_seconds(0.0011);
    assert_eq!(harness.read(PWM + 0x110), 1);
    assert_eq!(harness.read(PWM + 0x104), 1);
    assert_duty(&harness, 0, 0.0);
}

#[test]
fn pwm_reports_its_output_pins() {
    let mut harness = Harness::new();
    assert_eq!(harness.chip.pwm_output_pin(1, 0), None);
    harness.write(0x4002_1560, 14);
    assert_eq!(harness.chip.pwm_output_pin(1, 0), Some(14));
}

// ============================================================================
// COMP, LPCOMP and QDEC
// ============================================================================

#[test]
fn comp_and_lpcomp_accept_the_errata_trim_write() {
    let mut harness = Harness::new();
    assert_eq!(harness.read(COMP + 0x508), 4);
    harness.write(COMP + 0x540, 0x15);
    assert_eq!(harness.read(COMP + 0x540), 0x15);
    harness.write(COMP + 0x500, 1);
    harness.write(COMP, 1);
    assert_eq!(harness.read(COMP + 0x100), 1);
    harness.with_ports(|chip, ports| chip.set_comparator_input_above(true, ports));
    assert_eq!(harness.read(COMP + 0x108), 1);
    assert_eq!(harness.read(COMP + 0x10C), 1);
    assert_eq!(harness.read(COMP + 0x400), 1);
}

#[test]
fn qdec_has_its_reset_values() {
    let mut harness = Harness::new();
    assert_eq!(harness.read(QDEC + 0x540), 0x10);
    assert_eq!(harness.read(QDEC + 0x51C), 0xFFFF_FFFF);
    assert_eq!(harness.read(QDEC + 0x524), 0xFFFF_FFFF);
    harness.write(QDEC + 0x004, 1);
    assert_eq!(harness.read(QDEC + 0x110), 1);
}
