// SPDX-License-Identifier: GPL-3.0-only
//! One shared serial block, which acts as SPI, SPIM, TWI or TWIM depending on its ENABLE value.
//!
//! Bytes reach the board when a transfer starts; the EasyDMA transfers raise their END events
//! once the bytes would have crossed the bus, with one scheduled event per transfer.

use pinetime_core::Cycle;

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::{event_key, peripheral_id, Context};

// ============================================================================
// Registers
// ============================================================================

const TASKS_STARTRX: u32 = 0x000;
const TASKS_STARTTX: u32 = 0x008;
const TASKS_START: u32 = 0x010;
const TASKS_STOP: u32 = 0x014;
const TASKS_SUSPEND: u32 = 0x01C;
const TASKS_RESUME: u32 = 0x020;
const SHORTS: u32 = 0x200;
const ERRORSRC: u32 = 0x4C4;
const ENABLE: u32 = 0x500;
const RXD: u32 = 0x518;
const TXD: u32 = 0x51C;
const FREQUENCY: u32 = 0x524;
const RXD_PTR: u32 = 0x534;
const RXD_MAXCNT: u32 = 0x538;
const RXD_AMOUNT: u32 = 0x53C;
const RXD_LIST: u32 = 0x540;
const TXD_PTR: u32 = 0x544;
const TXD_MAXCNT: u32 = 0x548;
const TXD_AMOUNT: u32 = 0x54C;
const TXD_LIST: u32 = 0x550;
const ADDRESS: u32 = 0x588;
const ORC: u32 = 0x5C0;

// Event numbers: each event sits at 0x100 + 4n.
const EVENT_STOPPED: u32 = 1;
const EVENT_READY: u32 = 2;
const EVENT_ENDRX: u32 = 4;
const EVENT_END: u32 = 6;
const EVENT_TXDSENT: u32 = 7;
const EVENT_ENDTX: u32 = 8;
const EVENT_ERROR: u32 = 9;
const EVENT_BB: u32 = 14;
const EVENT_SUSPENDED: u32 = 18;
const EVENT_STARTED: u32 = 19;
const EVENT_TXSTARTED: u32 = 20;
const EVENT_LASTRX: u32 = 23;
const EVENT_LASTTX: u32 = 24;

// Every event any of the four modes defines.
const VALID_EVENTS: u32 = (1 << 1)
    | (1 << 2)
    | (1 << 4)
    | (1 << 6)
    | (1 << 7)
    | (1 << 8)
    | (1 << 9)
    | (1 << 14)
    | (1 << 18)
    | (1 << 19)
    | (1 << 20)
    | (1 << 23)
    | (1 << 24);

const SHORT_BB_SUSPEND: u32 = 1 << 0;
const SHORT_BB_STOP: u32 = 1 << 1;
const SHORT_LASTTX_STARTRX: u32 = 1 << 7;
const SHORT_LASTTX_SUSPEND: u32 = 1 << 8;
const SHORT_LASTTX_STOP: u32 = 1 << 9;
const SHORT_LASTRX_STARTTX: u32 = 1 << 10;
const SHORT_LASTRX_STOP: u32 = 1 << 12;
const SHORT_END_START: u32 = 1 << 17;

const ERROR_ANACK: u32 = 1 << 1;
const ERROR_DNACK: u32 = 1 << 2;

const ARRAY_LIST: u32 = 1;
const LEGACY_BUFFER_DEPTH: usize = 2;
const DEFAULT_FREQUENCY: u32 = 0x0400_0000;
const BITS_PER_SPI_BYTE: u64 = 8;
const BITS_PER_I2C_BYTE: u64 = 9;

const TRANSFER_TOKEN: u32 = 0;

static LAYOUT: [PlainRegister; 13] = [
    plain(0x508, 0xFFFF_FFFF, u32::MAX),
    plain(0x50C, 0xFFFF_FFFF, u32::MAX),
    plain(0x510, 0xFFFF_FFFF, u32::MAX),
    plain(0x51C, 0, 0xFF),
    plain(0x524, DEFAULT_FREQUENCY, u32::MAX),
    plain(0x534, 0, u32::MAX),
    plain(0x538, 0, 0xFF),
    plain(0x540, 0, 0x7),
    plain(0x544, 0, u32::MAX),
    plain(0x548, 0, 0xFF),
    plain(0x550, 0, 0x7),
    plain(0x554, 0, 0x7),
    plain(0x588, 0, 0x7F),
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Disabled = 0,
    Spi = 1,
    TwoWire = 5,
    TwoWireMaster = 6,
    SpiMaster = 7,
}

/// The end of a transfer that is still crossing the bus.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    SpiMasterEnd,
    TwoWireTransmitDone,
    TwoWireReceiveDone,
    TwoWireAddressNack,
    TwoWireDataNack,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WaitingTask {
    None,
    StartTransmit,
    StartReceive,
}

// ============================================================================
// Serial interface
// ============================================================================

pub(crate) struct SerialInterface {
    base: u32,
    instance: u8,
    events: EventRegisters,
    plain: PlainRegisters,
    mode: Mode,
    error_source: u32,
    suspended: bool,
    pending: Pending,
    stop_after_pending: bool,
    transmit_cursor: u32,
    receive_cursor: u32,
    transmit_amount: u32,
    receive_amount: u32,
    spi_receive_fifo: Vec<u8>,
    spi_transmit_queue: Vec<u8>,
    spi_start_waiting_for_resume: bool,
    two_wire_active: bool,
    address_not_acknowledged: bool,
    last_segment_was_write: bool,
    legacy_receiving: bool,
    legacy_transmitting: bool,
    transmit_data_waiting: bool,
    receive_data_unread: bool,
    receive_data: u8,
    task_waiting_for_resume: WaitingTask,
}

impl SerialInterface {
    pub fn new(base: u32, instance: u8) -> SerialInterface {
        SerialInterface {
            base,
            instance,
            events: EventRegisters::new(base, VALID_EVENTS),
            plain: PlainRegisters::new(&LAYOUT),
            mode: Mode::Disabled,
            error_source: 0,
            suspended: false,
            pending: Pending::None,
            stop_after_pending: false,
            transmit_cursor: 0,
            receive_cursor: 0,
            transmit_amount: 0,
            receive_amount: 0,
            spi_receive_fifo: Vec::new(),
            spi_transmit_queue: Vec::new(),
            spi_start_waiting_for_resume: false,
            two_wire_active: false,
            address_not_acknowledged: false,
            last_segment_was_write: false,
            legacy_receiving: false,
            legacy_transmitting: false,
            transmit_data_waiting: false,
            receive_data_unread: false,
            receive_data: 0,
            task_waiting_for_resume: WaitingTask::None,
        }
    }

    pub fn reset(&mut self, context: &mut Context) {
        *self = SerialInterface::new(self.base, self.instance);
        context.cancel(self.transfer_key());
    }

    pub fn interrupt_pending(&self) -> bool {
        self.mode != Mode::Disabled && self.events.interrupt_pending()
    }

    fn transfer_key(&self) -> pinetime_core::EventKey {
        event_key(peripheral_id(self.base), TRANSFER_TOKEN)
    }

    fn shortcut(&self, shortcut: u32) -> bool {
        self.plain.read(SHORTS) & shortcut != 0
    }

    fn clear_transfer_state(&mut self, context: &mut Context) {
        self.spi_receive_fifo.clear();
        self.spi_transmit_queue.clear();
        self.spi_start_waiting_for_resume = false;
        self.suspended = false;
        self.pending = Pending::None;
        self.stop_after_pending = false;
        self.two_wire_active = false;
        self.address_not_acknowledged = false;
        self.receive_data_unread = false;
        self.receive_data = 0;
        self.transmit_data_waiting = false;
        self.legacy_receiving = false;
        self.legacy_transmitting = false;
        self.task_waiting_for_resume = WaitingTask::None;
        self.last_segment_was_write = false;
        self.receive_amount = 0;
        self.transmit_amount = 0;
        context.cancel(self.transfer_key());
    }

    // ========================================================================
    // Bus timing
    // ========================================================================

    // FREQUENCY counts in units of 16 MHz / 2^32, so one bit lasts 2^34 / FREQUENCY cycles of 64 MHz.
    fn cycles_per_bit(&self) -> u64 {
        let frequency = match self.plain.read(FREQUENCY) {
            0 => DEFAULT_FREQUENCY,
            frequency => frequency,
        };
        ((1u64 << 34) / frequency as u64).max(1)
    }

    fn finish_transfer_after(&mut self, pending: Pending, bits: u64, context: &mut Context) {
        self.pending = pending;
        let end: Cycle = context.now + bits * self.cycles_per_bit();
        context.schedule(self.transfer_key(), end);
    }

    // ========================================================================
    // Registers
    // ========================================================================

    pub fn read(&mut self, offset: u32, context: &mut Context) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            ERRORSRC => self.error_source,
            ENABLE => self.mode as u32,
            RXD => self.read_receive_data(context) as u32,
            RXD_AMOUNT => self.receive_amount,
            TXD_AMOUNT => self.transmit_amount,
            0x000..0x100 => 0,
            _ => self.plain.read(offset),
        }
    }

    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_STARTRX if triggers(value) => self.start_receive_task(context),
            TASKS_STARTTX if triggers(value) => self.start_transmit_task(context),
            TASKS_START if triggers(value) => self.start_task(context),
            TASKS_STOP if triggers(value) => self.stop_task(context),
            TASKS_SUSPEND if triggers(value) => self.suspend_task(context),
            TASKS_RESUME if triggers(value) => self.resume_task(context),
            0x000..0x100 => {}
            ERRORSRC => self.error_source &= !value,
            ENABLE => self.set_mode(value, context),
            TXD => {
                self.plain.write(TXD, value);
                self.write_transmit_data(value as u8, context);
            }
            RXD_PTR => {
                self.plain.write(RXD_PTR, value);
                self.receive_cursor = value;
            }
            TXD_PTR => {
                self.plain.write(TXD_PTR, value);
                self.transmit_cursor = value;
            }
            RXD | RXD_AMOUNT | TXD_AMOUNT => {}
            _ => self.plain.write(offset, value),
        }
    }

    fn set_mode(&mut self, value: u32, context: &mut Context) {
        let mode = match value & 0xF {
            1 => Mode::Spi,
            5 => Mode::TwoWire,
            6 => Mode::TwoWireMaster,
            7 => Mode::SpiMaster,
            _ => Mode::Disabled,
        };
        if mode != self.mode {
            if self.two_wire_active {
                context.i2c_stop(self.instance);
            }
            self.clear_transfer_state(context);
            self.mode = mode;
        }
    }

    pub fn handle_event(&mut self, _token: u32, context: &mut Context) {
        let pending = std::mem::replace(&mut self.pending, Pending::None);
        match pending {
            Pending::None => return,
            Pending::SpiMasterEnd => self.finish_spi_master_transfer(context),
            Pending::TwoWireTransmitDone => self.finish_two_wire_transmit(context),
            Pending::TwoWireReceiveDone => self.finish_two_wire_receive(context),
            Pending::TwoWireAddressNack => {
                self.error_source |= ERROR_ANACK;
                self.events.raise(EVENT_ERROR, context);
            }
            Pending::TwoWireDataNack => {
                self.error_source |= ERROR_DNACK;
                self.events.raise(EVENT_ERROR, context);
            }
        }
        if self.stop_after_pending && self.pending == Pending::None {
            self.stop_after_pending = false;
            self.stop_task(context);
        }
    }

    // ========================================================================
    // Tasks
    // ========================================================================

    fn start_task(&mut self, context: &mut Context) {
        if self.mode != Mode::SpiMaster || self.pending != Pending::None {
            return;
        }
        if self.suspended {
            self.spi_start_waiting_for_resume = true;
            return;
        }
        self.run_spi_master_transfer(context);
    }

    fn start_transmit_task(&mut self, context: &mut Context) {
        match self.mode {
            Mode::TwoWireMaster if self.suspended => {
                self.task_waiting_for_resume = WaitingTask::StartTransmit
            }
            Mode::TwoWireMaster if self.pending == Pending::None => {
                self.run_two_wire_transmit(false, context)
            }
            Mode::TwoWire => self.start_legacy_transmit(context),
            _ => {}
        }
    }

    fn start_receive_task(&mut self, context: &mut Context) {
        match self.mode {
            Mode::TwoWireMaster if self.suspended => self.task_waiting_for_resume = WaitingTask::StartReceive,
            Mode::TwoWireMaster if self.pending == Pending::None => self.run_two_wire_receive(context),
            Mode::TwoWire => self.start_legacy_receive(context),
            _ => {}
        }
    }

    fn stop_task(&mut self, context: &mut Context) {
        match self.mode {
            Mode::SpiMaster => {
                self.spi_start_waiting_for_resume = false;
                self.suspended = false;
                self.pending = Pending::None;
                context.cancel(self.transfer_key());
                self.events.raise(EVENT_STOPPED, context);
            }
            // A stop during a transfer waits for the transfer to end.
            Mode::TwoWire | Mode::TwoWireMaster if self.pending != Pending::None => {
                self.stop_after_pending = true
            }
            Mode::TwoWire | Mode::TwoWireMaster => self.stop_two_wire(context),
            _ => {}
        }
    }

    fn suspend_task(&mut self, context: &mut Context) {
        match self.mode {
            Mode::SpiMaster => self.suspended = true,
            Mode::TwoWire | Mode::TwoWireMaster if self.two_wire_active => self.suspend_two_wire(context),
            _ => {}
        }
    }

    fn resume_task(&mut self, context: &mut Context) {
        if !self.suspended {
            return;
        }
        self.suspended = false;
        match self.mode {
            Mode::SpiMaster if self.spi_start_waiting_for_resume => {
                self.spi_start_waiting_for_resume = false;
                self.run_spi_master_transfer(context);
            }
            Mode::TwoWire => self.continue_legacy_transfer(context),
            Mode::TwoWireMaster => {
                match std::mem::replace(&mut self.task_waiting_for_resume, WaitingTask::None) {
                    WaitingTask::StartTransmit => {
                        self.run_two_wire_transmit(self.last_segment_was_write, context)
                    }
                    WaitingTask::StartReceive => self.run_two_wire_receive(context),
                    WaitingTask::None => {}
                }
            }
            _ => {}
        }
    }

    // ========================================================================
    // Legacy SPI
    // ========================================================================

    fn write_transmit_data(&mut self, value: u8, context: &mut Context) {
        match self.mode {
            Mode::Spi if self.spi_transmit_queue.len() < LEGACY_BUFFER_DEPTH => {
                self.spi_transmit_queue.push(value);
                self.pump_legacy_spi(context);
            }
            Mode::TwoWire => {
                self.transmit_data_waiting = true;
                self.continue_legacy_transfer(context);
            }
            _ => {}
        }
    }

    fn read_receive_data(&mut self, context: &mut Context) -> u8 {
        match self.mode {
            Mode::Spi if !self.spi_receive_fifo.is_empty() => {
                let value = self.spi_receive_fifo.remove(0);
                if !self.spi_receive_fifo.is_empty() {
                    self.events.raise(EVENT_READY, context);
                }
                self.pump_legacy_spi(context);
                value
            }
            Mode::TwoWire => {
                let value = self.receive_data;
                self.receive_data_unread = false;
                self.continue_legacy_transfer(context);
                value
            }
            _ => 0,
        }
    }

    // Sends queued bytes while the two-byte receive buffer has room, as the hardware stalls otherwise.
    fn pump_legacy_spi(&mut self, context: &mut Context) {
        while !self.spi_transmit_queue.is_empty() && self.spi_receive_fifo.len() < LEGACY_BUFFER_DEPTH {
            let sent = self.spi_transmit_queue.remove(0);
            let received = context.spi_transfer(self.instance, sent);
            self.spi_receive_fifo.push(received);
            if self.spi_receive_fifo.len() == 1 {
                self.events.raise(EVENT_READY, context);
            }
        }
    }

    // ========================================================================
    // SPIM
    // ========================================================================

    fn run_spi_master_transfer(&mut self, context: &mut Context) {
        self.events.raise(EVENT_STARTED, context);

        let transmit_count = self.plain.read(TXD_MAXCNT) as usize;
        let receive_count = self.plain.read(RXD_MAXCNT) as usize;
        let transmit_address = self.list_address(TXD_LIST, self.transmit_cursor, TXD_PTR);
        let receive_address = self.list_address(RXD_LIST, self.receive_cursor, RXD_PTR);
        let over_read_character = self.plain.read(ORC) as u8;

        let outgoing = context.memory.dma_read_vec(transmit_address, transmit_count);
        let total = transmit_count.max(receive_count);
        let mut incoming: Vec<u8> = (0..total)
            .map(|index| {
                let sent = outgoing.get(index).copied().unwrap_or(over_read_character);
                context.spi_transfer(self.instance, sent)
            })
            .collect();
        incoming.truncate(receive_count);
        if receive_count > 0 {
            context.memory.dma_write(receive_address, &incoming);
        }

        self.transmit_amount = transmit_count as u32;
        self.receive_amount = receive_count as u32;
        self.advance_list_cursors(transmit_count, receive_count);
        if total == 0 {
            // A transfer of no bytes ends at once, so a START right after it is not lost.
            self.finish_spi_master_transfer(context);
            return;
        }
        let bits = total as u64 * BITS_PER_SPI_BYTE;
        self.finish_transfer_after(Pending::SpiMasterEnd, bits, context);
    }

    fn finish_spi_master_transfer(&mut self, context: &mut Context) {
        self.events.raise(EVENT_ENDTX, context);
        self.events.raise(EVENT_ENDRX, context);
        self.events.raise(EVENT_END, context);
        if self.mode == Mode::SpiMaster && self.shortcut(SHORT_END_START) {
            self.start_task(context);
        }
    }

    // ========================================================================
    // EasyDMA lists
    // ========================================================================

    fn list_address(&self, list_register: u32, cursor: u32, pointer_register: u32) -> u32 {
        if self.plain.read(list_register) == ARRAY_LIST {
            cursor
        } else {
            self.plain.read(pointer_register)
        }
    }

    // In array-list mode each transfer moves its pointer on by MAXCNT, so the next one uses the next buffer.
    fn advance_list_cursors(&mut self, transmit_count: usize, receive_count: usize) {
        self.transmit_cursor = if self.plain.read(TXD_LIST) == ARRAY_LIST {
            self.transmit_cursor.wrapping_add(transmit_count as u32)
        } else {
            self.plain.read(TXD_PTR)
        };
        self.receive_cursor = if self.plain.read(RXD_LIST) == ARRAY_LIST {
            self.receive_cursor.wrapping_add(receive_count as u32)
        } else {
            self.plain.read(RXD_PTR)
        };
    }

    // ========================================================================
    // TWI and TWIM shared transaction
    // ========================================================================

    /// Sends a start or repeated start and the address; false means no part acknowledged.
    fn begin_two_wire_segment(&mut self, read: bool, context: &mut Context) -> bool {
        let address = self.plain.read(ADDRESS) as u8;
        self.two_wire_active = true;
        let acknowledged = context.i2c_start(self.instance, address, read);
        self.address_not_acknowledged = !acknowledged;
        acknowledged
    }

    fn stop_two_wire(&mut self, context: &mut Context) {
        if self.two_wire_active {
            context.i2c_stop(self.instance);
        }
        self.two_wire_active = false;
        self.address_not_acknowledged = false;
        self.suspended = false;
        self.legacy_receiving = false;
        self.legacy_transmitting = false;
        self.task_waiting_for_resume = WaitingTask::None;
        self.events.raise(EVENT_STOPPED, context);
    }

    fn suspend_two_wire(&mut self, context: &mut Context) {
        self.suspended = true;
        self.events.raise(EVENT_SUSPENDED, context);
    }

    // ========================================================================
    // Legacy TWI
    // ========================================================================

    fn start_legacy_transmit(&mut self, context: &mut Context) {
        self.legacy_receiving = false;
        self.legacy_transmitting = true;
        self.suspended = false;
        if !self.begin_two_wire_segment(false, context) {
            self.error_source |= ERROR_ANACK;
            self.events.raise(EVENT_ERROR, context);
            return;
        }
        self.last_segment_was_write = true;
        self.continue_legacy_transfer(context);
    }

    fn start_legacy_receive(&mut self, context: &mut Context) {
        self.legacy_transmitting = false;
        self.legacy_receiving = true;
        self.suspended = false;
        self.receive_data_unread = false;
        if !self.begin_two_wire_segment(true, context) {
            self.error_source |= ERROR_ANACK;
            self.events.raise(EVENT_ERROR, context);
            return;
        }
        self.last_segment_was_write = false;
        self.continue_legacy_transfer(context);
    }

    // Moves the next byte if the bus is running: a written TXD goes out, or a read RXD lets the next byte in.
    fn continue_legacy_transfer(&mut self, context: &mut Context) {
        if self.mode != Mode::TwoWire
            || !self.two_wire_active
            || self.suspended
            || self.address_not_acknowledged
        {
            return;
        }
        if self.legacy_transmitting && self.transmit_data_waiting {
            self.transmit_data_waiting = false;
            let byte = self.plain.read(TXD) as u8;
            if !context.i2c_write(self.instance, byte) {
                self.error_source |= ERROR_DNACK;
                self.events.raise(EVENT_ERROR, context);
            }
            self.events.raise(EVENT_TXDSENT, context);
            self.finish_legacy_byte(context);
        } else if self.legacy_receiving && !self.receive_data_unread {
            self.receive_data = context.i2c_read(self.instance);
            self.receive_data_unread = true;
            self.events.raise(EVENT_READY, context);
            self.finish_legacy_byte(context);
        }
    }

    fn finish_legacy_byte(&mut self, context: &mut Context) {
        self.events.raise(EVENT_BB, context);
        if self.shortcut(SHORT_BB_STOP) {
            self.stop_two_wire(context);
        } else if self.shortcut(SHORT_BB_SUSPEND) {
            self.suspend_two_wire(context);
        }
    }

    // ========================================================================
    // TWIM
    // ========================================================================

    fn run_two_wire_transmit(&mut self, continue_previous_write: bool, context: &mut Context) {
        self.events.raise(EVENT_TXSTARTED, context);
        let mut bits = 0;
        if !continue_previous_write || !self.two_wire_active || self.address_not_acknowledged {
            bits += BITS_PER_I2C_BYTE;
            if !self.begin_two_wire_segment(false, context) {
                self.transmit_amount = 0;
                self.finish_transfer_after(Pending::TwoWireAddressNack, bits, context);
                return;
            }
        }
        self.last_segment_was_write = true;

        let count = self.plain.read(TXD_MAXCNT) as usize;
        let address = self.list_address(TXD_LIST, self.transmit_cursor, TXD_PTR);
        let bytes = context.memory.dma_read_vec(address, count);
        let mut sent = 0;
        let mut acknowledged = true;
        for byte in bytes {
            sent += 1;
            if !context.i2c_write(self.instance, byte) {
                acknowledged = false;
                break;
            }
        }
        self.transmit_amount = sent;
        self.advance_list_cursors(count, 0);
        bits += sent as u64 * BITS_PER_I2C_BYTE;
        let pending = if acknowledged {
            Pending::TwoWireTransmitDone
        } else {
            Pending::TwoWireDataNack
        };
        self.finish_transfer_after(pending, bits.max(1), context);
    }

    fn finish_two_wire_transmit(&mut self, context: &mut Context) {
        self.events.raise(EVENT_LASTTX, context);
        if self.shortcut(SHORT_LASTTX_STARTRX) {
            self.run_two_wire_receive(context);
        } else if self.shortcut(SHORT_LASTTX_SUSPEND) {
            self.suspend_two_wire(context);
        } else if self.shortcut(SHORT_LASTTX_STOP) {
            self.stop_two_wire(context);
        }
    }

    fn run_two_wire_receive(&mut self, context: &mut Context) {
        self.events.raise(EVENT_STARTED, context);
        if !self.begin_two_wire_segment(true, context) {
            self.receive_amount = 0;
            self.finish_transfer_after(Pending::TwoWireAddressNack, BITS_PER_I2C_BYTE, context);
            return;
        }
        self.last_segment_was_write = false;

        let count = self.plain.read(RXD_MAXCNT) as usize;
        let address = self.list_address(RXD_LIST, self.receive_cursor, RXD_PTR);
        let bytes: Vec<u8> = (0..count).map(|_| context.i2c_read(self.instance)).collect();
        if count > 0 {
            context.memory.dma_write(address, &bytes);
        }
        self.receive_amount = count as u32;
        self.advance_list_cursors(0, count);
        let bits = (1 + count as u64) * BITS_PER_I2C_BYTE;
        self.finish_transfer_after(Pending::TwoWireReceiveDone, bits, context);
    }

    fn finish_two_wire_receive(&mut self, context: &mut Context) {
        self.events.raise(EVENT_LASTRX, context);
        if self.shortcut(SHORT_LASTRX_STARTTX) {
            self.run_two_wire_transmit(false, context);
        } else if self.shortcut(SHORT_LASTRX_STOP) {
            self.stop_two_wire(context);
        }
    }
}
