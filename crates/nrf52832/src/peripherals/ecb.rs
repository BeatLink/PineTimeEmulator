// SPDX-License-Identifier: GPL-3.0-only
//! The AES electronic codebook block: encrypts one 16-byte block with a 128-bit key, both read
//! from RAM through EasyDMA.

use super::{triggers, EventRegisters};
use crate::aes::Aes128;
use crate::context::Context;

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_E000;

const TASKS_STARTECB: u32 = 0x000;
const TASKS_STOPECB: u32 = 0x004;
const ECBDATAPTR: u32 = 0x504;

const EVENT_ENDECB: u32 = 0;
const EVENT_ERRORECB: u32 = 1;

const KEY_OFFSET: u32 = 0;
const CLEARTEXT_OFFSET: u32 = 16;
const CIPHERTEXT_OFFSET: u32 = 32;

// ============================================================================
// ECB
// ============================================================================

pub(crate) struct Ecb {
    events: EventRegisters,
    data_pointer: u32,
}

impl Ecb {
    pub fn new() -> Ecb {
        Ecb {
            events: EventRegisters::new(BASE, 0x3),
            data_pointer: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = Ecb::new();
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            ECBDATAPTR => self.data_pointer,
            _ => 0,
        }
    }

    // A block encrypts at once, so STOPECB has nothing to abort.
    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_STARTECB if triggers(value) => self.encrypt(context),
            TASKS_STOPECB => {}
            ECBDATAPTR => self.data_pointer = value,
            _ => {}
        }
    }

    // The data structure is the key and the cleartext, most significant byte first, then room for the ciphertext.
    fn encrypt(&mut self, context: &mut Context) {
        let mut key = [0; 16];
        let mut cleartext = [0; 16];
        context
            .memory
            .dma_read(self.data_pointer.wrapping_add(KEY_OFFSET), &mut key);
        context
            .memory
            .dma_read(self.data_pointer.wrapping_add(CLEARTEXT_OFFSET), &mut cleartext);
        let ciphertext = Aes128::new(&key).encrypt_block(&cleartext);
        let event = if context
            .memory
            .dma_write(self.data_pointer.wrapping_add(CIPHERTEXT_OFFSET), &ciphertext)
        {
            EVENT_ENDECB
        } else {
            EVENT_ERRORECB
        };
        self.events.raise(event, context);
    }
}
