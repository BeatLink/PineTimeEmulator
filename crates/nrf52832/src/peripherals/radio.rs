// SPDX-License-Identifier: GPL-3.0-only
//! The 2.4 GHz radio: ramp-up, packets sent and received at their bit times, shortcuts, TIFS
//! turnaround, and address and device matching.
//!
//! Packets go to an optional `RadioMedium`; with none attached they go nowhere, which is enough
//! for a Bluetooth LE stack to advertise. Shortcuts run as work due at the same instant, so the
//! chip runs an event's PPI tasks before the shortcut it triggers.

mod packet;

use pinetime_core::{Cycle, EventKey};

use self::packet::{compute_crc, logical_address, Header, PacketFormat};
use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::{event_key, Context};

// ============================================================================
// Medium
// ============================================================================

/// What a sender of this model knows about its packet, beyond the bytes on air.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SenderTiming {
    /// The cycle the first preamble bit went on air.
    pub air_start: Cycle,
    pub bit_cycles: u64,
    pub preamble_bits: u64,
    pub mode: u32,
    pub crc: u32,
    /// The whitening seed, or -1 with whitening off.
    pub whitening: i32,
}

/// One packet on air: access address, header and payload, on a Bluetooth LE channel index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RadioFrame {
    pub bytes: Vec<u8>,
    pub channel: u32,
    /// `None` for a frame from another kind of radio, which counts as having its address the moment it arrives.
    pub sender: Option<SenderTiming>,
}

/// Where transmitted packets go; a bridge to other radios implements it.
pub trait RadioMedium: Send {
    fn transmit(&mut self, frame: RadioFrame);
}

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_1000;
pub(crate) const ID: u8 = 0x01;

const TASKS_TXEN: u32 = 0x000;
const TASKS_RXEN: u32 = 0x004;
const TASKS_START: u32 = 0x008;
const TASKS_STOP: u32 = 0x00C;
const TASKS_DISABLE: u32 = 0x010;
const TASKS_RSSISTART: u32 = 0x014;
const TASKS_RSSISTOP: u32 = 0x018;
const TASKS_BCSTART: u32 = 0x01C;
const TASKS_BCSTOP: u32 = 0x020;
const SHORTS: u32 = 0x200;
const CRCSTATUS: u32 = 0x400;
const RXMATCH: u32 = 0x408;
const RXCRC: u32 = 0x40C;
const DAI: u32 = 0x410;
const PACKETPTR: u32 = 0x504;
const FREQUENCY: u32 = 0x508;
const MODE: u32 = 0x510;
const PCNF0: u32 = 0x514;
const PCNF1: u32 = 0x518;
const BASE0: u32 = 0x51C;
const BASE1: u32 = 0x520;
const PREFIX0: u32 = 0x524;
const PREFIX1: u32 = 0x528;
const TXADDRESS: u32 = 0x52C;
const RXADDRESSES: u32 = 0x530;
const CRCCNF: u32 = 0x534;
const CRCPOLY: u32 = 0x538;
const CRCINIT: u32 = 0x53C;
const TIFS: u32 = 0x544;
const RSSISAMPLE: u32 = 0x548;
const STATE: u32 = 0x550;
const DATAWHITEIV: u32 = 0x554;
const BCC: u32 = 0x560;
const DAB: u32 = 0x600;
const DAP: u32 = 0x620;
const DACNF: u32 = 0x640;
const MODECNF0: u32 = 0x650;
const POWER: u32 = 0xFFC;

const EVENT_READY: u32 = 0;
const EVENT_ADDRESS: u32 = 1;
const EVENT_PAYLOAD: u32 = 2;
const EVENT_END: u32 = 3;
const EVENT_DISABLED: u32 = 4;
const EVENT_DEVMATCH: u32 = 5;
const EVENT_DEVMISS: u32 = 6;
const EVENT_RSSIEND: u32 = 7;
const EVENT_BCMATCH: u32 = 10;
const EVENT_CRCOK: u32 = 12;
const EVENT_CRCERROR: u32 = 13;
const VALID_EVENTS: u32 = 0x34FF;

const SHORT_READY_START: u32 = 1 << 0;
const SHORT_END_DISABLE: u32 = 1 << 1;
const SHORT_DISABLED_TXEN: u32 = 1 << 2;
const SHORT_DISABLED_RXEN: u32 = 1 << 3;
const SHORT_ADDRESS_RSSISTART: u32 = 1 << 4;
const SHORT_END_START: u32 = 1 << 5;
const SHORT_ADDRESS_BCSTART: u32 = 1 << 6;
const SHORT_DISABLED_RSSISTOP: u32 = 1 << 8;
const VALID_SHORTCUTS: u32 = 0x17F;

const MODE_NRF_2MBIT: u32 = 1;
const MODE_NRF_250KBIT: u32 = 2;
const MODE_BLE_2MBIT: u32 = 4;

const STATE_DISABLED: u32 = 0;
const STATE_RX_RAMP_UP: u32 = 1;
const STATE_RX_IDLE: u32 = 2;
const STATE_RX: u32 = 3;
const STATE_RX_DISABLE: u32 = 4;
const STATE_TX_RAMP_UP: u32 = 9;
const STATE_TX_IDLE: u32 = 10;
const STATE_TX: u32 = 11;
const STATE_TX_DISABLE: u32 = 12;

const LOGICAL_ADDRESS_COUNT: usize = 8;
const DEVICE_ADDRESS_BYTES: u64 = 6;
const NORMAL_BAND_START: u32 = 2400;
const LOW_BAND_START: u32 = 2360;
const NON_BLUETOOTH_CHANNEL_BASE: u32 = 1000;
/// The receiver reports -40 dBm for every packet.
const RSSI_SAMPLE_MAGNITUDE: u32 = 40;
const MAXIMUM_DELIVERY_DELAY: Cycle = 64_000;

// Times in 64 MHz cycles.
const CYCLES_PER_MICROSECOND: u64 = 64;
const FAST_RAMP_UP: Cycle = 40 * CYCLES_PER_MICROSECOND;
const NORMAL_RAMP_UP: Cycle = 140 * CYCLES_PER_MICROSECOND;
const RSSI_SAMPLE: Cycle = 16;

const ALARM_TOKEN: u32 = 0;

static LAYOUT: [PlainRegister; 36] = {
    let mut layout = [const { plain(0, 0, 0) }; 36];
    let fixed = [
        plain(0x504, 0, u32::MAX),
        plain(0x508, 0x2, 0x17F),
        plain(0x50C, 0, 0xFF),
        plain(0x510, 0, 0xF),
        plain(0x514, 0, 0x011F_010F),
        plain(0x518, 0, 0x0307_FFFF),
        plain(0x52C, 0, 0x7),
        plain(0x530, 0, 0xFF),
        plain(0x534, 0, 0x103),
        plain(0x538, 0, 0xFF_FFFF),
        plain(0x53C, 0, 0xFF_FFFF),
        plain(0x544, 0, 0xFF),
        plain(0x640, 0, 0xFFFF),
        plain(0x650, 0x200, 0x301),
        plain(0x560, 0, u32::MAX),
        plain(0x520, 0, u32::MAX),
        plain(0x524, 0, u32::MAX),
        plain(0x528, 0, u32::MAX),
        plain(0x51C, 0, u32::MAX),
        plain(0x54C, 0, u32::MAX),
    ];
    let mut index = 0;
    while index < fixed.len() {
        layout[index] = plain(fixed[index].offset, fixed[index].reset, fixed[index].mask);
        index += 1;
    }
    let mut address = 0;
    while address < LOGICAL_ADDRESS_COUNT {
        layout[20 + address] = plain((DAB + 4 * address as u32) as u16, 0, u32::MAX);
        layout[28 + address] = plain((DAP + 4 * address as u32) as u16, 0, 0xFFFF);
        address += 1;
    }
    layout
};

// ============================================================================
// Scheduled work
// ============================================================================

/// Kinds of work, so a state change can cancel all of one kind.
const WORK_RAMP_UP: u8 = 1;
const WORK_PACKET: u8 = 2;
const WORK_DISABLING: u8 = 4;
const WORK_RSSI: u8 = 8;
const WORK_BIT_COUNTER: u8 = 16;
const WORK_ARRIVAL: u8 = 32;
const WORK_SHORTCUT: u8 = 64;

enum Action {
    ReadyReached,
    Start,
    TransmitEnable { after_end: bool },
    ReceiveEnable,
    DisableAfterEnd,
    RssiStart,
    RssiStop,
    BitCounterStart,
    TransmitAddress,
    Payload,
    TransmitEnd,
    ReceiveAddress,
    DeviceAddress,
    ReceiveEnd,
    DisabledReached,
    RssiEnd,
    BitCounterMatch,
    Arrival(Box<RadioFrame>),
}

struct Work {
    time: Cycle,
    order: u64,
    kind: u8,
    action: Action,
}

/// When each event of one packet happens, counted from its ADDRESS event.
#[derive(Clone, Copy)]
struct PacketTimeline {
    transmitting: bool,
    address_time: Cycle,
    header_bits: u64,
    payload_time: Cycle,
    end_time: Cycle,
}

impl PacketTimeline {
    fn new(
        transmitting: bool,
        address_time: Cycle,
        bit_cycles: u64,
        header_bits: u64,
        payload_bytes: usize,
        crc_bits: u64,
    ) -> PacketTimeline {
        let payload_time = address_time + (header_bits + payload_bytes as u64 * 8) * bit_cycles;
        PacketTimeline {
            transmitting,
            address_time,
            header_bits,
            payload_time,
            end_time: payload_time + crc_bits * bit_cycles,
        }
    }
}

/// A packet being received, written to RAM at its ADDRESS event.
struct Reception {
    address_index: u32,
    header: Header,
    payload: Vec<u8>,
    crc: u32,
    crc_good: bool,
}

// ============================================================================
// RADIO
// ============================================================================

pub(crate) struct Radio {
    events: EventRegisters,
    plain: PlainRegisters,
    medium: Option<Box<dyn RadioMedium>>,
    work: Vec<Work>,
    next_order: u64,
    alarm: Option<Cycle>,
    state: u32,
    powered: bool,
    shortcuts: u32,
    current_packet: Option<PacketTimeline>,
    current_bit_cycles: u64,
    receive_pointer: u32,
    reception: Option<Reception>,
    bit_counter_start: Option<Cycle>,
    crc_matched: bool,
    received_address_index: u32,
    received_crc: u32,
    device_address_index: u32,
    last_air_end: Cycle,
    turnaround_after_end: bool,
    whitening_seed: u32,
    transmitted_frames: u64,
}

impl Radio {
    pub fn new() -> Radio {
        Radio {
            events: EventRegisters::new(BASE, VALID_EVENTS),
            plain: PlainRegisters::new(&LAYOUT),
            medium: None,
            work: Vec::new(),
            next_order: 0,
            alarm: None,
            state: STATE_DISABLED,
            powered: true,
            shortcuts: 0,
            current_packet: None,
            current_bit_cycles: 64,
            receive_pointer: 0,
            reception: None,
            bit_counter_start: None,
            crc_matched: false,
            received_address_index: 0,
            received_crc: 0,
            device_address_index: 0,
            last_air_end: 0,
            turnaround_after_end: false,
            whitening_seed: 0x40,
            transmitted_frames: 0,
        }
    }

    /// Resets the radio; an attached medium stays attached.
    pub fn reset(&mut self, context: &mut Context) {
        let medium = self.medium.take();
        let transmitted_frames = self.transmitted_frames;
        *self = Radio::new();
        self.medium = medium;
        self.transmitted_frames = transmitted_frames;
        context.cancel(alarm_key());
    }

    pub fn set_medium(&mut self, medium: Option<Box<dyn RadioMedium>>) {
        self.medium = medium;
    }

    pub fn transmitted_frames(&self) -> u64 {
        self.transmitted_frames
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    // ========================================================================
    // Configuration
    // ========================================================================

    fn mode(&self) -> u32 {
        self.plain.read(MODE)
    }

    fn is_two_megabit(&self) -> bool {
        matches!(self.mode(), MODE_NRF_2MBIT | MODE_BLE_2MBIT)
    }

    fn bit_cycles(&self) -> u64 {
        match self.mode() {
            MODE_NRF_2MBIT | MODE_BLE_2MBIT => 32,
            MODE_NRF_250KBIT => 256,
            _ => 64,
        }
    }

    // Bluetooth LE 2M always sends a two-byte preamble; otherwise PLEN chooses.
    fn preamble_bits(&self) -> u64 {
        if self.mode() == MODE_BLE_2MBIT || self.plain.read(PCNF0) & (1 << 24) != 0 {
            16
        } else {
            8
        }
    }

    fn format(&self) -> PacketFormat {
        PacketFormat::new(
            self.plain.read(PCNF0),
            self.plain.read(PCNF1),
            self.plain.read(CRCCNF),
        )
    }

    // Fast ramp-up takes 40 us, the default 140 us, for either direction.
    fn ramp_up(&self) -> Cycle {
        if self.plain.read(MODECNF0) & 1 != 0 {
            FAST_RAMP_UP
        } else {
            NORMAL_RAMP_UP
        }
    }

    // Measured pipeline delays, as Apache NimBLE's nRF52 driver lists them: the air lags the TX events and the RX events lag the air.
    fn transmit_chain(&self) -> Cycle {
        (if self.is_two_megabit() { 3 } else { 4 }) * CYCLES_PER_MICROSECOND
    }

    fn receive_chain(&self) -> Cycle {
        (if self.is_two_megabit() { 2 } else { 6 }) * CYCLES_PER_MICROSECOND
    }

    fn transmit_disable(&self) -> Cycle {
        (if self.is_two_megabit() { 4 } else { 6 }) * CYCLES_PER_MICROSECOND
    }

    /// The Bluetooth LE channel index FREQUENCY points at; other frequencies get numbers no channel uses.
    pub fn channel(&self) -> u32 {
        let frequency = self.plain.read(FREQUENCY);
        let band_start = if frequency & (1 << 8) != 0 {
            LOW_BAND_START
        } else {
            NORMAL_BAND_START
        };
        let megahertz = band_start + (frequency & 0x7F);
        match megahertz {
            2402 => 37,
            2426 => 38,
            2480 => 39,
            2404..=2424 if megahertz.is_multiple_of(2) => (megahertz - 2404) / 2,
            2428..=2478 if megahertz.is_multiple_of(2) => (megahertz - 2428) / 2 + 11,
            _ => NON_BLUETOOTH_CHANNEL_BASE + megahertz,
        }
    }

    fn logical_address(&self, format: &PacketFormat, index: usize) -> Vec<u8> {
        let base = if index == 0 {
            self.plain.read(BASE0)
        } else {
            self.plain.read(BASE1)
        };
        let prefixes = if index < 4 {
            self.plain.read(PREFIX0)
        } else {
            self.plain.read(PREFIX1)
        };
        logical_address(format, base, (prefixes >> (8 * (index % 4))) as u8)
    }

    // Data whitening only has to agree between the two ends, so its seed stands in for the whitened bits.
    fn whitening_key(&self) -> i32 {
        if self.plain.read(PCNF1) & (1 << 25) != 0 {
            (self.whitening_seed & 0x3F) as i32
        } else {
            -1
        }
    }

    // ========================================================================
    // Work queue
    // ========================================================================

    fn schedule(&mut self, time: Cycle, kind: u8, action: Action) {
        self.work.push(Work {
            time,
            order: self.next_order,
            kind,
            action,
        });
        self.next_order += 1;
    }

    fn cancel(&mut self, kinds: u8) {
        self.work.retain(|work| work.kind & kinds == 0);
    }

    fn next_due(&self, now: Cycle) -> Option<usize> {
        self.work
            .iter()
            .enumerate()
            .filter(|(_, work)| work.time <= now)
            .min_by_key(|(_, work)| (work.time, work.order))
            .map(|(index, _)| index)
    }

    pub fn has_due_work(&self, now: Cycle) -> bool {
        self.work.iter().any(|work| work.time <= now)
    }

    /// Runs the earliest piece of work due by now, at its own time; says whether there was one.
    pub fn run_one_due_work(&mut self, context: &mut Context) -> bool {
        let Some(index) = self.next_due(context.now) else {
            return false;
        };
        let work = self.work.swap_remove(index);
        let now = context.now;
        context.now = work.time;
        self.run(work.action, context);
        context.now = now;
        true
    }

    /// Points the scheduler at the earliest queued work.
    pub fn arm_alarm(&mut self, context: &mut Context) {
        let earliest = self.work.iter().map(|work| work.time).min();
        if earliest == self.alarm {
            return;
        }
        self.alarm = earliest;
        match earliest {
            Some(time) => context.schedule(alarm_key(), time),
            None => context.cancel(alarm_key()),
        }
    }

    /// The alarm fired; the chip runs the due work.
    pub fn alarm_fired(&mut self) {
        self.alarm = None;
    }

    fn run(&mut self, action: Action, context: &mut Context) {
        let time = context.now;
        match action {
            Action::ReadyReached => self.ready_reached(context),
            Action::Start => self.start(time, context),
            Action::TransmitEnable { after_end } => self.transmit_enable(time, after_end),
            Action::ReceiveEnable => self.receive_enable(time),
            Action::DisableAfterEnd => self.disable(time, true, context),
            Action::RssiStart => self.rssi_start(time),
            Action::RssiStop => self.cancel(WORK_RSSI),
            Action::BitCounterStart => self.bit_counter_start(time),
            Action::TransmitAddress => self.address_reached(context),
            Action::Payload => self.events.raise(EVENT_PAYLOAD, context),
            Action::TransmitEnd | Action::ReceiveEnd => self.end_reached(time, context),
            Action::ReceiveAddress => self.received_address(context),
            Action::DeviceAddress => self.match_device_address(context),
            Action::DisabledReached => self.disabled_reached(time, context),
            Action::RssiEnd => self.events.raise(EVENT_RSSIEND, context),
            Action::BitCounterMatch => self.events.raise(EVENT_BCMATCH, context),
            Action::Arrival(frame) => self.try_receive(*frame, time),
        }
    }

    fn shortcut(&mut self, time: Cycle, action: Action) {
        self.schedule(time, WORK_SHORTCUT, action);
    }

    // ========================================================================
    // Tasks and state changes
    // ========================================================================

    // TXEN and RXEN ramp up from DISABLED or from either idle state.
    fn can_ramp_up(&self) -> bool {
        matches!(self.state, STATE_DISABLED | STATE_TX_IDLE | STATE_RX_IDLE)
    }

    // After END_DISABLE and DISABLED_TXEN the first bit goes on air exactly TIFS after the last packet ended.
    fn transmit_enable(&mut self, time: Cycle, after_end: bool) {
        if !self.can_ramp_up() {
            return;
        }
        let mut ready_time = time + self.ramp_up();
        if after_end {
            let on_air = self.last_air_end + self.plain.read(TIFS) as u64 * CYCLES_PER_MICROSECOND;
            ready_time = ready_time.max(on_air.saturating_sub(self.transmit_chain()));
        }
        self.state = STATE_TX_RAMP_UP;
        self.schedule(ready_time, WORK_RAMP_UP, Action::ReadyReached);
    }

    fn receive_enable(&mut self, time: Cycle) {
        self.turnaround_after_end = false;
        if !self.can_ramp_up() {
            return;
        }
        self.state = STATE_RX_RAMP_UP;
        self.schedule(time + self.ramp_up(), WORK_RAMP_UP, Action::ReadyReached);
    }

    fn ready_reached(&mut self, context: &mut Context) {
        self.state = if self.state == STATE_TX_RAMP_UP {
            STATE_TX_IDLE
        } else {
            STATE_RX_IDLE
        };
        self.events.raise(EVENT_READY, context);
        if self.shortcuts & SHORT_READY_START != 0 {
            self.shortcut(context.now, Action::Start);
        }
    }

    fn start(&mut self, time: Cycle, context: &mut Context) {
        match self.state {
            STATE_TX_IDLE => self.transmit(time, context),
            // The receiver listens for one packet into the buffer PACKETPTR names now.
            STATE_RX_IDLE => {
                self.state = STATE_RX;
                self.receive_pointer = self.plain.read(PACKETPTR);
            }
            _ => {}
        }
    }

    fn stop(&mut self) {
        if self.state != STATE_TX && self.state != STATE_RX {
            return;
        }
        self.cancel(WORK_PACKET | WORK_BIT_COUNTER);
        self.current_packet = None;
        self.reception = None;
        self.bit_counter_start = None;
        self.state = if self.state == STATE_TX {
            STATE_TX_IDLE
        } else {
            STATE_RX_IDLE
        };
    }

    fn disable(&mut self, time: Cycle, after_end: bool, context: &mut Context) {
        if self.state == STATE_TX_DISABLE || self.state == STATE_RX_DISABLE {
            return;
        }
        self.cancel(WORK_RAMP_UP | WORK_PACKET | WORK_BIT_COUNTER);
        self.current_packet = None;
        self.reception = None;
        self.bit_counter_start = None;
        self.turnaround_after_end = after_end;
        match self.state {
            STATE_TX_RAMP_UP | STATE_TX_IDLE | STATE_TX => {
                self.state = STATE_TX_DISABLE;
                self.schedule(
                    time + self.transmit_disable(),
                    WORK_DISABLING,
                    Action::DisabledReached,
                );
            }
            // The receiver turns off at once, and DISABLE while already disabled still raises DISABLED.
            _ => {
                self.state = STATE_RX_DISABLE;
                self.disabled_reached(time, context);
            }
        }
    }

    fn disabled_reached(&mut self, time: Cycle, context: &mut Context) {
        self.state = STATE_DISABLED;
        self.events.raise(EVENT_DISABLED, context);
        if self.shortcuts & SHORT_DISABLED_RSSISTOP != 0 {
            self.shortcut(time, Action::RssiStop);
        }
        if self.shortcuts & SHORT_DISABLED_TXEN != 0 {
            let after_end = self.turnaround_after_end;
            self.shortcut(time, Action::TransmitEnable { after_end });
        } else if self.shortcuts & SHORT_DISABLED_RXEN != 0 {
            self.shortcut(time, Action::ReceiveEnable);
        }
        self.turnaround_after_end = false;
    }

    // One RSSI sample takes a quarter of a microsecond.
    fn rssi_start(&mut self, time: Cycle) {
        self.cancel(WORK_RSSI);
        self.schedule(time + RSSI_SAMPLE, WORK_RSSI, Action::RssiEnd);
    }

    // The bit counter counts bits on air from the moment it starts, normally the ADDRESS event.
    fn bit_counter_start(&mut self, time: Cycle) {
        self.bit_counter_start = Some(time);
        self.schedule_bit_counter_match(time);
    }

    fn schedule_bit_counter_match(&mut self, now: Cycle) {
        self.cancel(WORK_BIT_COUNTER);
        let Some(start) = self.bit_counter_start else {
            return;
        };
        let match_time = start + self.plain.read(BCC) as u64 * self.current_bit_cycles;
        let after_packet = self
            .current_packet
            .is_some_and(|packet| match_time > packet.end_time);
        if match_time >= now && !after_packet {
            self.schedule(match_time, WORK_BIT_COUNTER, Action::BitCounterMatch);
        }
    }

    // ========================================================================
    // Transmitting
    // ========================================================================

    // START in TXIDLE reads the packet from RAM and puts it on the medium at once; its events follow at their bit times.
    fn transmit(&mut self, time: Cycle, context: &mut Context) {
        let format = self.format();
        let pointer = self.plain.read(PACKETPTR);
        let ram_header = context.memory.dma_read_vec(pointer, format.ram_header_bytes());
        let header = Header::from_ram(&format, &ram_header);
        let payload_length = format.payload_length(&header);
        let payload = context.memory.dma_read_vec(
            pointer.wrapping_add(format.ram_header_bytes() as u32),
            payload_length,
        );
        let air_header = header.to_air(&format);
        let address = self.logical_address(&format, self.plain.read(TXADDRESS) as usize);

        let bit_cycles = self.bit_cycles();
        let preamble_bits = self.preamble_bits();
        let address_time = time + (preamble_bits + address.len() as u64 * 8) * bit_cycles;
        let packet = PacketTimeline::new(
            true,
            address_time,
            bit_cycles,
            format.header_air_bits(),
            payload_length,
            format.crc_bits(),
        );
        self.current_packet = Some(packet);
        self.current_bit_cycles = bit_cycles;
        self.state = STATE_TX;

        self.schedule(packet.address_time, WORK_PACKET, Action::TransmitAddress);
        self.schedule(packet.payload_time, WORK_PACKET, Action::Payload);
        self.schedule(packet.end_time, WORK_PACKET, Action::TransmitEnd);

        self.transmitted_frames += 1;
        if self.medium.is_none() {
            return;
        }
        let crc = compute_crc(
            &format,
            self.plain.read(CRCINIT),
            self.plain.read(CRCPOLY),
            &address,
            &air_header,
            &payload,
        );
        let frame = RadioFrame {
            bytes: [address, air_header, payload].concat(),
            channel: self.channel(),
            sender: Some(SenderTiming {
                air_start: time + self.transmit_chain(),
                bit_cycles,
                preamble_bits,
                mode: self.mode(),
                crc,
                whitening: self.whitening_key(),
            }),
        };
        if let Some(medium) = &mut self.medium {
            medium.transmit(frame);
        }
    }

    // ========================================================================
    // Receiving
    // ========================================================================

    /// A frame starts arriving; the receiver decides when its whole access address has come in.
    pub fn frame_arrives(&mut self, frame: RadioFrame, now: Cycle) {
        let address_end = match frame.sender {
            Some(sender) => {
                // The frame may be handed over a little after its sender started it.
                let air_start = if sender.air_start.abs_diff(now) > MAXIMUM_DELIVERY_DELAY {
                    now
                } else {
                    sender.air_start
                };
                air_start
                    + (sender.preamble_bits + self.format().address_bytes() as u64 * 8) * sender.bit_cycles
            }
            None => now,
        };
        self.schedule(
            address_end.max(now),
            WORK_ARRIVAL,
            Action::Arrival(Box::new(frame)),
        );
    }

    // The receiver takes a frame only if it is listening, on its channel, at its rate, when the whole access address has arrived.
    fn try_receive(&mut self, frame: RadioFrame, time: Cycle) {
        if self.state != STATE_RX || self.current_packet.is_some() || frame.channel != self.channel() {
            return;
        }
        if frame.sender.is_some_and(|sender| sender.mode != self.mode()) {
            return;
        }
        let format = self.format();
        let address_length = format.address_bytes();
        if frame.bytes.len() < address_length {
            return;
        }
        let received_address = &frame.bytes[..address_length];
        let receive_addresses = self.plain.read(RXADDRESSES);
        let Some(matched) = (0..LOGICAL_ADDRESS_COUNT).find(|&index| {
            receive_addresses & (1 << index) != 0 && self.logical_address(&format, index) == received_address
        }) else {
            return;
        };

        let take = |start: usize, count: usize| -> Vec<u8> {
            (start..start + count)
                .map(|index| frame.bytes.get(index).copied().unwrap_or(0))
                .collect()
        };
        let air_header = take(address_length, format.air_header_bytes());
        let header = Header::from_air(&format, &air_header);
        let announced_length = (header.length + format.static_length as u64) as usize;
        let payload_length = format.payload_length(&header);
        let payload = take(address_length + air_header.len(), payload_length);

        let crc = compute_crc(
            &format,
            self.plain.read(CRCINIT),
            self.plain.read(CRCPOLY),
            received_address,
            &air_header,
            &payload,
        );
        let mut crc_good = announced_length == payload_length;
        if let Some(sender) = frame.sender {
            crc_good &= crc == sender.crc && self.whitening_key() == sender.whitening;
        }

        let bit_cycles = self.bit_cycles();
        let packet = PacketTimeline::new(
            false,
            time + self.receive_chain(),
            bit_cycles,
            format.header_air_bits(),
            payload_length,
            format.crc_bits(),
        );
        self.current_packet = Some(packet);
        self.current_bit_cycles = bit_cycles;
        self.reception = Some(Reception {
            address_index: matched as u32,
            header,
            payload,
            crc,
            crc_good,
        });

        self.schedule(packet.address_time, WORK_PACKET, Action::ReceiveAddress);
        if self.plain.read(DACNF) & 0xFF != 0 && payload_length as u64 >= DEVICE_ADDRESS_BYTES {
            let device_address_time =
                packet.address_time + (packet.header_bits + DEVICE_ADDRESS_BYTES * 8) * bit_cycles;
            self.schedule(device_address_time, WORK_PACKET, Action::DeviceAddress);
        }
        self.schedule(packet.payload_time, WORK_PACKET, Action::Payload);
        self.schedule(packet.end_time, WORK_PACKET, Action::ReceiveEnd);
    }

    // The whole packet lands in RAM by the ADDRESS event, so CCM and AAR started from it through PPI see real data.
    fn received_address(&mut self, context: &mut Context) {
        let format = self.format();
        if let Some(reception) = &self.reception {
            let header = reception.header.to_ram(&format);
            context.memory.dma_write(self.receive_pointer, &header);
            context.memory.dma_write(
                self.receive_pointer.wrapping_add(header.len() as u32),
                &reception.payload,
            );
            self.received_address_index = reception.address_index;
        }
        self.address_reached(context);
    }

    fn address_reached(&mut self, context: &mut Context) {
        self.events.raise(EVENT_ADDRESS, context);
        if self.shortcuts & SHORT_ADDRESS_RSSISTART != 0 {
            self.shortcut(context.now, Action::RssiStart);
        }
        if self.shortcuts & SHORT_ADDRESS_BCSTART != 0 {
            self.shortcut(context.now, Action::BitCounterStart);
        }
    }

    fn end_reached(&mut self, time: Cycle, context: &mut Context) {
        let Some(packet) = self.current_packet.take() else {
            return;
        };
        self.bit_counter_start = None;
        self.cancel(WORK_BIT_COUNTER);
        if packet.transmitting {
            self.last_air_end = time + self.transmit_chain();
            self.state = STATE_TX_IDLE;
        } else {
            self.last_air_end = time.saturating_sub(self.receive_chain());
            self.state = STATE_RX_IDLE;
            if let Some(reception) = self.reception.take() {
                self.crc_matched = reception.crc_good;
                self.received_crc = reception.crc;
            }
            self.events.raise(
                if self.crc_matched {
                    EVENT_CRCOK
                } else {
                    EVENT_CRCERROR
                },
                context,
            );
        }
        self.events.raise(EVENT_END, context);
        if self.shortcuts & SHORT_END_DISABLE != 0 {
            self.shortcut(time, Action::DisableAfterEnd);
        } else if self.shortcuts & SHORT_END_START != 0 {
            self.shortcut(time, Action::Start);
        }
    }

    // Compares the advertiser address after the header with each enabled DAB, DAP and TxAdd.
    fn match_device_address(&mut self, context: &mut Context) {
        let Some(reception) = &self.reception else {
            return;
        };
        let payload = &reception.payload;
        let low = u32::from_le_bytes([payload[0], payload[1], payload[2], payload[3]]);
        let high = u16::from_le_bytes([payload[4], payload[5]]) as u32;
        let random = reception.header.transmit_address_random();
        let configuration = self.plain.read(DACNF);
        let matched = (0..LOGICAL_ADDRESS_COUNT as u32).find(|&index| {
            let enabled = configuration & (1 << index) != 0;
            let wants_random = configuration & (1 << (index + 8)) != 0;
            enabled
                && wants_random == random
                && self.plain.read(DAB + 4 * index) == low
                && self.plain.read(DAP + 4 * index) == high
        });
        match matched {
            Some(index) => {
                self.device_address_index = index;
                self.events.raise(EVENT_DEVMATCH, context);
            }
            None => self.events.raise(EVENT_DEVMISS, context),
        }
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            SHORTS => self.shortcuts,
            CRCSTATUS => self.crc_matched as u32,
            RXMATCH => self.received_address_index,
            RXCRC => self.received_crc,
            DAI => self.device_address_index,
            RSSISAMPLE => RSSI_SAMPLE_MAGNITUDE,
            STATE => self.state,
            // Bit 6 of DATAWHITEIV always reads one.
            DATAWHITEIV => self.whitening_seed | 0x40,
            POWER => self.powered as u32,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        let now = context.now;
        match offset {
            0x000..0x100 if !triggers(value) || !self.powered => {}
            TASKS_TXEN => self.transmit_enable(now, false),
            TASKS_RXEN => self.receive_enable(now),
            TASKS_START => self.start(now, context),
            TASKS_STOP => self.stop(),
            TASKS_DISABLE => self.disable(now, false, context),
            TASKS_RSSISTART => self.rssi_start(now),
            TASKS_RSSISTOP => self.cancel(WORK_RSSI),
            TASKS_BCSTART => self.bit_counter_start(now),
            TASKS_BCSTOP => {
                self.bit_counter_start = None;
                self.cancel(WORK_BIT_COUNTER);
            }
            0x024..0x100 => {}
            SHORTS => self.shortcuts = value & VALID_SHORTCUTS,
            CRCSTATUS | RXMATCH | RXCRC | DAI | RSSISAMPLE | STATE => {}
            DATAWHITEIV => self.whitening_seed = value & 0x7F,
            BCC => {
                self.plain.write(BCC, value);
                self.schedule_bit_counter_match(now);
            }
            // Turning POWER off and on resets the radio, as firmware does at start-up.
            POWER => {
                if value & 1 == 0 && self.powered {
                    self.reset(context);
                    self.powered = false;
                } else if value & 1 != 0 {
                    self.powered = true;
                }
            }
            _ => self.plain.write(offset, value),
        }
    }
}

fn alarm_key() -> EventKey {
    event_key(ID, ALARM_TOKEN)
}
