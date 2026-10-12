// SPDX-License-Identifier: GPL-3.0-only
//! A host pseudo-terminal, reached through a symlink at a path the caller chooses, so tools such
//! as `btattach` can talk to an emulated UART. Only the client holds the terminal side open; once
//! it closes, a fresh pseudo-terminal takes the link's place, because the kernel keeps a line
//! discipline such as BlueZ's adapter until both sides of the old one are closed. Output sent
//! while no client is there is dropped, as on a UART with nothing listening.

use std::ffi::CStr;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::path::{Path, PathBuf};

// ============================================================================
// Pseudo-terminal
// ============================================================================

/// Output the terminal holds while nobody reads it; older bytes are dropped beyond this.
const MAXIMUM_PENDING_OUTPUT: usize = 64 * 1024;

pub struct PseudoTerminal {
    controller: File,
    terminal_path: PathBuf,
    client_present: bool,
    link: PathBuf,
    pending_output: Vec<u8>,
    dropped_output: u64,
}

impl PseudoTerminal {
    /// Opens a new pseudo-terminal and points `link` at it, replacing an old symlink there.
    pub fn open(link: &Path) -> io::Result<PseudoTerminal> {
        let (controller, terminal_path) = open_pair()?;
        point_link_at(link, &terminal_path)?;
        Ok(PseudoTerminal {
            controller,
            terminal_path,
            client_present: false,
            link: link.to_path_buf(),
            pending_output: Vec::new(),
            dropped_output: 0,
        })
    }

    // A client went away: a new pair behind the same link lets the old one close completely.
    fn replace_pair(&mut self) {
        let Ok((controller, terminal_path)) = open_pair() else {
            return;
        };
        if point_link_at(&self.link, &terminal_path).is_ok() {
            self.controller = controller;
            self.terminal_path = terminal_path;
        }
    }

    pub fn link(&self) -> &Path {
        &self.link
    }

    /// The terminal device itself, such as /dev/pts/7.
    pub fn terminal_path(&self) -> &Path {
        &self.terminal_path
    }

    /// Bytes output was too far behind to keep while nobody read the terminal.
    pub fn dropped_output(&self) -> u64 {
        self.dropped_output
    }

    // ========================================================================
    // Reading and writing
    // ========================================================================

    /// Reads what a client has written so far, up to `buffer.len()` bytes, without waiting.
    pub fn read_available(&mut self, buffer: &mut [u8]) -> usize {
        match self.controller.read(buffer) {
            Ok(count) => {
                self.client_present = true;
                count
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                self.client_present = true;
                0
            }
            // The line reads as hung up while no client holds the terminal open.
            Err(_) => {
                if self.client_present {
                    self.replace_pair();
                }
                self.client_present = false;
                self.pending_output.clear();
                0
            }
        }
    }

    /// Whether a client held the terminal open at the last read.
    pub fn client_present(&self) -> bool {
        self.client_present
    }

    /// Queues `bytes` for the client and sends as much as the terminal takes now; with no client
    /// at the last read they are dropped.
    pub fn write(&mut self, bytes: &[u8]) {
        if !self.client_present {
            self.dropped_output += bytes.len() as u64;
            return;
        }
        self.pending_output.extend_from_slice(bytes);
        if self.pending_output.len() > MAXIMUM_PENDING_OUTPUT {
            let excess = self.pending_output.len() - MAXIMUM_PENDING_OUTPUT;
            self.pending_output.drain(..excess);
            self.dropped_output += excess as u64;
        }
        self.flush();
    }

    /// Sends queued output the terminal had no room for before.
    pub fn flush(&mut self) {
        while !self.pending_output.is_empty() {
            match self.controller.write(&self.pending_output) {
                Ok(0) | Err(_) => return,
                Ok(count) => {
                    self.pending_output.drain(..count);
                }
            }
        }
    }

    pub fn has_pending_output(&self) -> bool {
        !self.pending_output.is_empty()
    }
}

impl Drop for PseudoTerminal {
    /// Removes the symlink, unless something else has replaced it meanwhile.
    fn drop(&mut self) {
        if std::fs::read_link(&self.link).is_ok_and(|target| target == self.terminal_path) {
            let _ = std::fs::remove_file(&self.link);
        }
    }
}

// ============================================================================
// Opening
// ============================================================================

/// A new pseudo-terminal pair: its controller side, which does not block, and the terminal's path.
fn open_pair() -> io::Result<(File, PathBuf)> {
    let mut controller_descriptor = -1;
    let mut terminal_descriptor = -1;
    // SAFETY: openpty only writes the two descriptors; the name, settings and size are optional.
    let result = unsafe {
        libc::openpty(
            &mut controller_descriptor,
            &mut terminal_descriptor,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: openpty succeeded, so both descriptors are open and owned by nothing else.
    let (controller, terminal) = unsafe {
        (
            File::from_raw_fd(controller_descriptor),
            File::from_raw_fd(terminal_descriptor),
        )
    };
    make_raw(&terminal)?;
    set_nonblocking(&controller)?;
    let terminal_path = terminal_name(&terminal)?;
    Ok((controller, terminal_path))
}

/// Points `link` at `target`, replacing a symlink already there but never another kind of file.
fn point_link_at(link: &Path, target: &Path) -> io::Result<()> {
    if std::fs::symlink_metadata(link).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
        std::fs::remove_file(link)?;
    }
    std::os::unix::fs::symlink(target, link)
}

// ============================================================================
// Terminal settings
// ============================================================================

fn make_raw(terminal: &File) -> io::Result<()> {
    // SAFETY: termios is plain data, filled by tcgetattr before use.
    let mut settings: libc::termios = unsafe { std::mem::zeroed() };
    // SAFETY: the descriptor is an open terminal and `settings` is valid for writing.
    if unsafe { libc::tcgetattr(terminal.as_raw_fd(), &mut settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: `settings` came from tcgetattr.
    unsafe { libc::cfmakeraw(&mut settings) };
    // SAFETY: the descriptor is an open terminal and `settings` is a valid termios.
    if unsafe { libc::tcsetattr(terminal.as_raw_fd(), libc::TCSANOW, &settings) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    // SAFETY: F_GETFL and F_SETFL only read and set the flags of an open descriptor.
    unsafe {
        let flags = libc::fcntl(file.as_raw_fd(), libc::F_GETFL);
        if flags < 0 || libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn terminal_name(terminal: &File) -> io::Result<PathBuf> {
    let mut name = [0 as libc::c_char; 128];
    // SAFETY: ttyname_r writes at most `name.len()` bytes, ending them with a zero.
    let result = unsafe { libc::ttyname_r(terminal.as_raw_fd(), name.as_mut_ptr(), name.len()) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result));
    }
    // SAFETY: ttyname_r succeeded, so `name` holds a string ending with a zero.
    let name = unsafe { CStr::from_ptr(name.as_ptr()) };
    Ok(PathBuf::from(name.to_string_lossy().into_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn scratch_link(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pinetime-pty-test-{}-{name}", std::process::id()))
    }

    fn open_client(path: &Path) -> File {
        let client = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        make_raw(&client).unwrap();
        client
    }

    fn read_until(terminal: &mut PseudoTerminal, wanted: usize) -> Vec<u8> {
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while received.len() < wanted && Instant::now() < deadline {
            let mut buffer = [0; 256];
            let count = terminal.read_available(&mut buffer);
            received.extend_from_slice(&buffer[..count]);
            if count == 0 {
                std::thread::sleep(Duration::from_millis(1));
            }
        }
        received
    }

    #[test]
    fn bytes_go_both_ways_through_the_link() {
        let link = scratch_link("round-trip");
        let mut terminal = PseudoTerminal::open(&link).unwrap();
        let mut client = open_client(&link);
        terminal.read_available(&mut [0; 1]);
        assert!(terminal.client_present());

        let every_byte: Vec<u8> = (0..=255).collect();
        client.write_all(&every_byte).unwrap();
        assert_eq!(read_until(&mut terminal, every_byte.len()), every_byte);

        terminal.write(&[0x04, 0x0E, 0x04, 0x01, 0x03, 0x0C, 0x00]);
        let mut answer = [0; 7];
        client.read_exact(&mut answer).unwrap();
        assert_eq!(answer, [0x04, 0x0E, 0x04, 0x01, 0x03, 0x0C, 0x00]);
    }

    #[test]
    fn a_new_client_can_open_the_link_after_one_closes() {
        let link = scratch_link("reopen");
        let mut terminal = PseudoTerminal::open(&link).unwrap();
        let first_terminal = terminal.terminal_path().to_path_buf();
        let first_client = open_client(&link);
        terminal.read_available(&mut [0; 1]);
        drop(first_client);
        terminal.read_available(&mut [0; 1]);
        assert!(!terminal.client_present());
        assert_ne!(terminal.terminal_path(), first_terminal);
        assert_eq!(std::fs::read_link(&link).unwrap(), terminal.terminal_path());

        let mut client = open_client(&link);
        client.write_all(b"again").unwrap();
        assert_eq!(read_until(&mut terminal, 5), b"again");
        assert!(terminal.client_present());
    }

    #[test]
    fn output_without_a_client_is_dropped() {
        let link = scratch_link("no-client");
        let mut terminal = PseudoTerminal::open(&link).unwrap();
        terminal.read_available(&mut [0; 1]);
        terminal.write(b"lost");
        assert_eq!(terminal.dropped_output(), 4);
        assert!(!terminal.has_pending_output());

        let mut client = open_client(&link);
        terminal.read_available(&mut [0; 1]);
        terminal.write(b"kept");
        let mut answer = [0; 4];
        client.read_exact(&mut answer).unwrap();
        assert_eq!(&answer, b"kept");
    }

    #[test]
    fn the_link_goes_away_with_the_terminal() {
        let link = scratch_link("removed");
        let terminal = PseudoTerminal::open(&link).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), terminal.terminal_path());

        drop(terminal);
        assert!(std::fs::symlink_metadata(&link).is_err());
    }

    #[test]
    fn output_a_client_does_not_read_is_capped() {
        let link = scratch_link("capped");
        let mut terminal = PseudoTerminal::open(&link).unwrap();
        let _client = open_client(&link);
        terminal.read_available(&mut [0; 1]);
        let chunk = vec![0x55; 16 * 1024];
        for _ in 0..16 {
            terminal.write(&chunk);
        }
        assert!(terminal.dropped_output() > 0);
        assert!(terminal.pending_output.len() <= MAXIMUM_PENDING_OUTPUT);
    }
}
