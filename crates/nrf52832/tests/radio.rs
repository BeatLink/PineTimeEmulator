// SPDX-License-Identifier: GPL-3.0-only
//! The radio, ported from tests/radio.robot: two chips share the air through test outboxes.

mod common;

use common::{microseconds, Harness, Outbox};
use pinetime_core::Cycle;

const RADIO: u32 = 0x4000_1000;
const TIMER: u32 = 0x4000_8000;
const PPI: u32 = 0x4001_F000;
const TX_BUFFER: u32 = 0x2000_1000;
const RX_BUFFER: u32 = 0x2000_2000;

const READY_START: u32 = 0x1;
const END_DISABLE: u32 = 0x2;
const DISABLED_TXEN: u32 = 0x4;
const DISABLED_RXEN: u32 = 0x8;
const ADDRESS_RSSISTART: u32 = 0x10;
const ADDRESS_BCSTART: u32 = 0x40;
const DISABLED_RSSISTOP: u32 = 0x100;
const LISTEN_SHORTCUTS: u32 =
    READY_START | END_DISABLE | ADDRESS_BCSTART | ADDRESS_RSSISTART | DISABLED_RSSISTOP;

// ============================================================================
// Two radios on one air
// ============================================================================

struct Air {
    sender: Harness,
    receiver: Harness,
    sender_outbox: Outbox,
    receiver_outbox: Outbox,
}

impl Air {
    fn new() -> Air {
        let sender_outbox = Outbox::default();
        let receiver_outbox = Outbox::default();
        let mut sender = Harness::new();
        let mut receiver = Harness::new();
        sender
            .chip
            .set_radio_medium(Some(Box::new(sender_outbox.clone())));
        receiver
            .chip
            .set_radio_medium(Some(Box::new(receiver_outbox.clone())));
        configure_radio(&mut sender);
        configure_radio(&mut receiver);
        Air {
            sender,
            receiver,
            sender_outbox,
            receiver_outbox,
        }
    }

    fn deliver(&mut self) {
        for frame in self.sender_outbox.take() {
            self.receiver
                .with_ports(|chip, ports| chip.receive_radio_frame(frame, ports));
        }
        for frame in self.receiver_outbox.take() {
            self.sender
                .with_ports(|chip, ports| chip.receive_radio_frame(frame, ports));
        }
    }

    /// Runs both chips in step, handing each frame to the other chip the moment it is sent.
    fn run_for(&mut self, cycles: Cycle) {
        let target = self.sender.now + cycles;
        self.deliver();
        loop {
            let next = [self.sender.next_deadline(), self.receiver.next_deadline()]
                .into_iter()
                .flatten()
                .filter(|&deadline| deadline <= target)
                .min();
            let Some(next) = next else {
                break;
            };
            self.sender.advance_to(next);
            self.receiver.advance_to(next);
            self.deliver();
        }
        self.sender.advance_to(target);
        self.receiver.advance_to(target);
    }

    fn run_for_microseconds(&mut self, value: f64) {
        self.run_for(microseconds(value));
    }
}

// Bluetooth LE advertising settings at 1 Mbit: S0, an 8-bit LENGTH and an S1 byte in RAM, the advertising access address, channel 37.
fn configure_radio(harness: &mut Harness) {
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
        harness.write(RADIO + offset, value);
    }
    harness.chip.take_event_trace();
}

// ADV_IND with a random address C0:DE:0A:1B:2C:3D and the flags 06.
fn write_advertisement(harness: &mut Harness) {
    harness.write_words(TX_BUFFER, &[0x3D00_0940, 0xDE0A_1B2C, 0x0601_02C0]);
    harness.write(RADIO + 0x504, TX_BUFFER);
}

fn send_advertisement(harness: &mut Harness, shortcuts: u32) {
    write_advertisement(harness);
    harness.write(RADIO + 0x200, shortcuts);
    harness.write(RADIO, 1);
}

fn listen(harness: &mut Harness, shortcuts: u32) {
    harness.write(RADIO + 0x504, RX_BUFFER);
    harness.write(RADIO + 0x560, 8);
    harness.write(RADIO + 0x200, shortcuts);
    harness.write(RADIO + 0x004, 1);
}

fn advertisement_should_be_in_the_receive_buffer(harness: &mut Harness) {
    assert_eq!(harness.read(RX_BUFFER), 0x3D00_0940);
    assert_eq!(harness.read(RX_BUFFER + 4), 0xDE0A_1B2C);
    assert_eq!(harness.read(RX_BUFFER + 8), 0x0601_02C0);
}

fn event_name(offset: u32) -> String {
    match offset {
        0x100 => "READY".into(),
        0x104 => "ADDRESS".into(),
        0x108 => "PAYLOAD".into(),
        0x10C => "END".into(),
        0x110 => "DISABLED".into(),
        0x114 => "DEVMATCH".into(),
        0x118 => "DEVMISS".into(),
        0x11C => "RSSIEND".into(),
        0x128 => "BCMATCH".into(),
        0x130 => "CRCOK".into(),
        0x134 => "CRCERROR".into(),
        _ => format!("0x{offset:X}"),
    }
}

/// The radio events traced so far, each with its time in microseconds, as the Renode recorder wrote them.
fn radio_events(harness: &mut Harness) -> String {
    harness
        .chip
        .take_event_trace()
        .into_iter()
        .filter(|(_, address)| address & !0xFFF == RADIO)
        .map(|(cycle, address)| {
            let time = format!("{:.3}", cycle as f64 / 64.0);
            let time = time.trim_end_matches('0').trim_end_matches('.');
            format!("{}@{}", event_name(address & 0xFFF), time)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

// ============================================================================
// Transmitting
// ============================================================================

#[test]
fn transmit_events_land_on_their_bit_times() {
    let mut air = Air::new();
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    // 40 us ramp-up, 8 preamble and 32 address bits, 16 header and 72 payload bits, 24 CRC bits, 6 us to disable.
    assert_eq!(
        radio_events(&mut air.sender),
        "READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198"
    );
    assert_eq!(air.sender.read(RADIO + 0x550), 0);
}

#[test]
fn normal_ramp_up_takes_140_microseconds() {
    let mut air = Air::new();
    air.sender.write(RADIO + 0x650, 0x200);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.sender),
        "READY@140 ADDRESS@180 PAYLOAD@268 END@292 DISABLED@298"
    );
}

#[test]
fn two_megabit_packets_take_half_the_time_with_a_longer_preamble() {
    let mut air = Air::new();
    air.sender.write(RADIO + 0x510, 4);
    air.sender.write(RADIO + 0x514, 0x0110_0108);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    // 16 preamble and 32 address bits at 0.5 us each, then 112 bits, and 4 us to disable.
    assert_eq!(
        radio_events(&mut air.sender),
        "READY@40 ADDRESS@64 PAYLOAD@108 END@120 DISABLED@124"
    );
}

#[test]
fn states_follow_the_packet() {
    let mut air = Air::new();
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    assert_eq!(air.sender.read(RADIO + 0x550), 9);
    air.run_for_microseconds(40.0);
    assert_eq!(air.sender.read(RADIO + 0x550), 11);
    air.run_for_microseconds(152.0);
    assert_eq!(air.sender.read(RADIO + 0x550), 12);
    air.run_for_microseconds(6.0);
    assert_eq!(air.sender.read(RADIO + 0x550), 0);
}

#[test]
fn transmits_without_a_medium() {
    let mut harness = Harness::new();
    configure_radio(&mut harness);
    send_advertisement(&mut harness, READY_START | END_DISABLE);
    harness.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut harness),
        "READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198"
    );
    assert_eq!(harness.chip.radio_transmitted_frames(), 1);
}

// ============================================================================
// Receiving
// ============================================================================

#[test]
fn receiver_raises_its_events_at_the_air_times() {
    let mut air = Air::new();
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    // The first bit is on air 4 us after START, the address ends 40 us later, and the receiver reports it 6 us after that.
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
    advertisement_should_be_in_the_receive_buffer(&mut air.receiver);
    assert_eq!(air.receiver.read(RADIO + 0x400), 1);
    assert_eq!(air.receiver.read(RADIO + 0x408), 0);
    assert_eq!(air.receiver.read(RADIO + 0x548), 40);
    assert_eq!(air.receiver.read(RADIO + 0x550), 0);
}

#[test]
fn receiver_answers_exactly_tifs_after_the_packet() {
    let mut air = Air::new();
    air.receiver.write_words(0x2000_3000, &[0x0000_0004, 0]);
    listen(&mut air.receiver, READY_START | END_DISABLE | DISABLED_TXEN);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE | DISABLED_RXEN);
    // Between DISABLED and START the receiver points at its answer and drops the turnaround shortcut, as firmware does.
    air.run_for_microseconds(250.0);
    air.receiver.write(RADIO + 0x504, 0x2000_3000);
    air.receiver.write(RADIO + 0x200, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    // The advertisement ends on air at 196 us; the answer starts on air at 346 us, so START is at 342 us.
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202 READY@342 ADDRESS@382 PAYLOAD@398 END@422 DISABLED@428"
    );
    assert_eq!(
        radio_events(&mut air.sender),
        "READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198 READY@238 ADDRESS@392 PAYLOAD@408 CRCOK@432 END@432 DISABLED@432 READY@472"
    );
}

#[test]
fn frame_is_lost_while_the_receiver_still_ramps_up() {
    let mut air = Air::new();
    // The access address ends on air at 84 us; this receiver is ready only at 85 us.
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(45.0);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.receiver), "READY@85");
    assert_eq!(air.receiver.read(RADIO + 0x550), 3);
}

#[test]
fn frame_is_taken_when_listening_starts_just_before_the_address_ends() {
    let mut air = Air::new();
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(43.0);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@83 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
    advertisement_should_be_in_the_receive_buffer(&mut air.receiver);
}

#[test]
fn frame_is_lost_when_the_receiver_is_not_listening() {
    let mut air = Air::new();
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.receiver), "");
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.receiver), "READY@1040");
    assert_eq!(air.receiver.read(RADIO + 0x550), 3);
}

#[test]
fn address_mismatch_drops_the_frame() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x51C, 0x1234_5600);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.receiver), "READY@40");
    assert_eq!(air.receiver.read(RADIO + 0x550), 3);
}

#[test]
fn second_logical_address_matches_and_shows_in_rxmatch() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x51C, 0x1234_5600);
    air.receiver.write(RADIO + 0x520, 0x89BE_D600);
    air.receiver.write(RADIO + 0x524, 0x8E00);
    air.receiver.write(RADIO + 0x530, 0x2);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
    assert_eq!(air.receiver.read(RADIO + 0x408), 1);
}

#[test]
fn another_channel_never_hears_the_frame() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x508, 26);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.receiver), "READY@40");
}

#[test]
fn device_address_match_after_the_advertiser_address() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x600, 0x0A1B_2C3D);
    air.receiver.write(RADIO + 0x620, 0xC0DE);
    // Logical address 0 enabled, with TxAdd set for a random address.
    air.receiver.write(RADIO + 0x640, 0x0101);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    // 16 header bits and 48 address bits after ADDRESS at 90 us.
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 DEVMATCH@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
    assert_eq!(air.receiver.read(RADIO + 0x410), 0);
}

#[test]
fn device_address_miss_on_a_different_address() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x604, 0x0A1B_2C3D);
    air.receiver.write(RADIO + 0x624, 0xC0DF);
    air.receiver.write(RADIO + 0x640, 0x0202);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 DEVMISS@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
}

#[test]
fn bit_counter_matches_again_after_bcc_moves_on() {
    let mut air = Air::new();
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(100.0);
    assert_eq!(air.receiver.read(RADIO + 0x128), 1);
    air.receiver.write(RADIO + 0x128, 0);
    air.receiver.write(RADIO + 0x560, 64);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 BCMATCH@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202"
    );
    assert_eq!(air.receiver.read(RADIO + 0x128), 1);
}

#[test]
fn different_crc_seed_gives_a_crc_error() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x53C, 0x12_3456);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut air.receiver),
        "READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCERROR@202 END@202 DISABLED@202"
    );
    assert_eq!(air.receiver.read(RADIO + 0x400), 0);
}

#[test]
fn stop_abandons_the_packet() {
    let mut air = Air::new();
    send_advertisement(&mut air.sender, READY_START);
    air.run_for_microseconds(100.0);
    air.sender.write(RADIO + 0x00C, 1);
    assert_eq!(air.sender.read(RADIO + 0x550), 10);
    air.run_for_microseconds(1000.0);
    assert_eq!(radio_events(&mut air.sender), "READY@40 ADDRESS@80");
}

#[test]
fn end_interrupt_follows_inten() {
    let mut air = Air::new();
    air.receiver.write(RADIO + 0x304, 0x8);
    listen(&mut air.receiver, LISTEN_SHORTCUTS);
    send_advertisement(&mut air.sender, READY_START | END_DISABLE);
    air.run_for_microseconds(1000.0);
    assert!(air.receiver.interrupt_line(1));
    air.receiver.write(RADIO + 0x10C, 0);
    assert!(!air.receiver.interrupt_line(1));
}

#[test]
fn unknown_registers_keep_their_value() {
    let mut harness = Harness::new();
    configure_radio(&mut harness);
    harness.write(RADIO + 0x774, 0x0100_0000);
    assert_eq!(harness.read(RADIO + 0x774), 0x0100_0000);
    assert_eq!(harness.read(RADIO + 0x554), 0x65);
}

#[test]
fn power_off_resets_the_radio() {
    let mut harness = Harness::new();
    configure_radio(&mut harness);
    harness.write(RADIO + 0xFFC, 0);
    assert_eq!(harness.read(RADIO + 0x510), 0);
    harness.write(RADIO, 1);
    assert_eq!(harness.read(RADIO + 0x550), 0);
    harness.write(RADIO + 0xFFC, 1);
    assert_eq!(harness.read(RADIO + 0xFFC), 1);
}

// PPI tasks run at the instant of their event, so the END capture is exact rather than at the next sync point.
#[test]
fn ppi_captures_radio_events_and_starts_the_radio_from_timer0() {
    let mut harness = Harness::new();
    configure_radio(&mut harness);
    write_advertisement(&mut harness);
    harness.write(RADIO + 0x200, READY_START | END_DISABLE);
    // TIMER0 at 1 MHz with CC[0] at 100 us; channels 20, 26 and 27 are TXEN, ADDRESS capture and END capture.
    harness.write(TIMER + 0x508, 3);
    harness.write(TIMER + 0x540, 100);
    harness.write(PPI + 0x504, 0x0C10_0000);
    harness.write(TIMER, 1);
    harness.run_for_microseconds(1000.0);
    assert_eq!(
        radio_events(&mut harness),
        "READY@140 ADDRESS@180 PAYLOAD@268 END@292 DISABLED@298"
    );
    assert_eq!(harness.read(TIMER + 0x544), 180);
    assert_eq!(harness.read(TIMER + 0x548), 292);
}

// Fixed channel 24 starts the CCM on READY before READY_START reads the packet, so the radio sends the encrypted packet.
#[test]
fn ppi_tasks_run_before_the_shortcut_their_event_triggers() {
    let mut harness = Harness::new();
    configure_radio(&mut harness);
    harness.write_words(0x2000_4000, &[0, 0, 0, 0, 0, 0, 0, 0, 0]);
    harness.write_words(0x2000_5000, &[0x0006_010F]);
    harness.write(0x4000_F500, 2);
    harness.write(0x4000_F504, 0);
    harness.write(0x4000_F508, 0x2000_4000);
    harness.write(0x4000_F50C, 0x2000_5000);
    harness.write(0x4000_F510, 0x2000_6000);
    harness.write(0x4000_F200, 1);
    harness.write(PPI + 0x504, 1 << 24);
    harness.write(RADIO + 0x504, 0x2000_6000);
    harness.write(RADIO + 0x200, READY_START | END_DISABLE);
    harness.write(RADIO, 1);
    harness.run_for_microseconds(41.0);
    assert_eq!(harness.read(0x4000_F104), 1);
    // The encrypted packet is five bytes long, so it lasts 32 bits longer on air than the plain one would.
    harness.run_for_microseconds(1000.0);
    let events = radio_events(&mut harness);
    assert!(events.starts_with("READY@40 ADDRESS@80 PAYLOAD@136"), "{events}");
}
