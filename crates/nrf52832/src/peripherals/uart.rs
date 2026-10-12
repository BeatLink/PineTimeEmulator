// SPDX-License-Identifier: GPL-3.0-only
//! The UART and its EasyDMA form, the UARTE, which share 0x40002000, and the nRF52840's UARTE1 at
//! 0x40028000. Bytes the firmware sends collect in an output buffer; bytes from outside wait in a
//! queue until a receiver is armed to take them. Bytes move at once, without baud-rate timing.

use std::collections::VecDeque;

use super::{plain, triggers, EventRegisters, PlainRegister, PlainRegisters};
use crate::context::Context;

// ============================================================================
// Registers
// ============================================================================

pub(crate) const UART0_BASE: u32 = 0x4000_2000;
pub(crate) const UARTE1_BASE: u32 = 0x4002_8000;

const TASKS_STARTRX: u32 = 0x000;
const TASKS_STOPRX: u32 = 0x004;
const TASKS_STARTTX: u32 = 0x008;
const TASKS_STOPTX: u32 = 0x00C;
const TASKS_FLUSHRX: u32 = 0x02C;
const SHORTS: u32 = 0x200;
const ERRORSRC: u32 = 0x480;
const ENABLE: u32 = 0x500;
const RXD: u32 = 0x518;
const TXD: u32 = 0x51C;
const RXD_PTR: u32 = 0x534;
const RXD_MAXCNT: u32 = 0x538;
const RXD_AMOUNT: u32 = 0x53C;
const TXD_PTR: u32 = 0x544;
const TXD_MAXCNT: u32 = 0x548;
const TXD_AMOUNT: u32 = 0x54C;

const EVENT_RXDRDY: u32 = 2;
const EVENT_ENDRX: u32 = 4;
const EVENT_TXDRDY: u32 = 7;
const EVENT_ENDTX: u32 = 8;
const EVENT_RXTO: u32 = 17;
const EVENT_RXSTARTED: u32 = 19;
const EVENT_TXSTARTED: u32 = 20;
const EVENT_TXSTOPPED: u32 = 22;
const VALID_EVENTS: u32 = 0x005A_0397;

const SHORT_ENDRX_STARTRX: u32 = 1 << 5;
const SHORT_ENDRX_STOPRX: u32 = 1 << 6;

const ENABLE_UART: u32 = 4;
const ENABLE_UARTE: u32 = 8;

/// The nRF52832's UARTE counts up to 255 bytes per buffer.
static NRF52832_LAYOUT: [PlainRegister; 10] = layout(0xFF);

/// The nRF52840's UARTEs count up to 65535 bytes per buffer.
static NRF52840_LAYOUT: [PlainRegister; 10] = layout(0xFFFF);

const fn layout(maximum_count_mask: u32) -> [PlainRegister; 10] {
    [
        plain(0x200, 0, 0x78),
        plain(0x508, 0xFFFF_FFFF, u32::MAX),
        plain(0x50C, 0xFFFF_FFFF, u32::MAX),
        plain(0x510, 0xFFFF_FFFF, u32::MAX),
        plain(0x514, 0xFFFF_FFFF, u32::MAX),
        plain(0x524, 0x0400_0000, u32::MAX),
        plain(0x538, 0, maximum_count_mask),
        plain(0x548, 0, maximum_count_mask),
        plain(0x56C, 0, 0xF),
        plain(0x51C, 0, 0xFF),
    ]
}

// ============================================================================
// UART and UARTE
// ============================================================================

pub(crate) struct Uart {
    base: u32,
    nrf52840_counts: bool,
    events: EventRegisters,
    plain: PlainRegisters,
    enable: u32,
    error_source: u32,
    receiving: bool,
    receive_data: Option<u8>,
    receive_pointer: u32,
    receive_buffer: u32,
    receive_maximum_count: u32,
    receive_amount: u32,
    transmit_amount: u32,
    incoming: VecDeque<u8>,
    output: Vec<u8>,
}

impl Uart {
    /// A UART at `base`; `nrf52840_counts` widens the EasyDMA byte counts to 16 bits.
    pub fn new(base: u32, nrf52840_counts: bool) -> Uart {
        Uart {
            base,
            nrf52840_counts,
            events: EventRegisters::new(base, VALID_EVENTS),
            plain: PlainRegisters::new(if nrf52840_counts {
                &NRF52840_LAYOUT
            } else {
                &NRF52832_LAYOUT
            }),
            enable: 0,
            error_source: 0,
            receiving: false,
            receive_data: None,
            receive_pointer: 0,
            receive_buffer: 0,
            receive_maximum_count: 0,
            receive_amount: 0,
            transmit_amount: 0,
            incoming: VecDeque::new(),
            output: Vec::new(),
        }
    }

    /// Resets the registers; bytes waiting in either direction stay.
    pub fn reset(&mut self) {
        let incoming = std::mem::take(&mut self.incoming);
        let output = std::mem::take(&mut self.output);
        *self = Uart::new(self.base, self.nrf52840_counts);
        self.incoming = incoming;
        self.output = output;
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn take_output(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.output)
    }

    pub fn waiting_bytes(&self) -> usize {
        self.incoming.len()
    }

    /// Queues bytes arriving on the RX line and hands them to a running receiver.
    pub fn receive(&mut self, bytes: &[u8], context: &mut Context) {
        self.incoming.extend(bytes);
        self.deliver(context);
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
            ENABLE => self.enable,
            RXD => {
                let value = self.receive_data.take().unwrap_or(0);
                self.deliver(context);
                value as u32
            }
            RXD_PTR => self.receive_pointer,
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
            TASKS_STARTRX if triggers(value) => self.start_receive(context),
            TASKS_STOPRX if triggers(value) => self.stop_receive(context),
            TASKS_STARTTX if triggers(value) => self.start_transmit(context),
            TASKS_STOPTX if triggers(value) && self.enable == ENABLE_UARTE => {
                self.events.raise(EVENT_TXSTOPPED, context);
            }
            TASKS_FLUSHRX if triggers(value) && self.enable == ENABLE_UARTE => {
                self.receive_amount = 0;
                self.events.raise(EVENT_ENDRX, context);
            }
            0x000..0x100 | RXD | RXD_AMOUNT | TXD_AMOUNT => {}
            ERRORSRC => self.error_source &= !value,
            ENABLE => {
                self.enable = value & 0xF;
                if self.enable != ENABLE_UART && self.enable != ENABLE_UARTE {
                    self.receiving = false;
                }
            }
            TXD => {
                self.plain.write(TXD, value);
                if self.enable == ENABLE_UART {
                    self.output.push(value as u8);
                    self.events.raise(EVENT_TXDRDY, context);
                }
            }
            RXD_PTR => self.receive_pointer = value,
            _ => self.plain.write(offset, value),
        }
    }

    // ========================================================================
    // Transmitting
    // ========================================================================

    fn start_transmit(&mut self, context: &mut Context) {
        if self.enable != ENABLE_UARTE {
            return;
        }
        self.events.raise(EVENT_TXSTARTED, context);
        let count = self.plain.read(TXD_MAXCNT) as usize;
        let bytes = context.memory.dma_read_vec(self.plain.read(TXD_PTR), count);
        self.output.extend_from_slice(&bytes);
        self.transmit_amount = count as u32;
        self.events.raise(EVENT_ENDTX, context);
    }

    // ========================================================================
    // Receiving
    // ========================================================================

    // The UARTE latches its buffer here, so firmware may queue the next one straight away.
    fn start_receive(&mut self, context: &mut Context) {
        match self.enable {
            ENABLE_UART => self.receiving = true,
            ENABLE_UARTE => {
                self.receiving = true;
                self.receive_buffer = self.receive_pointer;
                self.receive_maximum_count = self.plain.read(RXD_MAXCNT);
                self.receive_amount = 0;
                self.events.raise(EVENT_RXSTARTED, context);
            }
            _ => return,
        }
        self.deliver(context);
    }

    fn stop_receive(&mut self, context: &mut Context) {
        let was_receiving = self.receiving;
        self.receiving = false;
        if self.enable == ENABLE_UARTE && was_receiving {
            self.events.raise(EVENT_ENDRX, context);
        }
        self.events.raise(EVENT_RXTO, context);
    }

    fn deliver(&mut self, context: &mut Context) {
        match self.enable {
            ENABLE_UART if self.receiving && self.receive_data.is_none() => {
                if let Some(byte) = self.incoming.pop_front() {
                    self.receive_data = Some(byte);
                    self.events.raise(EVENT_RXDRDY, context);
                }
            }
            ENABLE_UARTE => {
                while self.receiving && self.receive_amount < self.receive_maximum_count {
                    let Some(byte) = self.incoming.pop_front() else {
                        break;
                    };
                    let address = self.receive_buffer.wrapping_add(self.receive_amount);
                    context.memory.dma_write(address, &[byte]);
                    self.receive_amount += 1;
                    self.events.raise(EVENT_RXDRDY, context);
                    if self.receive_amount == self.receive_maximum_count {
                        self.end_receive_buffer(context);
                    }
                }
            }
            _ => {}
        }
    }

    fn end_receive_buffer(&mut self, context: &mut Context) {
        self.events.raise(EVENT_ENDRX, context);
        let shortcuts = self.plain.read(SHORTS);
        if shortcuts & SHORT_ENDRX_STOPRX != 0 {
            self.receiving = false;
            self.events.raise(EVENT_RXTO, context);
        } else if shortcuts & SHORT_ENDRX_STARTRX != 0 {
            self.receive_buffer = self.receive_pointer;
            self.receive_maximum_count = self.plain.read(RXD_MAXCNT);
            self.receive_amount = 0;
            self.events.raise(EVENT_RXSTARTED, context);
        } else {
            self.receiving = false;
        }
    }
}
