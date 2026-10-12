// SPDX-License-Identifier: GPL-3.0-only
//! Writes the screen and the status block to the files the window maps; the layouts are in
//! frontend/pinetime_emulator/shared.py.

use std::fs::{File, OpenOptions};
use std::io;
use std::os::unix::fs::FileExt;
use std::path::Path;

// ============================================================================
// Frames
// ============================================================================

const FRAME_MAGIC: u32 = 0x4246_5450;
const STATUS_MAGIC: u32 = 0x5453_5450;
const LAYOUT_VERSION: u32 = 1;
const FRAME_HEADER_BYTES: usize = 64;

pub struct FrameSnapshot<'a> {
    pub count: u32,
    pub width: u16,
    pub height: u16,
    pub sleeping: bool,
    pub display_on: bool,
    pub in_reset: bool,
    pub inverted: bool,
    pub backlight_level: u32,
    pub rgb_pixels: &'a [u8],
}

pub struct FrameFile {
    file: File,
    sequence: u32,
    last_count: Option<u32>,
}

impl FrameFile {
    pub fn create(path: &Path, width: u16, height: u16) -> io::Result<FrameFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len((FRAME_HEADER_BYTES + width as usize * height as usize * 3) as u64)?;
        Ok(FrameFile {
            file,
            sequence: 0,
            last_count: None,
        })
    }

    /// Writes the frame unless it is the one already written.
    pub fn write(&mut self, frame: &FrameSnapshot, backlight_changed: bool) -> io::Result<()> {
        if self.last_count == Some(frame.count) && !backlight_changed {
            return Ok(());
        }
        self.last_count = Some(frame.count);

        let flags = frame.sleeping as u32
            | (frame.display_on as u32) << 1
            | (frame.in_reset as u32) << 2
            | (frame.inverted as u32) << 3;

        self.sequence = self.sequence.wrapping_add(1) | 1;
        let mut header = Vec::with_capacity(FRAME_HEADER_BYTES);
        for word in [FRAME_MAGIC, LAYOUT_VERSION, self.sequence, frame.count] {
            header.extend_from_slice(&word.to_le_bytes());
        }
        header.extend_from_slice(&frame.width.to_le_bytes());
        header.extend_from_slice(&frame.height.to_le_bytes());
        for word in [flags, frame.backlight_level, FRAME_HEADER_BYTES as u32] {
            header.extend_from_slice(&word.to_le_bytes());
        }
        header.resize(FRAME_HEADER_BYTES, 0);

        self.file.write_all_at(&header, 0)?;
        self.file
            .write_all_at(frame.rgb_pixels, FRAME_HEADER_BYTES as u64)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.file.write_all_at(&self.sequence.to_le_bytes(), 8)
    }
}

// ============================================================================
// Status
// ============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct StatusSnapshot {
    pub motor_running: bool,
    pub button_pressed: bool,
    pub charger_connected: bool,
    pub charging: bool,
    pub paused: bool,
    pub motor_starts: u32,
    pub battery_percent: i32,
    pub battery_volts: f64,
    pub emulated_microseconds: u64,
    pub steps: u32,
}

const STATUS_BYTES: usize = 44;

pub struct StatusFile {
    file: File,
    sequence: u32,
}

impl StatusFile {
    pub fn create(path: &Path) -> io::Result<StatusFile> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(path)?;
        file.set_len(STATUS_BYTES as u64)?;
        Ok(StatusFile { file, sequence: 0 })
    }

    pub fn write(&mut self, status: &StatusSnapshot) -> io::Result<()> {
        let flags = status.motor_running as u32
            | (status.button_pressed as u32) << 1
            | (status.charger_connected as u32) << 2
            | (status.charging as u32) << 3
            | (status.paused as u32) << 4;

        self.sequence = self.sequence.wrapping_add(1) | 1;
        let mut block = Vec::with_capacity(STATUS_BYTES);
        for word in [
            STATUS_MAGIC,
            LAYOUT_VERSION,
            self.sequence,
            flags,
            status.motor_starts,
        ] {
            block.extend_from_slice(&word.to_le_bytes());
        }
        block.extend_from_slice(&status.battery_percent.to_le_bytes());
        block.extend_from_slice(&status.battery_volts.to_le_bytes());
        block.extend_from_slice(&status.emulated_microseconds.to_le_bytes());
        block.extend_from_slice(&status.steps.to_le_bytes());

        self.file.write_all_at(&block, 0)?;
        self.sequence = self.sequence.wrapping_add(1);
        self.file.write_all_at(&self.sequence.to_le_bytes(), 8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pinetime-sharing-{}-{name}", std::process::id()))
    }

    #[test]
    fn the_frame_header_matches_the_window_layout() {
        let path = scratch_path("frame");
        let mut file = FrameFile::create(&path, 2, 1).unwrap();
        let pixels = [1, 2, 3, 4, 5, 6];
        let frame = FrameSnapshot {
            count: 7,
            width: 2,
            height: 1,
            sleeping: false,
            display_on: true,
            in_reset: false,
            inverted: true,
            backlight_level: 3,
            rgb_pixels: &pixels,
        };
        file.write(&frame, false).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let word = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!(word(0), FRAME_MAGIC);
        assert_eq!(word(8) % 2, 0);
        assert_eq!(word(12), 7);
        assert_eq!(&bytes[16..20], &[2, 0, 1, 0]);
        assert_eq!(word(20), 0b1010);
        assert_eq!(word(24), 3);
        assert_eq!(word(28), 64);
        assert_eq!(&bytes[64..70], &pixels);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn the_status_block_matches_the_window_layout() {
        let path = scratch_path("status");
        let mut file = StatusFile::create(&path).unwrap();
        let status = StatusSnapshot {
            motor_running: true,
            charging: true,
            motor_starts: 2,
            battery_percent: 80,
            battery_volts: 3.9,
            emulated_microseconds: 1_500_000,
            steps: 42,
            ..StatusSnapshot::default()
        };
        file.write(&status).unwrap();

        let bytes = std::fs::read(&path).unwrap();
        let word = |offset: usize| u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
        assert_eq!(word(0), STATUS_MAGIC);
        assert_eq!(word(12), 0b1001);
        assert_eq!(word(16), 2);
        assert_eq!(word(20), 80);
        assert_eq!(f64::from_le_bytes(bytes[24..32].try_into().unwrap()), 3.9);
        assert_eq!(u64::from_le_bytes(bytes[32..40].try_into().unwrap()), 1_500_000);
        assert_eq!(word(40), 42);
        std::fs::remove_file(path).unwrap();
    }
}
