// SPDX-License-Identifier: GPL-3.0-only
//! The watch and the Bluetooth bridge on one air: packets cross between an nRF52832 and an
//! nRF52840 at their bit times while the two machines run in lockstep.

use pinetime_core::{AccessSize, Cycle};
use pinetime_emulator::bluetooth::{
    lookahead, run_in_lockstep, BridgeBoard, Outbox, ACTIVE_LOOKAHEAD, IDLE_RADIO_LATENCY, MAXIMUM_STEP,
};
use pinetime_emulator::machine::{Board, Machine};
use pinetime_emulator::monitor::Monitor;
use pinetime_nrf52832::{Nrf52832, Variant};

const RADIO: u32 = 0x4000_1000;
const TX_BUFFER: u32 = 0x2000_1000;
const RX_BUFFER: u32 = 0x2000_2000;
const ANSWER_BUFFER: u32 = 0x2000_3000;

const READY_START: u32 = 0x1;
const END_DISABLE: u32 = 0x2;
const DISABLED_TXEN: u32 = 0x4;
const DISABLED_RXEN: u32 = 0x8;

const CYCLES_PER_MICROSECOND: Cycle = 64;

// ============================================================================
// Two machines on one air
// ============================================================================

struct Air {
    watch: Machine,
    bridge: Machine<BridgeBoard>,
    watch_outbox: Outbox,
    bridge_outbox: Outbox,
    largest_gap: Cycle,
}

impl Air {
    fn new() -> Air {
        let mut watch = Machine::new();
        let mut bridge = Machine::with_parts(Nrf52832::new_variant(Variant::Nrf52840), BridgeBoard, 0);
        load_sleeping_program(&mut watch);
        load_sleeping_program(&mut bridge);
        let watch_outbox = Outbox::default();
        let bridge_outbox = Outbox::default();
        watch.chip.set_radio_medium(Some(Box::new(watch_outbox.clone())));
        bridge
            .chip
            .set_radio_medium(Some(Box::new(bridge_outbox.clone())));
        configure_radio(&mut watch);
        configure_radio(&mut bridge);
        Air {
            watch,
            bridge,
            watch_outbox,
            bridge_outbox,
            largest_gap: 0,
        }
    }

    fn run_for_microseconds(&mut self, microseconds: u64) {
        let target = self.watch.now().max(self.bridge.now()) + microseconds * CYCLES_PER_MICROSECOND;
        let mut largest_gap = self.largest_gap;
        run_in_lockstep(
            &mut self.watch,
            &self.watch_outbox,
            &mut self.bridge,
            &self.bridge_outbox,
            target,
            |watch, bridge| largest_gap = largest_gap.max(watch.now().abs_diff(bridge.now())),
        );
        self.largest_gap = largest_gap;
    }
}

/// A vector table and a `wfi` loop, so the CPU sleeps while the tests drive the radio.
fn load_sleeping_program<B: Board>(machine: &mut Machine<B>) {
    const WAIT_FOR_INTERRUPT_LOOP: [u16; 2] = [0xBF30, 0xE7FD];
    write(machine, 0x0, 0x2000_8000);
    write(machine, 0x4, 0x101);
    for (index, halfword) in WAIT_FOR_INTERRUPT_LOOP.iter().enumerate() {
        machine
            .write_for_debugger(0x100 + 2 * index as u32, *halfword as u32, AccessSize::Halfword)
            .unwrap();
    }
}

fn write<B: Board>(machine: &mut Machine<B>, address: u32, value: u32) {
    machine
        .write_for_debugger(address, value, AccessSize::Word)
        .unwrap();
}

fn read<B: Board>(machine: &mut Machine<B>, address: u32) -> u32 {
    machine.read_for_debugger(address, AccessSize::Word).unwrap()
}

// Bluetooth LE advertising settings at 1 Mbit: S0, an 8-bit LENGTH and an S1 byte in RAM, the advertising access address, channel 37.
fn configure_radio<B: Board>(machine: &mut Machine<B>) {
    for (offset, value) in [
        (0x510, 3),
        (0x514, 0x0010_0108),
        (0x518, 0x0203_00FF),
        (0x51C, 0x89BE_D600),
        (0x524, 0x8E),
        (0x52C, 0),
        (0x530, 1),
        (0x534, 0x103),
        (0x538, 0x65B),
        (0x53C, 0x55_5555),
        (0x508, 2),
        (0x554, 37),
        (0x544, 150),
        (0x650, 0x201),
    ] {
        write(machine, RADIO + offset, value);
    }
    machine.chip.set_event_trace(true);
}

// ADV_IND with a random address C0:DE:0A:1B:2C:3D and the flags 06.
fn send_advertisement<B: Board>(machine: &mut Machine<B>, shortcuts: u32) {
    for (index, word) in [0x3D00_0940, 0xDE0A_1B2C, 0x0601_02C0].into_iter().enumerate() {
        write(machine, TX_BUFFER + 4 * index as u32, word);
    }
    write(machine, RADIO + 0x504, TX_BUFFER);
    write(machine, RADIO + 0x200, shortcuts);
    write(machine, RADIO, 1);
}

fn listen<B: Board>(machine: &mut Machine<B>, shortcuts: u32) {
    write(machine, RADIO + 0x504, RX_BUFFER);
    write(machine, RADIO + 0x200, shortcuts);
    write(machine, RADIO + 0x004, 1);
}

fn event_name(offset: u32) -> String {
    match offset {
        0x100 => "READY".into(),
        0x104 => "ADDRESS".into(),
        0x108 => "PAYLOAD".into(),
        0x10C => "END".into(),
        0x110 => "DISABLED".into(),
        0x130 => "CRCOK".into(),
        0x134 => "CRCERROR".into(),
        0x154 => "TXREADY".into(),
        0x158 => "RXREADY".into(),
        0x16C => "PHYEND".into(),
        _ => format!("0x{offset:X}"),
    }
}

/// The radio events traced so far, each with its time in microseconds.
fn radio_events<B: Board>(machine: &mut Machine<B>) -> String {
    machine
        .chip
        .take_event_trace()
        .into_iter()
        .filter(|(_, address)| address & !0xFFF == RADIO)
        .map(|(cycle, address)| {
            let time = format!("{:.3}", cycle as f64 / CYCLES_PER_MICROSECOND as f64);
            let time = time.trim_end_matches('0').trim_end_matches('.');
            format!("{}@{}", event_name(address & 0xFFF), time)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================================
// Packets between the watch and the bridge
// ============================================================================

#[test]
fn an_advertisement_from_the_watch_reaches_the_bridge_at_its_air_times() {
    let mut air = Air::new();
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);

    assert_eq!(
        radio_events(&mut air.watch),
        "READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198"
    );
    // The first bit is on air 4 us after START, the address ends 40 us later, and the receiver reports it 6 us after that.
    assert_eq!(
        radio_events(&mut air.bridge),
        "READY@40 RXREADY@40 ADDRESS@90 PAYLOAD@178 CRCOK@202 END@202 PHYEND@202 DISABLED@202"
    );
    assert_eq!(read(&mut air.bridge, RX_BUFFER), 0x3D00_0940);
    assert_eq!(read(&mut air.bridge, RX_BUFFER + 4), 0xDE0A_1B2C);
    assert_eq!(read(&mut air.bridge, RX_BUFFER + 8), 0x0601_02C0);
    assert_eq!(read(&mut air.bridge, RADIO + 0x400), 1);
}

#[test]
fn the_bridge_answers_exactly_tifs_after_the_advertisement() {
    let mut air = Air::new();
    write(&mut air.bridge, ANSWER_BUFFER, 0x0000_0004);
    write(&mut air.bridge, ANSWER_BUFFER + 4, 0);
    listen(&mut air.bridge, READY_START | END_DISABLE | DISABLED_TXEN);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE | DISABLED_RXEN);
    // Between DISABLED and START the bridge points at its answer and drops the turnaround shortcut, as firmware does.
    air.run_for_microseconds(250);
    write(&mut air.bridge, RADIO + 0x504, ANSWER_BUFFER);
    write(&mut air.bridge, RADIO + 0x200, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);

    // The advertisement ends on air at 196 us; the answer starts on air at 346 us, so START is at 342 us.
    assert_eq!(
        radio_events(&mut air.bridge),
        "READY@40 RXREADY@40 ADDRESS@90 PAYLOAD@178 CRCOK@202 END@202 PHYEND@202 DISABLED@202 \
         READY@342 TXREADY@342 ADDRESS@382 PAYLOAD@398 END@422 PHYEND@422 DISABLED@428"
    );
    assert_eq!(
        radio_events(&mut air.watch),
        "READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198 READY@238 ADDRESS@392 PAYLOAD@408 CRCOK@432 END@432 DISABLED@432 READY@472"
    );
}

#[test]
fn a_two_megabit_packet_arrives_on_its_shorter_bit_times() {
    let mut air = Air::new();
    for machine_radio in [0x510, 0x514] {
        let value = if machine_radio == 0x510 { 4 } else { 0x0110_0108 };
        write(&mut air.watch, RADIO + machine_radio, value);
        write(&mut air.bridge, RADIO + machine_radio, value);
    }
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);

    assert_eq!(
        radio_events(&mut air.watch),
        "READY@40 ADDRESS@64 PAYLOAD@108 END@120 DISABLED@124"
    );
    // On air from 43 us, the address ends 24 us later and the receiver reports it 2 us after that.
    assert_eq!(
        radio_events(&mut air.bridge),
        "READY@40 RXREADY@40 ADDRESS@69 PAYLOAD@113 CRCOK@125 END@125 PHYEND@125 DISABLED@125"
    );
}

#[test]
fn a_different_access_address_is_not_received() {
    let mut air = Air::new();
    write(&mut air.bridge, RADIO + 0x51C, 0x1234_5600);
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);
    assert_eq!(radio_events(&mut air.bridge), "READY@40 RXREADY@40");
    assert_eq!(read(&mut air.bridge, RADIO + 0x550), 3);
}

#[test]
fn another_channel_is_not_received() {
    let mut air = Air::new();
    write(&mut air.bridge, RADIO + 0x508, 26);
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);
    assert_eq!(radio_events(&mut air.bridge), "READY@40 RXREADY@40");
}

#[test]
fn a_different_crc_seed_gives_a_crc_error() {
    let mut air = Air::new();
    write(&mut air.bridge, RADIO + 0x53C, 0x12_3456);
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(1000);
    assert_eq!(
        radio_events(&mut air.bridge),
        "READY@40 RXREADY@40 ADDRESS@90 PAYLOAD@178 CRCERROR@202 END@202 PHYEND@202 DISABLED@202"
    );
    assert_eq!(read(&mut air.bridge, RADIO + 0x400), 0);
}

// ============================================================================
// Lockstep
// ============================================================================

#[test]
fn the_machines_stay_within_one_step_of_each_other() {
    let mut air = Air::new();
    listen(&mut air.bridge, READY_START | END_DISABLE);
    send_advertisement(&mut air.watch, READY_START | END_DISABLE);
    air.run_for_microseconds(5_000);
    assert!(air.largest_gap <= MAXIMUM_STEP, "gap {}", air.largest_gap);
    assert!(air.watch.now().abs_diff(air.bridge.now()) <= ACTIVE_LOOKAHEAD);
}

#[test]
fn steps_stay_short_while_a_radio_is_on() {
    assert_eq!(lookahead(1_000, true, [1_000, u64::MAX]), ACTIVE_LOOKAHEAD);
}

#[test]
fn idle_radios_allow_a_step_up_to_the_earliest_wake_plus_a_ramp_up() {
    let now = 10_000;
    assert_eq!(lookahead(now, false, [now, now]), IDLE_RADIO_LATENCY);
    assert_eq!(
        lookahead(now, false, [now + 3_200, u64::MAX]),
        3_200 + IDLE_RADIO_LATENCY
    );
    assert_eq!(lookahead(now, false, [u64::MAX, u64::MAX]), MAXIMUM_STEP);
}

#[test]
fn sleeping_machines_with_idle_radios_take_long_steps() {
    let mut air = Air::new();
    let mut steps = 0;
    let target = 100_000 * CYCLES_PER_MICROSECOND;
    run_in_lockstep(
        &mut air.watch,
        &air.watch_outbox,
        &mut air.bridge,
        &air.bridge_outbox,
        target,
        |_, _| steps += 1,
    );
    // A hundred milliseconds in no more steps than one per millisecond and a few for the wakes.
    assert!(steps <= 120, "{steps} steps");
    assert!(air.watch.now() >= target && air.bridge.now() >= target);
}

// ============================================================================
// Monitor commands
// ============================================================================

/// A file in the system's temporary folder, unique to this test run.
fn scratch_path(name: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("pinetime-bridge-test-{}-{name}", std::process::id()))
}

/// An ELF file with no segments, enough to start a controller that does nothing.
fn empty_controller_firmware(name: &str) -> std::path::PathBuf {
    let mut file = vec![0; 52];
    file[..6].copy_from_slice(&[0x7F, b'E', b'L', b'F', 1, 1]);
    let path = scratch_path(name);
    std::fs::write(&path, file).unwrap();
    path
}

#[test]
fn a_new_watch_keeps_the_bridge_at_the_same_link() {
    let link = scratch_path("mach-create-link");
    let controller = empty_controller_firmware("mach-create.elf");
    let mut monitor = Monitor::new();
    monitor
        .execute(&format!(
            "bluetooth Attach @{} @{}",
            link.display(),
            controller.display()
        ))
        .unwrap();
    monitor.execute("emulation RunFor \"0.001\"").unwrap();

    monitor.execute("mach create").unwrap();
    assert!(monitor.bluetooth.is_some());
    assert_eq!(monitor.bluetooth.as_ref().unwrap().machine.now(), 0);
    let terminal = std::fs::read_link(&link).unwrap();
    assert_eq!(
        monitor.execute("bluetooth Terminal").unwrap(),
        terminal.display().to_string()
    );

    monitor.execute("emulation RunFor \"0.002\"").unwrap();
    assert!(monitor.bluetooth.as_ref().unwrap().machine.now() >= monitor.machine.now());
    std::fs::remove_file(controller).unwrap();
}

#[test]
fn detaching_removes_the_link_and_a_new_watch_stays_without_a_bridge() {
    let link = scratch_path("detach-link");
    let controller = empty_controller_firmware("detach.elf");
    let mut monitor = Monitor::new();
    monitor
        .execute(&format!(
            "bluetooth Attach @{} @{}",
            link.display(),
            controller.display()
        ))
        .unwrap();
    assert!(monitor.execute("bluetooth Status").unwrap().starts_with("link "));

    monitor.execute("bluetooth Detach").unwrap();
    assert!(std::fs::symlink_metadata(&link).is_err());
    assert_eq!(monitor.execute("bluetooth Status").unwrap(), "detached");
    monitor.execute("mach create").unwrap();
    assert!(monitor.bluetooth.is_none());
    std::fs::remove_file(controller).unwrap();
}

#[test]
fn attaching_without_a_controller_file_fails() {
    let mut monitor = Monitor::new();
    let link = scratch_path("missing-controller-link");
    let missing = scratch_path("no-such-controller.elf");
    assert!(monitor
        .execute(&format!(
            "bluetooth Attach @{} @{}",
            link.display(),
            missing.display()
        ))
        .is_err());
    assert!(monitor.bluetooth.is_none());
    assert!(std::fs::symlink_metadata(&link).is_err());
}
