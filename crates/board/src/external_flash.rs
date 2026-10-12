// SPDX-License-Identifier: GPL-3.0-only
//! The PineTime's 4 MB XTX XT25F32B SPI NOR flash, with its erase and program timing.

use std::io;
use std::path::Path;

use pinetime_core::time::{cycles_from_microseconds, cycles_from_milliseconds};
use pinetime_core::Cycle;

// ============================================================================
// Commands and sizes
// ============================================================================

const WRSR: u8 = 0x01;
const PP: u8 = 0x02;
const READ: u8 = 0x03;
const WRDI: u8 = 0x04;
const RDSR: u8 = 0x05;
const WREN: u8 = 0x06;
const FAST_READ: u8 = 0x0B;
const SE: u8 = 0x20;
const RDSCUR: u8 = 0x2B;
const RDSR2: u8 = 0x35;
const RUID: u8 = 0x4B;
const BE32: u8 = 0x52;
const RDSFDP: u8 = 0x5A;
const CE: u8 = 0x60;
const RSTEN: u8 = 0x66;
const REMS: u8 = 0x90;
const RST: u8 = 0x99;
const RDID: u8 = 0x9F;
const RDP: u8 = 0xAB;
const DP: u8 = 0xB9;
const CE_ALTERNATIVE: u8 = 0xC7;
const BE64: u8 = 0xD8;

const STATUS_WRITE_IN_PROGRESS: u8 = 0x01;
const STATUS_WRITE_ENABLE_LATCH: u8 = 0x02;
const ERASED_BYTE: u8 = 0xFF;
const IDLE_BUS: u8 = 0xFF;

pub const SIZE: usize = 4 * 1024 * 1024;
const PAGE_SIZE: usize = 256;
const SECTOR_SIZE: usize = 4 * 1024;
const BLOCK_32K_SIZE: usize = 32 * 1024;
const BLOCK_64K_SIZE: usize = 64 * 1024;
const STORED_COMMAND_BYTES: usize = 4;

pub const JEDEC_ID: [u8; 3] = [0x0B, 0x40, 0x16];
const MANUFACTURER_DEVICE_ID: [u8; 2] = [0x0B, 0x15];
const UNIQUE_ID: [u8; 16] = [
    0x50, 0x49, 0x4E, 0x45, 0x54, 0x49, 0x4D, 0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01,
];

/// A JEDEC SFDP header with one basic parameter table: 32 Mbit, 4 KB, 32 KB and 64 KB erases.
const SFDP_TABLE_ADDRESS: usize = 0x30;
const SFDP_HEADER: [u8; 16] = [
    b'S', b'F', b'D', b'P', 0x00, 0x01, 0x00, 0xFF, 0x00, 0x00, 0x01, 0x09, 0x30, 0x00, 0x00, 0xFF,
];
const SFDP_BASIC_TABLE: [u8; 36] = [
    0xE5, 0x20, 0x00, 0xFF, 0xFF, 0xFF, 0xFF, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x20, 0x0F, 0x52, 0x10, 0xD8,
    0x00, 0x00,
];

/// Typical times from the XT25F32B datasheet.
fn page_program_time() -> Cycle {
    cycles_from_microseconds(500)
}

fn sector_erase_time() -> Cycle {
    cycles_from_milliseconds(45)
}

fn block_32k_erase_time() -> Cycle {
    cycles_from_milliseconds(150)
}

fn block_64k_erase_time() -> Cycle {
    cycles_from_milliseconds(250)
}

fn chip_erase_time() -> Cycle {
    cycles_from_milliseconds(10_000)
}

// ============================================================================
// Flash state
// ============================================================================

/// Most commands take effect when the chip select lifts at the end of the command.
pub struct ExternalFlash {
    memory: Box<[u8]>,
    command: [u8; STORED_COMMAND_BYTES],
    command_length: usize,
    page_program_bytes: [u8; PAGE_SIZE],
    page_program_byte_count: usize,
    deep_powered_down: bool,
    write_enabled: bool,
    busy_until: Cycle,
    status_low_writable: u8,
    status_high: u8,
    security_register: u8,
    reset_enabled: bool,
}

impl Default for ExternalFlash {
    fn default() -> ExternalFlash {
        ExternalFlash::new()
    }
}

impl ExternalFlash {
    /// A flash that starts erased, as a new chip does.
    pub fn new() -> ExternalFlash {
        ExternalFlash {
            memory: vec![ERASED_BYTE; SIZE].into_boxed_slice(),
            command: [0; STORED_COMMAND_BYTES],
            command_length: 0,
            page_program_bytes: [ERASED_BYTE; PAGE_SIZE],
            page_program_byte_count: 0,
            deep_powered_down: false,
            write_enabled: false,
            busy_until: 0,
            status_low_writable: 0,
            status_high: 0,
            security_register: 0,
            reset_enabled: false,
        }
    }

    /// Forgets the command in flight and the latches, and keeps the contents.
    pub fn reset(&mut self) {
        self.clear_command();
        self.deep_powered_down = false;
        self.write_enabled = false;
        self.busy_until = 0;
    }

    pub fn deep_powered_down(&self) -> bool {
        self.deep_powered_down
    }

    pub fn write_enabled(&self) -> bool {
        self.write_enabled
    }

    pub fn busy(&self, now: Cycle) -> bool {
        now < self.busy_until
    }

    /// The cycle the current program or erase finishes.
    pub fn busy_until(&self) -> Cycle {
        self.busy_until
    }

    // ========================================================================
    // Contents
    // ========================================================================

    pub fn contents(&self) -> &[u8] {
        &self.memory
    }

    pub fn contents_mut(&mut self) -> &mut [u8] {
        &mut self.memory
    }

    /// Copies a raw image, such as a dump from a real watch, to the start of the flash.
    pub fn load_image(&mut self, image: &[u8]) {
        let length = image.len().min(SIZE);
        self.memory[..length].copy_from_slice(&image[..length]);
    }

    pub fn load_image_file(&mut self, path: &Path) -> io::Result<()> {
        let image = std::fs::read(path)?;
        self.load_image(&image);
        Ok(())
    }

    pub fn save_image_file(&self, path: &Path) -> io::Result<()> {
        std::fs::write(path, &self.memory)
    }

    // ========================================================================
    // SPI
    // ========================================================================

    #[inline]
    pub fn transfer(&mut self, byte: u8, now: Cycle) -> u8 {
        let position = self.command_length;
        if position < STORED_COMMAND_BYTES {
            self.command[position] = byte;
        }
        self.command_length = position.saturating_add(1);
        let opcode = self.command[0];

        if self.deep_powered_down && opcode != RDP {
            return IDLE_BUS;
        }

        match opcode {
            READ if position >= 4 => self.memory[self.address_plus(position - 4)],
            FAST_READ if position >= 5 => self.memory[self.address_plus(position - 5)],
            RDSR => self.status_low(now),
            RDSR2 => self.status_high,
            RDSCUR => self.security_register,
            RDID if (1..=3).contains(&position) => JEDEC_ID[position - 1],
            REMS if position >= 4 => MANUFACTURER_DEVICE_ID[(position - 4) % 2],
            RDP if position >= 4 => JEDEC_ID[2],
            RUID if position >= 5 => UNIQUE_ID[(position - 5) % UNIQUE_ID.len()],
            RDSFDP if position >= 5 => sfdp_byte(self.address() + position - 5),
            PP if position >= 4 => {
                let offset = (self.address() + self.page_program_byte_count) % PAGE_SIZE;
                self.page_program_bytes[offset] &= byte;
                self.page_program_byte_count += 1;
                IDLE_BUS
            }
            _ => IDLE_BUS,
        }
    }

    /// The chip select lifted: finishes the command it ended.
    pub fn finish_transmission(&mut self, now: Cycle) {
        if self.command_length == 0 {
            return;
        }

        let opcode = self.command[0];
        if self.deep_powered_down {
            if opcode == RDP {
                self.deep_powered_down = false;
            }
            self.clear_command();
            return;
        }

        match opcode {
            WREN => self.write_enabled = true,
            WRDI => self.write_enabled = false,
            DP => self.deep_powered_down = true,
            PP => self.program_page(now),
            SE => self.erase(SECTOR_SIZE, sector_erase_time(), now),
            BE32 => self.erase(BLOCK_32K_SIZE, block_32k_erase_time(), now),
            BE64 => self.erase(BLOCK_64K_SIZE, block_64k_erase_time(), now),
            CE | CE_ALTERNATIVE if self.command_length == 1 => {
                self.erase_range(0, SIZE, chip_erase_time(), now)
            }
            WRSR if self.write_enabled && !self.busy(now) && self.command_length >= 2 => {
                self.status_low_writable = self.command[1] & 0xFC;
                if self.command_length >= 3 {
                    self.status_high = self.command[2];
                }
                self.write_enabled = false;
            }
            RSTEN => {
                self.reset_enabled = true;
                self.command_length = 0;
                return;
            }
            RST if self.reset_enabled => self.write_enabled = false,
            _ => {}
        }

        self.reset_enabled = false;
        self.clear_command();
    }

    fn clear_command(&mut self) {
        self.command_length = 0;
        self.command = [0; STORED_COMMAND_BYTES];
        if self.page_program_byte_count != 0 {
            self.page_program_bytes = [ERASED_BYTE; PAGE_SIZE];
            self.page_program_byte_count = 0;
        }
    }

    // ========================================================================
    // Reading, programming and erasing
    // ========================================================================

    fn address(&self) -> usize {
        ((self.command[1] as usize) << 16) | ((self.command[2] as usize) << 8) | self.command[3] as usize
    }

    fn address_plus(&self, offset: usize) -> usize {
        (self.address() + offset) % SIZE
    }

    /// Programming only clears bits, and bytes past the end of the page wrap to its start.
    fn program_page(&mut self, now: Cycle) {
        if !self.write_enabled || self.busy(now) || self.command_length < 5 {
            return;
        }
        let page_start = (self.address() % SIZE) & !(PAGE_SIZE - 1);
        let page = &mut self.memory[page_start..page_start + PAGE_SIZE];
        for (stored, programmed) in page.iter_mut().zip(self.page_program_bytes.iter()) {
            *stored &= *programmed;
        }
        self.security_register = 0;
        self.start_busy(page_program_time(), now);
    }

    fn erase(&mut self, size: usize, duration: Cycle, now: Cycle) {
        if self.command_length < 4 {
            return;
        }
        let start = (self.address() % SIZE) & !(size - 1);
        self.erase_range(start, size, duration, now);
    }

    fn erase_range(&mut self, start: usize, size: usize, duration: Cycle, now: Cycle) {
        if !self.write_enabled || self.busy(now) {
            return;
        }
        self.memory[start..start + size].fill(ERASED_BYTE);
        self.security_register = 0;
        self.start_busy(duration, now);
    }

    fn start_busy(&mut self, duration: Cycle, now: Cycle) {
        self.write_enabled = false;
        self.busy_until = now + duration;
    }

    fn status_low(&self, now: Cycle) -> u8 {
        let mut status = self.status_low_writable;
        if self.busy(now) {
            status |= STATUS_WRITE_IN_PROGRESS;
        }
        if self.write_enabled {
            status |= STATUS_WRITE_ENABLE_LATCH;
        }
        status
    }
}

fn sfdp_byte(address: usize) -> u8 {
    if address < SFDP_HEADER.len() {
        SFDP_HEADER[address]
    } else if (SFDP_TABLE_ADDRESS..SFDP_TABLE_ADDRESS + SFDP_BASIC_TABLE.len()).contains(&address) {
        SFDP_BASIC_TABLE[address - SFDP_TABLE_ADDRESS]
    } else {
        IDLE_BUS
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs one command with the chip select held low, returning the bytes shifted in.
    fn run(flash: &mut ExternalFlash, bytes: &[u8], now: Cycle) -> Vec<u8> {
        let answer = bytes.iter().map(|byte| flash.transfer(*byte, now)).collect();
        flash.finish_transmission(now);
        answer
    }

    fn read(flash: &mut ExternalFlash, address: usize, count: usize, now: Cycle) -> Vec<u8> {
        let mut bytes = vec![READ, (address >> 16) as u8, (address >> 8) as u8, address as u8];
        bytes.resize(4 + count, 0);
        run(flash, &bytes, now)[4..].to_vec()
    }

    fn command_with_address(opcode: u8, address: usize) -> Vec<u8> {
        vec![opcode, (address >> 16) as u8, (address >> 8) as u8, address as u8]
    }

    #[test]
    fn reports_the_jedec_and_manufacturer_ids() {
        let mut flash = ExternalFlash::new();
        assert_eq!(
            run(&mut flash, &[RDID, 0, 0, 0, 0], 0),
            vec![0xFF, 0x0B, 0x40, 0x16, 0xFF]
        );
        assert_eq!(
            run(&mut flash, &[REMS, 0, 0, 0, 0, 0, 0], 0)[4..],
            [0x0B, 0x15, 0x0B]
        );
        assert_eq!(run(&mut flash, &[RDP, 0, 0, 0, 0], 0)[4], 0x16);
    }

    #[test]
    fn reads_the_unique_id_and_sfdp_header() {
        let mut flash = ExternalFlash::new();
        let unique = run(&mut flash, &[RUID, 0, 0, 0, 0, 0, 0, 0, 0, 0], 0);
        assert_eq!(&unique[5..], b"PINET");

        let mut sfdp = command_with_address(RDSFDP, 0);
        sfdp.resize(5 + 4, 0);
        assert_eq!(&run(&mut flash, &sfdp, 0)[5..], b"SFDP");

        let mut density = command_with_address(RDSFDP, SFDP_TABLE_ADDRESS + 4);
        density.resize(5 + 4, 0);
        assert_eq!(&run(&mut flash, &density, 0)[5..], &[0xFF, 0xFF, 0xFF, 0x01]);
    }

    #[test]
    fn starts_erased_and_reads_loaded_images_with_read_and_fast_read() {
        let mut flash = ExternalFlash::new();
        assert_eq!(read(&mut flash, 0x1000, 2, 0), vec![0xFF, 0xFF]);

        flash.load_image(&[1, 2, 3, 4]);

        assert_eq!(read(&mut flash, 1, 3, 0), vec![2, 3, 4]);
        let fast = run(&mut flash, &[FAST_READ, 0, 0, 2, 0, 0, 0], 0);
        assert_eq!(&fast[5..], &[3, 4]);
        assert_eq!(&flash.contents()[..4], &[1, 2, 3, 4]);
    }

    #[test]
    fn reading_wraps_at_the_end_of_the_flash() {
        let mut flash = ExternalFlash::new();
        flash.contents_mut()[0] = 0x42;
        assert_eq!(read(&mut flash, SIZE - 1, 2, 0), vec![0xFF, 0x42]);
    }

    #[test]
    fn page_program_needs_write_enable_and_only_clears_bits() {
        let mut flash = ExternalFlash::new();
        let mut program = command_with_address(PP, 0x100);
        program.extend_from_slice(&[0x0F, 0xF0]);

        run(&mut flash, &program, 0);
        assert_eq!(read(&mut flash, 0x100, 2, 0), vec![0xFF, 0xFF]);

        run(&mut flash, &[WREN], 0);
        assert!(flash.write_enabled());
        run(&mut flash, &program, 0);
        assert!(!flash.write_enabled());
        assert_eq!(read(&mut flash, 0x100, 2, 0), vec![0x0F, 0xF0]);

        let busy_end = flash.busy_until();
        run(&mut flash, &[WREN], busy_end);
        let mut overwrite = command_with_address(PP, 0x100);
        overwrite.extend_from_slice(&[0xF3, 0x3F]);
        run(&mut flash, &overwrite, busy_end);
        assert_eq!(read(&mut flash, 0x100, 2, busy_end), vec![0x03, 0x30]);
    }

    #[test]
    fn page_program_wraps_inside_the_page() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        let mut program = command_with_address(PP, 0x1FF);
        program.extend_from_slice(&[0x11, 0x22]);
        run(&mut flash, &program, 0);

        assert_eq!(flash.contents()[0x1FF], 0x11);
        assert_eq!(flash.contents()[0x100], 0x22);
        assert_eq!(flash.contents()[0x200], 0xFF);
    }

    #[test]
    fn status_shows_write_in_progress_until_the_busy_time_passes() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        assert_eq!(run(&mut flash, &[RDSR, 0], 0)[1], STATUS_WRITE_ENABLE_LATCH);

        run(&mut flash, &command_with_address(SE, 0x2345), 1_000);
        let finished = 1_000 + sector_erase_time();
        assert_eq!(flash.busy_until(), finished);
        assert_eq!(
            run(&mut flash, &[RDSR, 0, 0], finished - 1)[1..],
            [STATUS_WRITE_IN_PROGRESS; 2]
        );
        assert_eq!(run(&mut flash, &[RDSR, 0], finished)[1], 0);
    }

    #[test]
    fn a_busy_flash_ignores_programs_and_erases() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        run(&mut flash, &command_with_address(SE, 0), 0);
        run(&mut flash, &[WREN], 10);
        let mut program = command_with_address(PP, 0);
        program.push(0x00);
        run(&mut flash, &program, 10);

        assert_eq!(flash.contents()[0], 0xFF);
    }

    #[test]
    fn erases_sectors_blocks_and_the_whole_chip() {
        let mut flash = ExternalFlash::new();
        flash.contents_mut().fill(0);
        let mut now = 0;
        let mut erase = |flash: &mut ExternalFlash, bytes: &[u8]| {
            run(flash, &[WREN], now);
            run(flash, bytes, now);
            now = flash.busy_until();
        };

        erase(&mut flash, &command_with_address(SE, 0x1234));
        assert_eq!(flash.contents()[0x0FFF], 0x00);
        assert!(flash.contents()[0x1000..0x2000].iter().all(|byte| *byte == 0xFF));
        assert_eq!(flash.contents()[0x2000], 0x00);

        erase(&mut flash, &command_with_address(BE32, 0x9000));
        assert!(flash.contents()[0x8000..0x10000].iter().all(|byte| *byte == 0xFF));
        assert_eq!(flash.contents()[0x10000], 0x00);

        erase(&mut flash, &command_with_address(BE64, 0x2_3456));
        assert!(flash.contents()[0x2_0000..0x3_0000]
            .iter()
            .all(|byte| *byte == 0xFF));
        assert_eq!(flash.contents()[0x3_0000], 0x00);

        erase(&mut flash, &[CE]);
        assert!(flash.contents().iter().all(|byte| *byte == 0xFF));
    }

    #[test]
    fn chip_erase_with_extra_bytes_does_nothing() {
        let mut flash = ExternalFlash::new();
        flash.contents_mut()[5] = 0;
        run(&mut flash, &[WREN], 0);
        run(&mut flash, &[CE_ALTERNATIVE, 0], 0);
        assert_eq!(flash.contents()[5], 0);
        run(&mut flash, &[CE_ALTERNATIVE], 0);
        assert_eq!(flash.contents()[5], 0xFF);
    }

    #[test]
    fn write_disable_clears_the_latch() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        run(&mut flash, &[WRDI], 0);
        assert!(!flash.write_enabled());
    }

    #[test]
    fn write_status_sets_both_status_registers() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WRSR, 0xFF, 0x42], 0);
        assert_eq!(run(&mut flash, &[RDSR2, 0], 0)[1], 0);

        run(&mut flash, &[WREN], 0);
        run(&mut flash, &[WRSR, 0xFF, 0x42], 0);

        assert_eq!(run(&mut flash, &[RDSR, 0], 0)[1], 0xFC);
        assert_eq!(run(&mut flash, &[RDSR2, 0], 0)[1], 0x42);
        assert!(!flash.write_enabled());
    }

    #[test]
    fn deep_power_down_ignores_everything_but_release() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[DP], 0);
        assert!(flash.deep_powered_down());
        assert_eq!(run(&mut flash, &[RDID, 0, 0, 0], 0), vec![0xFF; 4]);
        run(&mut flash, &[WREN], 0);
        assert!(!flash.write_enabled());

        run(&mut flash, &[RDP], 0);

        assert!(!flash.deep_powered_down());
        assert_eq!(run(&mut flash, &[RDID, 0, 0, 0], 0)[1..], JEDEC_ID);
    }

    #[test]
    fn software_reset_needs_reset_enable_first() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        run(&mut flash, &[RST], 0);
        assert!(flash.write_enabled());

        run(&mut flash, &[RSTEN], 0);
        run(&mut flash, &[RST], 0);
        assert!(!flash.write_enabled());
    }

    #[test]
    fn programming_clears_the_security_register() {
        let mut flash = ExternalFlash::new();
        flash.security_register = 0x40;
        run(&mut flash, &[WREN], 0);
        let mut program = command_with_address(PP, 0);
        program.push(0);
        run(&mut flash, &program, 0);
        assert_eq!(run(&mut flash, &[RDSCUR, 0], 0)[1], 0);
    }

    #[test]
    fn reset_keeps_the_contents_and_ends_busy() {
        let mut flash = ExternalFlash::new();
        run(&mut flash, &[WREN], 0);
        run(&mut flash, &command_with_address(SE, 0), 0);
        flash.contents_mut()[0] = 7;

        flash.reset();

        assert!(!flash.busy(0));
        assert_eq!(flash.contents()[0], 7);
    }

    #[test]
    fn loads_and_saves_image_files() {
        let path = std::env::temp_dir().join(format!("xt25f32b-image-{}.bin", std::process::id()));
        std::fs::write(&path, [9, 8, 7]).unwrap();
        let mut flash = ExternalFlash::new();

        flash.load_image_file(&path).unwrap();
        flash.save_image_file(&path).unwrap();

        let saved = std::fs::read(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        assert_eq!(saved.len(), SIZE);
        assert_eq!(&saved[..4], &[9, 8, 7, 0xFF]);
    }
}
