// SPDX-License-Identifier: GPL-3.0-only
//! A flash-and-RAM test bus, an assembler wrapper around arm-none-eabi-as, and a harness that
//! runs a snippet with a generated vector table.

use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

use pinetime_core::{AccessSize, Bus, BusError};

use crate::{CortexM4, StopReason};

// ============================================================================
// Test bus
// ============================================================================

pub const FLASH_SIZE: usize = 0x8_0000;
pub const RAM_START: u32 = 0x2000_0000;
pub const RAM_SIZE: usize = 0x1_0000;

pub struct TestBus {
    pub flash: Vec<u8>,
    pub ram: Vec<u8>,
    pub lines: u64,
    pub lines_changed: bool,
    pub advanced_cycles: u64,
    /// `stop_requested` turns true once `advanced_cycles` reaches this.
    pub stop_at_cycle: Option<u64>,
    /// Wait states each instruction word fetched from flash costs.
    pub flash_fetch_wait: u32,
    /// Wait states each data access to flash costs.
    pub flash_data_wait: u32,
}

impl TestBus {
    pub fn new() -> TestBus {
        TestBus {
            flash: vec![0xFF; FLASH_SIZE],
            ram: vec![0; RAM_SIZE],
            lines: 0,
            lines_changed: false,
            advanced_cycles: 0,
            stop_at_cycle: None,
            flash_fetch_wait: 0,
            flash_data_wait: 0,
        }
    }

    pub fn with_program(program: &[u8]) -> TestBus {
        let mut bus = TestBus::new();
        bus.flash[..program.len()].copy_from_slice(program);
        bus
    }

    pub fn set_lines(&mut self, lines: u64) {
        self.lines = lines;
        self.lines_changed = true;
    }

    fn memory(&mut self, address: u32, size: AccessSize) -> Result<&mut [u8], BusError> {
        let bytes = size.bytes() as usize;
        let address = address as usize;
        if address + bytes <= FLASH_SIZE {
            return Ok(&mut self.flash[address..address + bytes]);
        }
        let offset = address.wrapping_sub(RAM_START as usize);
        if offset + bytes <= RAM_SIZE {
            return Ok(&mut self.ram[offset..offset + bytes]);
        }
        Err(BusError::Unmapped)
    }

    pub fn read_word(&mut self, address: u32) -> u32 {
        self.read(address, AccessSize::Word).unwrap()
    }
}

impl Bus for TestBus {
    #[inline]
    fn read(&mut self, address: u32, size: AccessSize) -> Result<u32, BusError> {
        let bytes = self.memory(address, size)?;
        Ok(match size {
            AccessSize::Byte => bytes[0] as u32,
            AccessSize::Halfword => u16::from_le_bytes([bytes[0], bytes[1]]) as u32,
            AccessSize::Word => u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]),
        })
    }

    #[inline]
    fn write(&mut self, address: u32, value: u32, size: AccessSize) -> Result<(), BusError> {
        let bytes = self.memory(address, size)?;
        match size {
            AccessSize::Byte => bytes[0] = value as u8,
            AccessSize::Halfword => bytes.copy_from_slice(&(value as u16).to_le_bytes()),
            AccessSize::Word => bytes.copy_from_slice(&value.to_le_bytes()),
        }
        Ok(())
    }

    #[inline]
    fn fetch(&mut self, address: u32) -> Result<u16, BusError> {
        let address = address as usize;
        if address + 2 <= FLASH_SIZE {
            return Ok(u16::from_le_bytes([self.flash[address], self.flash[address + 1]]));
        }
        let offset = address.wrapping_sub(RAM_START as usize);
        if offset + 2 <= RAM_SIZE {
            return Ok(u16::from_le_bytes([self.ram[offset], self.ram[offset + 1]]));
        }
        Err(BusError::Unmapped)
    }

    fn fetch_wait_cycles(&mut self, address: u32) -> u32 {
        match (address as usize) < FLASH_SIZE {
            true => self.flash_fetch_wait,
            false => 0,
        }
    }

    fn data_wait_cycles(&mut self, address: u32) -> u32 {
        match (address as usize) < FLASH_SIZE {
            true => self.flash_data_wait,
            false => 0,
        }
    }

    fn advance(&mut self, cycles: u32) {
        self.advanced_cycles += cycles as u64;
    }

    fn interrupt_lines(&mut self) -> Option<u64> {
        if self.lines_changed {
            self.lines_changed = false;
            Some(self.lines)
        } else {
            None
        }
    }

    fn stop_requested(&self) -> bool {
        self.stop_at_cycle
            .is_some_and(|cycle| self.advanced_cycles >= cycle)
    }
}

// ============================================================================
// Assembler
// ============================================================================

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

/// Assembles Thumb `source` linked at address 0 and returns the raw image.
pub fn assemble(source: &str) -> Vec<u8> {
    let directory: PathBuf = std::env::temp_dir().join(format!(
        "pinetime-cortex-m4-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&directory).unwrap();
    let source_path = directory.join("program.s");
    let object_path = directory.join("program.o");
    let linked_path = directory.join("program.elf");
    let binary_path = directory.join("program.bin");
    std::fs::write(&source_path, source).unwrap();
    let run = |program: &str, arguments: &[&std::ffi::OsStr]| {
        let output = Command::new(program)
            .args(arguments)
            .output()
            .unwrap_or_else(|error| panic!("{program} could not start (run inside `nix develop`): {error}"));
        assert!(
            output.status.success(),
            "{program} failed:\n{}\nsource:\n{source}",
            String::from_utf8_lossy(&output.stderr)
        );
    };
    run(
        "arm-none-eabi-as",
        &[
            "-mcpu=cortex-m4".as_ref(),
            "-mthumb".as_ref(),
            "-mfpu=fpv4-sp-d16".as_ref(),
            "-mfloat-abi=hard".as_ref(),
            "-o".as_ref(),
            object_path.as_os_str(),
            source_path.as_os_str(),
        ],
    );
    run(
        "arm-none-eabi-ld",
        &[
            "-Ttext=0".as_ref(),
            "--entry=0".as_ref(),
            "-o".as_ref(),
            linked_path.as_os_str(),
            object_path.as_os_str(),
        ],
    );
    run(
        "arm-none-eabi-objcopy",
        &[
            "-O".as_ref(),
            "binary".as_ref(),
            linked_path.as_os_str(),
            binary_path.as_os_str(),
        ],
    );
    let image = std::fs::read(&binary_path).unwrap();
    let _ = std::fs::remove_dir_all(&directory);
    image
}

/// Wraps `body` at 0x100 with a vector table pointing at its `vector_<n>` labels or a BKPT.
pub fn program(body: &str) -> String {
    let mut source = String::from(
        ".syntax unified\n.cpu cortex-m4\n.fpu fpv4-sp-d16\n.thumb\n.text\n.word 0x20010000\n.word start\n",
    );
    for number in 2..64 {
        if body.contains(&format!("vector_{number}:")) {
            source.push_str(&format!(".word vector_{number}\n"));
        } else {
            source.push_str(".word unhandled\n");
        }
    }
    source.push_str(".thumb_func\nstart:\n");
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("vector_") && trimmed.ends_with(':') {
            source.push_str(".thumb_func\n");
        }
        source.push_str(line);
        source.push('\n');
    }
    source.push_str(".thumb_func\nunhandled:\n    bkpt #0xEE\n.ltorg\n");
    source
}

// ============================================================================
// Harness
// ============================================================================

pub struct Machine {
    pub cpu: CortexM4,
    pub bus: TestBus,
}

impl Machine {
    /// Assembles `body` inside `program` and resets the core into it.
    pub fn new(body: &str) -> Machine {
        let image = assemble(&program(body));
        let mut bus = TestBus::with_program(&image);
        let mut cpu = CortexM4::new();
        cpu.reset(&mut bus);
        Machine { cpu, bus }
    }

    /// Runs until something other than the cycle budget stops the core.
    pub fn run(&mut self) -> StopReason {
        for _ in 0..1000 {
            let outcome = self.cpu.run(&mut self.bus, 1_000_000);
            if outcome.reason != StopReason::Deadline {
                return outcome.reason;
            }
        }
        panic!("program did not stop; pc {:#x}", self.cpu.pc());
    }

    /// Runs to a BKPT and checks it is not the unhandled-exception one.
    pub fn run_to_breakpoint(&mut self) {
        let reason = self.run();
        assert_eq!(reason, StopReason::Breakpoint, "stopped at {:#x}", self.cpu.pc());
        let instruction = self.bus.fetch(self.cpu.pc()).unwrap();
        assert_ne!(
            instruction,
            0xBEEE,
            "an exception without a handler was taken; ipsr {}",
            self.cpu.xpsr() & 0x1FF
        );
    }

    pub fn register(&self, number: usize) -> u32 {
        self.cpu.register(number)
    }

    /// The N, Z, C and V flags as a 4-bit number.
    pub fn flags(&self) -> u32 {
        self.cpu.xpsr() >> 28
    }
}

/// Runs `body` to its BKPT and returns the machine.
pub fn run(body: &str) -> Machine {
    let mut machine = Machine::new(body);
    machine.run_to_breakpoint();
    machine
}
