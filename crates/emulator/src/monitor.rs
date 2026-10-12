// SPDX-License-Identifier: GPL-3.0-only
//! The command language: the subset of Renode's monitor that the window, the tools and NeoTime's
//! tests send, so they drive this emulator unchanged.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use pinetime_core::time::{cycles_from_nanoseconds, nanoseconds_from_cycles};
use pinetime_core::AccessSize;

use crate::images::{binary_pieces, elf_pieces, hex_pieces, Piece};
use crate::machine::Machine;
use crate::parts;
use crate::sharing::{FrameFile, StatusFile};

// ============================================================================
// Session
// ============================================================================

pub const EXTERNAL_FLASH_ADDRESS: u32 = 0x6000_0000;
const EXTERNAL_FLASH_END: u32 = 0x6040_0000;

/// Monitor addresses from 0x6000_0000 reach the 4 MB external flash, as in the Renode platform.
fn is_external_flash(address: u32) -> bool {
    (EXTERNAL_FLASH_ADDRESS..EXTERNAL_FLASH_END).contains(&address)
}

/// One watch and everything the commands set up around it.
pub struct Monitor {
    pub machine: Machine,
    pub started: bool,
    pub quit_requested: bool,
    pub frame_file: Option<FrameFile>,
    pub status_file: Option<StatusFile>,
    watchpoint_logs: Vec<WatchpointLog>,
}

struct WatchpointLog {
    address: u32,
    path: PathBuf,
}

pub type Reply = Result<String, String>;

impl Monitor {
    pub fn new() -> Monitor {
        Monitor {
            machine: Machine::new(),
            started: false,
            quit_requested: false,
            frame_file: None,
            status_file: None,
            watchpoint_logs: Vec::new(),
        }
    }

    /// Runs one command line and returns what it prints.
    pub fn execute(&mut self, line: &str) -> Reply {
        let line = self.substitute_backticks(line.trim())?;
        let words = split_words(&line)?;
        let Some((first, rest)) = words.split_first() else {
            return Ok(String::new());
        };
        if first.starts_with('#') || (first.starts_with(':') && first.ends_with(':')) {
            return Ok(String::new());
        }
        if first.starts_with('$') {
            return Ok(String::new());
        }
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();

        match first.as_str() {
            "include" | "i" => self.include(argument(&rest, 0)?),
            "mach" => self.mach(&rest),
            "machine" => self.machine_command(&rest),
            "emulation" => self.emulation(&rest),
            "start" | "s" => {
                self.started = true;
                Ok(String::new())
            }
            "pause" | "p" => {
                self.started = false;
                Ok(String::new())
            }
            "quit" | "q" => {
                self.quit_requested = true;
                Ok(String::new())
            }
            "logLevel" | "showAnalyzer" | "using" => Ok(String::new()),
            "sysbus" => self.sysbus(&rest),
            "cpu" | "sysbus.cpu" => self.cpu(&rest),
            part => self.part(part, &rest),
        }
    }

    /// Runs every line of a script file; Renode scripts that build the watch are kept as such.
    pub fn include(&mut self, target: &str) -> Reply {
        let path = path_argument(target);
        let text = fs::read_to_string(&path)
            .map_err(|error| format!("could not read {}: {error}", path.display()))?;
        let mut output = String::new();
        for line in text.lines() {
            let reply = self.execute(line)?;
            if !reply.is_empty() {
                output.push_str(&reply);
                output.push('\n');
            }
        }
        Ok(output.trim_end().to_string())
    }

    fn substitute_backticks(&mut self, line: &str) -> Result<String, String> {
        let mut result = String::new();
        let mut remaining = line;
        while let Some(start) = remaining.find('`') {
            let after = &remaining[start + 1..];
            let end = after.find('`').ok_or("a backtick is not closed")?;
            result.push_str(&remaining[..start]);
            result.push_str(self.execute(&after[..end])?.trim());
            remaining = &after[end + 1..];
        }
        result.push_str(remaining);
        Ok(result)
    }

    // ------------------------------------------------------------------------
    // Machine and time
    // ------------------------------------------------------------------------

    fn mach(&mut self, words: &[&str]) -> Reply {
        match words.first().copied() {
            Some("create") => {
                *self = Monitor::new();
                Ok(String::new())
            }
            Some("clear") | Some("set") => Ok(String::new()),
            _ => Err("usage: mach create".into()),
        }
    }

    fn machine_command(&mut self, words: &[&str]) -> Reply {
        match words.first().copied() {
            Some("Reset") => {
                self.machine.power_on();
                Ok(String::new())
            }
            Some("LoadPlatformDescription") | Some("LoadPlatformDescriptionFromString") => Ok(String::new()),
            _ => Err(format!("unknown machine command: {}", words.join(" "))),
        }
    }

    fn emulation(&mut self, words: &[&str]) -> Reply {
        match words.first().copied() {
            Some("RunFor") => {
                let nanoseconds = parse_duration(argument(words, 1)?)?;
                let target = self.machine.now() + cycles_from_nanoseconds(nanoseconds);
                self.run_until(target);
                Ok(String::new())
            }
            Some("SetGlobalQuantum") | Some("SetAdvanceImmediately") | Some("CreateBLEMedium") => {
                Ok(String::new())
            }
            Some("GetTimeSourceInfo") | Some("ElapsedTime") => Ok(format!(
                "{:.6} s",
                nanoseconds_from_cycles(self.machine.now()) as f64 / 1e9
            )),
            _ => Err(format!("unknown emulation command: {}", words.join(" "))),
        }
    }

    /// Runs the machine to `target`, writing every watchpoint hit to its log on the way.
    pub fn run_until(&mut self, target: u64) {
        while self.machine.now() < target {
            self.machine.run_until(target);
            self.write_watchpoint_logs();
        }
        self.write_watchpoint_logs();
    }

    fn write_watchpoint_logs(&mut self) {
        for hit in self.machine.take_watchpoint_hits() {
            for log in self
                .watchpoint_logs
                .iter()
                .filter(|log| log.address == hit.address)
            {
                if let Ok(mut file) = fs::OpenOptions::new().create(true).append(true).open(&log.path) {
                    let _ = writeln!(
                        file,
                        "{:x} {:x} {:x}",
                        hit.address, hit.program_counter, hit.value
                    );
                }
            }
        }
    }

    // ------------------------------------------------------------------------
    // Memory
    // ------------------------------------------------------------------------

    fn sysbus(&mut self, words: &[&str]) -> Reply {
        let command = argument(words, 0)?;
        match command {
            "LoadBinary" => {
                let path = path_argument(argument(words, 1)?);
                let address = parse_number(argument(words, 2)?)?;
                let bytes = read_file(&path)?;
                self.load_pieces(&binary_pieces(bytes, address))
            }
            "LoadELF" => {
                let path = path_argument(argument(words, 1)?);
                let pieces =
                    elf_pieces(&read_file(&path)?).map_err(|error| format!("{}: {error}", path.display()))?;
                self.load_pieces(&pieces)
            }
            "LoadHEX" => {
                let path = path_argument(argument(words, 1)?);
                let text =
                    fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))?;
                let pieces = hex_pieces(&text).map_err(|error| format!("{}: {error}", path.display()))?;
                self.load_pieces(&pieces)
            }
            "ReadDoubleWord" => self.read_value(words, AccessSize::Word),
            "ReadWord" => self.read_value(words, AccessSize::Halfword),
            "ReadByte" => self.read_value(words, AccessSize::Byte),
            "WriteDoubleWord" => self.write_value(words, AccessSize::Word),
            "WriteWord" => self.write_value(words, AccessSize::Halfword),
            "WriteByte" => self.write_value(words, AccessSize::Byte),
            "ReadBytes" => {
                let address = parse_number(argument(words, 1)?)?;
                let count = parse_number(argument(words, 2)?)?;
                let bytes = self.read_bytes(address, count)?;
                Ok(format_byte_list(&bytes))
            }
            "AddWatchpointLog" => {
                let address = parse_number(argument(words, 1)?)?;
                let path = path_argument(argument(words, 4)?);
                self.machine.chip.add_watchpoint(address, 4);
                self.watchpoint_logs.push(WatchpointLog { address, path });
                Ok(String::new())
            }
            "LogAllPeripheralsAccess" => {
                crate::machine::trace_peripherals(argument(words, 1).unwrap_or("true") == "true");
                Ok(String::new())
            }
            "LogSpiTraffic" => {
                pinetime_board::board::trace_spi(argument(words, 1).unwrap_or("true") == "true");
                Ok(String::new())
            }
            "LogPeripheralAccess" => Ok(String::new()),
            _ => Err(format!("unknown sysbus command: {command}")),
        }
    }

    fn load_pieces(&mut self, pieces: &[Piece]) -> Reply {
        for piece in pieces {
            if is_external_flash(piece.address) {
                let offset = (piece.address - EXTERNAL_FLASH_ADDRESS) as usize;
                parts::load_external_flash(&mut self.machine, offset, &piece.bytes)?;
            } else {
                self.write_memory(piece.address, &piece.bytes)?;
            }
        }
        Ok(String::new())
    }

    fn write_memory(&mut self, address: u32, bytes: &[u8]) -> Result<(), String> {
        for (index, byte) in bytes.iter().enumerate() {
            let at = address.wrapping_add(index as u32);
            self.machine
                .write_for_debugger(at, *byte as u32, AccessSize::Byte)
                .map_err(|_| format!("nothing to write at {at:#010x}"))?;
        }
        Ok(())
    }

    fn read_bytes(&mut self, address: u32, count: u32) -> Result<Vec<u8>, String> {
        if is_external_flash(address) {
            let offset = (address - EXTERNAL_FLASH_ADDRESS) as usize;
            return parts::read_external_flash(&self.machine, offset, count as usize);
        }
        (0..count)
            .map(|index| {
                let at = address.wrapping_add(index);
                self.machine
                    .read_for_debugger(at, AccessSize::Byte)
                    .map(|value| value as u8)
                    .map_err(|_| format!("nothing to read at {at:#010x}"))
            })
            .collect()
    }

    fn read_value(&mut self, words: &[&str], size: AccessSize) -> Reply {
        let address = parse_number(argument(words, 1)?)?;
        let value = if is_external_flash(address) {
            let bytes = self.read_bytes(address, size.bytes())?;
            bytes
                .iter()
                .rev()
                .fold(0u32, |value, byte| value << 8 | *byte as u32)
        } else {
            self.machine
                .read_for_debugger(address, size)
                .map_err(|_| format!("nothing to read at {address:#010x}"))?
        };
        Ok(match size {
            AccessSize::Word => format!("0x{value:08X}"),
            AccessSize::Halfword => format!("0x{value:04X}"),
            AccessSize::Byte => format!("0x{value:02X}"),
        })
    }

    fn write_value(&mut self, words: &[&str], size: AccessSize) -> Reply {
        let address = parse_number(argument(words, 1)?)?;
        let value = parse_number(argument(words, 2)?)?;
        if is_external_flash(address) {
            let bytes = value.to_le_bytes();
            let offset = (address - EXTERNAL_FLASH_ADDRESS) as usize;
            parts::load_external_flash(&mut self.machine, offset, &bytes[..size.bytes() as usize])?;
            return Ok(String::new());
        }
        self.machine
            .write_for_debugger(address, value, size)
            .map_err(|_| format!("nothing to write at {address:#010x}"))?;
        Ok(String::new())
    }

    // ------------------------------------------------------------------------
    // CPU
    // ------------------------------------------------------------------------

    fn cpu(&mut self, words: &[&str]) -> Reply {
        let command = argument(words, 0)?;
        let value = words.get(1).map(|text| parse_number(text)).transpose()?;
        if value.is_some() && matches!(command, "PC" | "SP" | "VectorTableOffset") {
            self.machine.keep_cpu_registers();
        }
        let cpu = &mut self.machine.cpu;
        let shown = match (command, value) {
            ("PC", None) => cpu.pc(),
            ("PC", Some(value)) => {
                cpu.set_pc(value);
                return Ok(String::new());
            }
            ("SP", None) => cpu.sp(),
            ("SP", Some(value)) => {
                cpu.set_sp(value);
                return Ok(String::new());
            }
            ("VectorTableOffset", None) => cpu.vector_table_offset(),
            ("VectorTableOffset", Some(value)) => {
                cpu.set_vector_table_offset(value);
                return Ok(String::new());
            }
            ("Step", count) => {
                let mut lines = Vec::new();
                for _ in 0..count.unwrap_or(1) {
                    let reason = self.machine.step();
                    let cpu = &self.machine.cpu;
                    lines.push(format!(
                        "{:#x} sp {:#x} xpsr {:#x} {reason:?}",
                        cpu.pc(),
                        cpu.sp(),
                        cpu.xpsr()
                    ));
                }
                return Ok(lines.join("\n"));
            }
            ("ExecutedInstructions", _) => return Ok(format!("0x{:016X}", cpu.executed_instructions())),
            ("IsHalted", _) => return Ok(if cpu.is_sleeping() { "True" } else { "False" }.into()),
            ("GetRegister", Some(number)) => cpu.register(number as usize),
            _ => return Err(format!("unknown cpu command: {}", words.join(" "))),
        };
        Ok(format!("0x{shown:X}"))
    }

    // ------------------------------------------------------------------------
    // Watch parts
    // ------------------------------------------------------------------------

    fn part(&mut self, path: &str, words: &[&str]) -> Reply {
        let name = path.rsplit('.').next().unwrap_or(path);
        let command = argument(words, 0)?;
        let arguments = &words[1..];
        match (name, command) {
            ("lcd", "ShareFramesThrough") => {
                let path = path_argument(argument(arguments, 0)?);
                let (width, height) = parts::visible_size(&self.machine);
                let file = FrameFile::create(&path, width, height).map_err(|error| error.to_string())?;
                self.frame_file = Some(file);
                self.share_now();
                Ok(String::new())
            }
            ("windowBridge", "ShareStatusThrough") => {
                let path = path_argument(argument(arguments, 0)?);
                self.status_file = Some(StatusFile::create(&path).map_err(|error| error.to_string())?);
                self.share_now();
                Ok(String::new())
            }
            _ => parts::run(&mut self.machine, name, command, arguments),
        }
    }

    /// Writes the screen and the status to their shared files, if the window asked for them.
    pub fn share_now(&mut self) {
        if let Some(file) = self.frame_file.as_mut() {
            let _ = parts::write_frame(&self.machine, file);
        }
        if let Some(file) = self.status_file.as_mut() {
            let status = parts::status(&self.machine, !self.started);
            let _ = file.write(&status);
        }
    }
}

impl Default for Monitor {
    fn default() -> Monitor {
        Monitor::new()
    }
}

// ============================================================================
// Words and values
// ============================================================================

/// Splits a line into words, keeping quoted text whole and dropping the quotes.
fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut characters = line.chars();
    while let Some(character) = characters.next() {
        match character {
            '"' => {
                in_word = true;
                loop {
                    match characters.next() {
                        Some('\\') => current.extend(characters.next()),
                        Some('"') => break,
                        Some(other) => current.push(other),
                        None => return Err("a quote is not closed".into()),
                    }
                }
            }
            whitespace if whitespace.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut current));
                    in_word = false;
                }
            }
            other => {
                in_word = true;
                current.push(other);
            }
        }
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

fn argument<'a>(words: &[&'a str], index: usize) -> Result<&'a str, String> {
    words
        .get(index)
        .copied()
        .ok_or_else(|| format!("missing argument {}", index + 1))
}

/// A file path, written with Renode's leading `@` or without it.
fn path_argument(text: &str) -> PathBuf {
    PathBuf::from(text.strip_prefix('@').unwrap_or(text))
}

pub fn parse_number(text: &str) -> Result<u32, String> {
    let text = text.trim();
    let parsed = if let Some(hex) = text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
        u64::from_str_radix(&hex.replace('_', ""), 16)
    } else {
        text.replace('_', "").parse::<u64>()
    };
    parsed
        .map(|value| value as u32)
        .map_err(|_| format!("not a number: {text}"))
}

/// Reads Renode's time forms: seconds ("3", "0.013") or "hh:mm:ss.fraction".
pub fn parse_duration(text: &str) -> Result<u64, String> {
    let parts: Vec<&str> = text.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [seconds] => ("0", "0", *seconds),
        [minutes, seconds] => ("0", *minutes, *seconds),
        [hours, minutes, seconds] => (*hours, *minutes, *seconds),
        _ => return Err(format!("not a time: {text}")),
    };
    let whole = |value: &str| value.parse::<u64>().map_err(|_| format!("not a time: {text}"));
    let (whole_seconds, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let fraction_nanoseconds = if fraction.is_empty() {
        0
    } else {
        let digits: String = fraction.chars().chain(std::iter::repeat('0')).take(9).collect();
        whole(&digits)?
    };
    let total_seconds = whole(hours)? * 3600 + whole(minutes)? * 60 + whole(whole_seconds)?;
    Ok(total_seconds * 1_000_000_000 + fraction_nanoseconds)
}

fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|error| format!("could not read {}: {error}", path.display()))
}

/// Prints bytes the way Renode's ReadBytes does, sixteen to a line.
fn format_byte_list(bytes: &[u8]) -> String {
    let lines: Vec<String> = bytes
        .chunks(16)
        .map(|chunk| {
            chunk
                .iter()
                .map(|byte| format!("0x{byte:02X}"))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .collect();
    format!("[\n{}\n]", lines.join(",\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_words_and_keeps_quoted_text_whole() {
        assert_eq!(
            split_words(r#"emulation RunFor "00:00:01.5"  lcd SaveScreenshot @/tmp/a b"#).unwrap(),
            [
                "emulation",
                "RunFor",
                "00:00:01.5",
                "lcd",
                "SaveScreenshot",
                "@/tmp/a",
                "b"
            ]
        );
        assert!(split_words(r#"say "unclosed"#).is_err());
    }

    #[test]
    fn reads_numbers_in_hex_and_decimal() {
        assert_eq!(parse_number("0x4001E504"), Ok(0x4001_E504));
        assert_eq!(parse_number("4096"), Ok(4096));
        assert!(parse_number("twelve").is_err());
    }

    #[test]
    fn reads_renode_time_forms() {
        assert_eq!(parse_duration("3"), Ok(3_000_000_000));
        assert_eq!(parse_duration("0.013"), Ok(13_000_000));
        assert_eq!(parse_duration("00:00:02.5"), Ok(2_500_000_000));
        assert_eq!(parse_duration("01:02:03"), Ok(3_723_000_000_000));
        assert!(parse_duration("soon").is_err());
    }

    #[test]
    fn prints_bytes_in_a_form_the_tests_parse() {
        let text = format_byte_list(&[0x00, 0xAB, 0x10]);
        assert_eq!(text, "[\n0x00, 0xAB, 0x10\n]");
    }
}
