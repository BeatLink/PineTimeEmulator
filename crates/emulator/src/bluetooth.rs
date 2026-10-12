// SPDX-License-Identifier: GPL-3.0-only
//! The Bluetooth bridge: a second machine, an nRF52840 running a Bluetooth LE controller, whose
//! radio shares the air with the watch's and whose UARTE1 carries H4 HCI to a host
//! pseudo-terminal that BlueZ attaches with `btattach`.
//!
//! The two machines run in lockstep. Each step ends no later than the earliest moment a packet
//! sent by either radio could need the other to have heard its access address, so frames are
//! handed over at step ends and still arrive in the receiver's future.

use std::path::Path;
use std::sync::{Arc, Mutex};

use pinetime_core::time::cycles_from_microseconds;
use pinetime_core::{BoardPorts, Cycle, DeviceContext, EventKey};
use pinetime_nrf52832::{Nrf52832, RadioFrame, RadioMedium, SerialPort, Variant};

use crate::images::{elf_pieces, Piece};
use crate::machine::{Board, Machine};
use crate::pseudo_terminal::PseudoTerminal;

// ============================================================================
// Bridge board
// ============================================================================

/// The bridge's board: nothing on its pins, buses or analog inputs.
#[derive(Default)]
pub struct BridgeBoard;

impl BoardPorts for BridgeBoard {
    fn pin_outputs_changed(&mut self, _levels: u32, _outputs: u32, _context: &mut dyn DeviceContext) {}

    fn spi_transfer(&mut self, _instance: u8, _byte: u8, _context: &mut dyn DeviceContext) -> u8 {
        0xFF
    }

    fn i2c_start(
        &mut self,
        _instance: u8,
        _address: u8,
        _read: bool,
        _context: &mut dyn DeviceContext,
    ) -> bool {
        false
    }

    fn i2c_write(&mut self, _instance: u8, _byte: u8, _context: &mut dyn DeviceContext) -> bool {
        false
    }

    fn i2c_read(&mut self, _instance: u8, _context: &mut dyn DeviceContext) -> u8 {
        0xFF
    }

    fn i2c_stop(&mut self, _instance: u8, _context: &mut dyn DeviceContext) {}

    fn analog_input_volts(&self, _channel: u8) -> Option<f32> {
        None
    }

    fn handle_event(&mut self, _key: EventKey, _context: &mut dyn DeviceContext) {}
}

impl Board for BridgeBoard {
    fn power_on(&mut self, _context: &mut dyn DeviceContext) {}
}

// ============================================================================
// The air between the two radios
// ============================================================================

/// The frames one radio sent since the last hand-over.
#[derive(Clone, Default)]
pub struct Outbox(Arc<Mutex<Vec<RadioFrame>>>);

impl RadioMedium for Outbox {
    fn transmit(&mut self, frame: RadioFrame) {
        self.0.lock().unwrap().push(frame);
    }
}

impl Outbox {
    pub fn take(&self) -> Vec<RadioFrame> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

// ============================================================================
// Lockstep
// ============================================================================

/// The step while either radio is on: shorter than the 27 µs from START to the end of the access
/// address at 2 Mbit, so a frame is handed over before its receiver needs it.
pub const ACTIVE_LOOKAHEAD: Cycle = cycles_from_microseconds(16);

/// From a disabled radio, ramp-up alone takes 40 µs before START, then the address takes 27 µs or more.
pub const IDLE_RADIO_LATENCY: Cycle = cycles_from_microseconds(64);

/// The longest step, so the pseudo-terminal is looked at at least once per emulated millisecond.
pub const MAXIMUM_STEP: Cycle = cycles_from_microseconds(1_000);

/// How long a step may be from `now`, given when each machine could next act and whether either radio is on.
pub fn lookahead(now: Cycle, any_radio_active: bool, quiet_until: [Cycle; 2]) -> Cycle {
    if any_radio_active {
        return ACTIVE_LOOKAHEAD;
    }
    let earliest_action = quiet_until.into_iter().min().unwrap_or(now).max(now);
    (earliest_action - now)
        .saturating_add(IDLE_RADIO_LATENCY)
        .min(MAXIMUM_STEP)
}

/// Hands each machine the frames the other sent, at the receiver's current time.
pub fn exchange_frames<A: Board, B: Board>(
    first: &mut Machine<A>,
    first_outbox: &Outbox,
    second: &mut Machine<B>,
    second_outbox: &Outbox,
) {
    for frame in first_outbox.take() {
        second.receive_radio_frame(frame);
    }
    for frame in second_outbox.take() {
        first.receive_radio_frame(frame);
    }
}

/// Runs two machines on one air until both reach `target`, calling `between_steps` before each
/// step; they never drift further apart than one step.
pub fn run_in_lockstep<A: Board, B: Board>(
    first: &mut Machine<A>,
    first_outbox: &Outbox,
    second: &mut Machine<B>,
    second_outbox: &Outbox,
    target: Cycle,
    mut between_steps: impl FnMut(&mut Machine<A>, &mut Machine<B>),
) {
    while first.now() < target || second.now() < target {
        between_steps(first, second);
        let now = first.now().max(second.now());
        let any_radio_active = !first.chip.radio_idle() || !second.chip.radio_idle();
        let quiet_until = [first.quiet_until(), second.quiet_until()];
        let step_end = (now + lookahead(now, any_radio_active, quiet_until)).min(target.max(now));
        first.run_until(step_end);
        second.run_until(step_end);
        exchange_frames(first, first_outbox, second, second_outbox);
    }
}

// ============================================================================
// Bridge
// ============================================================================

/// The UART that carries HCI on the bridge, as in Antmicro's Zephyr build for Renode.
const HCI_PORT: SerialPort = SerialPort::Uarte1;

/// Bytes from the host wait in the chip only up to this many, so the host's writes are held back.
const MAXIMUM_WAITING_INPUT: usize = 64;

/// How often the pseudo-terminal is read, in emulated time; at 1 Mbaud this is ten bytes.
const INPUT_POLL_INTERVAL: Cycle = cycles_from_microseconds(100);

pub struct Bridge {
    pub machine: Machine<BridgeBoard>,
    pub terminal: PseudoTerminal,
    pub watch_outbox: Outbox,
    pub bridge_outbox: Outbox,
    last_input_poll: Cycle,
}

impl Bridge {
    /// Builds the bridge with its clock at `now`, loads the controller firmware and opens the
    /// pseudo-terminal behind `link`; the caller attaches `watch_outbox` to the watch's radio.
    pub fn new(now: Cycle, controller_elf: &[u8], link: &Path) -> Result<Bridge, String> {
        let pieces = elf_pieces(controller_elf).map_err(|error| error.to_string())?;
        let terminal = PseudoTerminal::open(link)
            .map_err(|error| format!("could not open a pseudo-terminal: {error}"))?;
        let mut machine = Machine::with_parts(Nrf52832::new_variant(Variant::Nrf52840), BridgeBoard, now);
        load_pieces(&mut machine, &pieces)?;
        let bridge_outbox = Outbox::default();
        machine
            .chip
            .set_radio_medium(Some(Box::new(bridge_outbox.clone())));
        Ok(Bridge {
            machine,
            terminal,
            watch_outbox: Outbox::default(),
            bridge_outbox,
            last_input_poll: 0,
        })
    }

    /// Runs the watch and the bridge together until both reach `target`, calling `between_steps`
    /// on the watch before each step.
    pub fn run_with<B: Board>(
        &mut self,
        watch: &mut Machine<B>,
        target: Cycle,
        mut between_steps: impl FnMut(&mut Machine<B>),
    ) {
        let Bridge {
            machine,
            terminal,
            watch_outbox,
            bridge_outbox,
            last_input_poll,
        } = self;
        let mut serial = SerialLink {
            terminal,
            last_input_poll,
        };
        run_in_lockstep(
            watch,
            watch_outbox,
            machine,
            bridge_outbox,
            target,
            |watch, bridge| {
                between_steps(watch);
                serial.exchange(bridge);
            },
        );
        serial.exchange(machine);
    }
}

/// The pseudo-terminal's side of the controller's UART.
struct SerialLink<'a> {
    terminal: &'a mut PseudoTerminal,
    last_input_poll: &'a mut Cycle,
}

impl SerialLink<'_> {
    /// Sends what the controller wrote, then hands it what the host wrote, while its queue has room.
    fn exchange(&mut self, machine: &mut Machine<BridgeBoard>) {
        let output = machine.chip.take_serial_port_output(HCI_PORT);
        if !output.is_empty() {
            self.terminal.write(&output);
        } else if self.terminal.has_pending_output() {
            self.terminal.flush();
        }

        let now = machine.now();
        if now.saturating_sub(*self.last_input_poll) < INPUT_POLL_INTERVAL {
            return;
        }
        *self.last_input_poll = now;
        let waiting = machine.chip.serial_port_waiting_bytes(HCI_PORT);
        if waiting >= MAXIMUM_WAITING_INPUT {
            return;
        }
        let mut buffer = [0; MAXIMUM_WAITING_INPUT];
        let count = self
            .terminal
            .read_available(&mut buffer[..MAXIMUM_WAITING_INPUT - waiting]);
        if count > 0 {
            machine.serial_port_receive(HCI_PORT, &buffer[..count]);
        }
    }
}

// The controller runs from flash, so every piece must land there.
fn load_pieces(machine: &mut Machine<BridgeBoard>, pieces: &[Piece]) -> Result<(), String> {
    let flash = machine.chip.flash_mut();
    for piece in pieces {
        let start = piece.address as usize;
        let destination = flash.get_mut(start..start + piece.bytes.len()).ok_or_else(|| {
            format!("the controller firmware has bytes at {start:#010x}, outside the flash")
        })?;
        destination.copy_from_slice(&piece.bytes);
    }
    Ok(())
}
