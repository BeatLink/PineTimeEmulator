// SPDX-License-Identifier: GPL-3.0-only
//! The shared CCM and AAR block: Bluetooth LE packet encryption (AES-CCM) and resolvable private
//! address resolution. The ENABLE value picks which one the tasks drive.

use super::{triggers, EventRegisters};
use crate::aes::Aes128;
use crate::context::Context;
use crate::memory::Memory;

// ============================================================================
// Registers
// ============================================================================

pub(crate) const BASE: u32 = 0x4000_F000;

const TASKS_KSGEN_START: u32 = 0x000;
const TASKS_CRYPT: u32 = 0x004;
const SHORTS: u32 = 0x200;
const STATUS: u32 = 0x400;
const ENABLE: u32 = 0x500;
const MODE_NIRK: u32 = 0x504;
const CNFPTR_IRKPTR: u32 = 0x508;
const INPTR: u32 = 0x50C;
const OUTPTR_ADDRPTR: u32 = 0x510;
const SCRATCHPTR: u32 = 0x514;

// Each event slot means one thing for the CCM and another for the AAR.
const EVENT_ENDKSGEN_END: u32 = 0;
const EVENT_ENDCRYPT_RESOLVED: u32 = 1;
const EVENT_ERROR_NOTRESOLVED: u32 = 2;

const ENABLE_CCM: u32 = 2;
const ENABLE_AAR: u32 = 3;
const MODE_DECRYPT: u32 = 1 << 0;
const MODE_EXTENDED_LENGTH: u32 = 1 << 24;
const IRK_COUNT_MASK: u32 = 0x1F;
const MAXIMUM_IRK_COUNT: u32 = 16;

const PACKET_HEADER_SIZE: u32 = 3;
const DEFAULT_LENGTH_MASK: u8 = 0x1F;
const CONFIGURATION_SIZE: usize = 33;
const KEY_SIZE: usize = 16;
const DIRECTION_OFFSET: usize = 24;
const IV_OFFSET: usize = 25;
const IV_SIZE: usize = 8;
const NONCE_SIZE: usize = 13;
const MIC_SIZE: usize = 4;
const BLOCK_SIZE: usize = 16;
const ADDRESS_SIZE: usize = 6;
const CCM_AUTHENTICATION_FLAGS: u8 = 0x49;
const CCM_COUNTER_FLAGS: u8 = 0x01;
const HEADER_AUTHENTICATION_MASK: u8 = 0xE3;

// ============================================================================
// CCM and AAR
// ============================================================================

pub(crate) struct CcmAar {
    events: EventRegisters,
    shortcut_keystream_to_crypt: bool,
    enable: u32,
    mode_or_irk_count: u32,
    configuration_or_irk_pointer: u32,
    input_pointer: u32,
    output_or_address_pointer: u32,
    scratch_pointer: u32,
    status: u32,
}

impl CcmAar {
    pub fn new() -> CcmAar {
        CcmAar {
            events: EventRegisters::new(BASE, 0x7),
            shortcut_keystream_to_crypt: false,
            enable: 0,
            mode_or_irk_count: 1,
            configuration_or_irk_pointer: 0,
            input_pointer: 0,
            output_or_address_pointer: 0,
            scratch_pointer: 0,
            status: 0,
        }
    }

    pub fn reset(&mut self) {
        *self = CcmAar::new();
    }

    pub fn interrupt_pending(&self) -> bool {
        self.events.interrupt_pending()
    }

    pub fn read(&self, offset: u32) -> u32 {
        if let Some(value) = self.events.read(offset) {
            return value;
        }
        match offset {
            SHORTS => self.shortcut_keystream_to_crypt as u32,
            STATUS => self.status,
            ENABLE => self.enable,
            MODE_NIRK => self.mode_or_irk_count,
            CNFPTR_IRKPTR => self.configuration_or_irk_pointer,
            INPTR => self.input_pointer,
            OUTPTR_ADDRPTR => self.output_or_address_pointer,
            SCRATCHPTR => self.scratch_pointer,
            _ => 0,
        }
    }

    // Every operation finishes at once, so STOP has nothing to stop.
    pub fn write(&mut self, offset: u32, value: u32, context: &mut Context) {
        if self.events.write(offset, value) {
            return;
        }
        match offset {
            TASKS_KSGEN_START if triggers(value) => self.keystream_or_start_task(context),
            TASKS_CRYPT if triggers(value) => self.crypt_task(context),
            SHORTS => self.shortcut_keystream_to_crypt = value & 1 != 0,
            ENABLE => self.enable = value & 3,
            MODE_NIRK => self.mode_or_irk_count = value,
            CNFPTR_IRKPTR => self.configuration_or_irk_pointer = value,
            INPTR => self.input_pointer = value,
            OUTPTR_ADDRPTR => self.output_or_address_pointer = value,
            SCRATCHPTR => self.scratch_pointer = value,
            _ => {}
        }
    }

    // ========================================================================
    // Tasks
    // ========================================================================

    fn keystream_or_start_task(&mut self, context: &mut Context) {
        match self.enable {
            // The keystream is derived during CRYPT, so KSGEN only has to report completion.
            ENABLE_CCM => {
                self.events.raise(EVENT_ENDKSGEN_END, context);
                if self.shortcut_keystream_to_crypt {
                    self.crypt_task(context);
                }
            }
            ENABLE_AAR => self.resolve_address(context),
            _ => {}
        }
    }

    fn crypt_task(&mut self, context: &mut Context) {
        if self.enable != ENABLE_CCM {
            return;
        }
        let extended_length = self.mode_or_irk_count & MODE_EXTENDED_LENGTH != 0;
        if self.mode_or_irk_count & MODE_DECRYPT != 0 {
            self.decrypt(extended_length, context.memory);
        } else {
            self.encrypt(extended_length, context.memory);
        }
        self.events.raise(EVENT_ENDCRYPT_RESOLVED, context);
    }

    // ========================================================================
    // CCM: Bluetooth LE AES-CCM
    // ========================================================================

    // Packets in RAM are a header byte, a length byte, an RFU byte, then the payload.
    fn read_packet(&self, memory: &Memory, extended_length: bool) -> ([u8; 3], Vec<u8>) {
        let mut header = [0; 3];
        memory.dma_read(self.input_pointer, &mut header);
        let length = if extended_length {
            header[1]
        } else {
            header[1] & DEFAULT_LENGTH_MASK
        };
        let payload = memory.dma_read_vec(
            self.input_pointer.wrapping_add(PACKET_HEADER_SIZE),
            length as usize,
        );
        (header, payload)
    }

    fn write_packet(&self, memory: &mut Memory, mut header: [u8; 3], payload: &[u8]) {
        header[1] = payload.len() as u8;
        memory.dma_write(self.output_or_address_pointer, &header);
        memory.dma_write(
            self.output_or_address_pointer.wrapping_add(PACKET_HEADER_SIZE),
            payload,
        );
    }

    fn encrypt(&self, extended_length: bool, memory: &mut Memory) {
        let (header, payload) = self.read_packet(memory, extended_length);
        let (cipher, nonce) = self.read_configuration(memory);
        // An empty packet is sent unencrypted and without a MIC.
        let output = if payload.is_empty() {
            payload
        } else {
            ccm_encrypt(&cipher, &nonce, header[0], &payload)
        };
        self.write_packet(memory, header, &output);
    }

    fn decrypt(&mut self, extended_length: bool, memory: &mut Memory) {
        let (header, packet) = self.read_packet(memory, extended_length);
        let (cipher, nonce) = self.read_configuration(memory);
        let (plaintext, mic_passed) = if packet.is_empty() {
            (packet, true)
        } else if packet.len() < MIC_SIZE {
            (Vec::new(), false)
        } else {
            ccm_decrypt(&cipher, &nonce, header[0], &packet)
        };
        self.write_packet(memory, header, &plaintext);
        self.status = mic_passed as u32;
    }

    // The CCM data structure: a 16-byte key, an 8-byte packet counter (39 bits used), a direction byte and an 8-byte IV.
    fn read_configuration(&self, memory: &Memory) -> (Aes128, [u8; NONCE_SIZE]) {
        let mut configuration = [0; CONFIGURATION_SIZE];
        memory.dma_read(self.configuration_or_irk_pointer, &mut configuration);
        let key: [u8; KEY_SIZE] = configuration[..KEY_SIZE].try_into().unwrap_or([0; KEY_SIZE]);
        // The nonce is the 39-bit counter little-endian, the direction bit on top, then the IV.
        let mut nonce = [0; NONCE_SIZE];
        nonce[..5].copy_from_slice(&configuration[KEY_SIZE..KEY_SIZE + 5]);
        nonce[4] = (nonce[4] & 0x7F) | ((configuration[DIRECTION_OFFSET] & 1) << 7);
        nonce[5..].copy_from_slice(&configuration[IV_OFFSET..IV_OFFSET + IV_SIZE]);
        (Aes128::new(&key), nonce)
    }

    // ========================================================================
    // AAR: resolvable private address resolution
    // ========================================================================

    // ADDRPTR points at a packet; the 6-byte address follows its 3-byte header, least significant byte first.
    fn resolve_address(&mut self, context: &mut Context) {
        let mut address = [0; ADDRESS_SIZE];
        context.memory.dma_read(
            self.output_or_address_pointer.wrapping_add(PACKET_HEADER_SIZE),
            &mut address,
        );
        let irk_count = (self.mode_or_irk_count & IRK_COUNT_MASK).min(MAXIMUM_IRK_COUNT);
        let resolved = (0..irk_count).find(|&index| {
            let mut irk = [0; KEY_SIZE];
            let irk_address = self
                .configuration_or_irk_pointer
                .wrapping_add(KEY_SIZE as u32 * index);
            context.memory.dma_read(irk_address, &mut irk);
            address_matches_irk(&irk, &address)
        });
        if let Some(index) = resolved {
            self.status = index;
        }
        let outcome = if resolved.is_some() {
            EVENT_ENDCRYPT_RESOLVED
        } else {
            EVENT_ERROR_NOTRESOLVED
        };
        self.events.raise(outcome, context);
        self.events.raise(EVENT_ENDKSGEN_END, context);
    }
}

// ah(k, r) = e(k, 0^104 || prand) mod 2^24; the IRK is stored most significant byte first, as AES takes it.
fn address_matches_irk(irk: &[u8; KEY_SIZE], address: &[u8; ADDRESS_SIZE]) -> bool {
    let mut block = [0; BLOCK_SIZE];
    block[13] = address[5];
    block[14] = address[4];
    block[15] = address[3];
    let hash = Aes128::new(irk).encrypt_block(&block);
    hash[15] == address[0] && hash[14] == address[1] && hash[13] == address[2]
}

// ============================================================================
// AES-CCM
// ============================================================================

fn ccm_encrypt(cipher: &Aes128, nonce: &[u8; NONCE_SIZE], header: u8, plaintext: &[u8]) -> Vec<u8> {
    let tag = ccm_tag(cipher, nonce, header, plaintext);
    let mut output = counter_mode(cipher, nonce, plaintext);
    let first_keystream = counter_block_keystream(cipher, nonce, 0);
    output.extend((0..MIC_SIZE).map(|index| tag[index] ^ first_keystream[index]));
    output
}

fn ccm_decrypt(cipher: &Aes128, nonce: &[u8; NONCE_SIZE], header: u8, packet: &[u8]) -> (Vec<u8>, bool) {
    let (ciphertext, mic) = packet.split_at(packet.len() - MIC_SIZE);
    let plaintext = counter_mode(cipher, nonce, ciphertext);
    let tag = ccm_tag(cipher, nonce, header, &plaintext);
    let first_keystream = counter_block_keystream(cipher, nonce, 0);
    let mic_passed = (0..MIC_SIZE).all(|index| mic[index] == tag[index] ^ first_keystream[index]);
    (plaintext, mic_passed)
}

// The CBC-MAC over B0, the one-byte header with NESN, SN and MD masked off, and the payload.
fn ccm_tag(cipher: &Aes128, nonce: &[u8; NONCE_SIZE], header: u8, plaintext: &[u8]) -> [u8; BLOCK_SIZE] {
    let mut block = [0; BLOCK_SIZE];
    block[0] = CCM_AUTHENTICATION_FLAGS;
    block[1..1 + NONCE_SIZE].copy_from_slice(nonce);
    block[14] = (plaintext.len() >> 8) as u8;
    block[15] = plaintext.len() as u8;
    let mut mac = cipher.encrypt_block(&block);

    let mut block = [0; BLOCK_SIZE];
    block[1] = 0x01;
    block[2] = header & HEADER_AUTHENTICATION_MASK;
    mac = cipher.encrypt_block(&xor(&mac, &block));

    for chunk in plaintext.chunks(BLOCK_SIZE) {
        let mut block = [0; BLOCK_SIZE];
        block[..chunk.len()].copy_from_slice(chunk);
        mac = cipher.encrypt_block(&xor(&mac, &block));
    }
    mac
}

// Counter blocks from 1 upward encrypt the payload; counter block 0 masks the MIC.
fn counter_mode(cipher: &Aes128, nonce: &[u8; NONCE_SIZE], input: &[u8]) -> Vec<u8> {
    input
        .chunks(BLOCK_SIZE)
        .enumerate()
        .flat_map(|(index, chunk)| {
            let keystream = counter_block_keystream(cipher, nonce, index as u16 + 1);
            chunk
                .iter()
                .zip(keystream)
                .map(|(byte, key)| byte ^ key)
                .collect::<Vec<u8>>()
        })
        .collect()
}

fn counter_block_keystream(cipher: &Aes128, nonce: &[u8; NONCE_SIZE], counter: u16) -> [u8; BLOCK_SIZE] {
    let mut block = [0; BLOCK_SIZE];
    block[0] = CCM_COUNTER_FLAGS;
    block[1..1 + NONCE_SIZE].copy_from_slice(nonce);
    block[14..].copy_from_slice(&counter.to_be_bytes());
    cipher.encrypt_block(&block)
}

fn xor(left: &[u8; BLOCK_SIZE], right: &[u8; BLOCK_SIZE]) -> [u8; BLOCK_SIZE] {
    core::array::from_fn(|index| left[index] ^ right[index])
}
