// SPDX-License-Identifier: GPL-3.0-only
//! The memory bus as the CPU sees it: everything outside the CPU's own private peripheral bus.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccessSize {
    Byte,
    Halfword,
    Word,
}

impl AccessSize {
    pub const fn bytes(self) -> u32 {
        match self {
            AccessSize::Byte => 1,
            AccessSize::Halfword => 2,
            AccessSize::Word => 4,
        }
    }

    pub const fn mask(self) -> u32 {
        match self {
            AccessSize::Byte => 0xFF,
            AccessSize::Halfword => 0xFFFF,
            AccessSize::Word => 0xFFFF_FFFF,
        }
    }
}

/// Why the bus refused an access; the CPU turns it into a BusFault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BusError {
    /// Nothing answers at this address.
    Unmapped,
    /// Something is there but does not take accesses of this size.
    BadSize,
}

pub trait Bus {
    /// Reads `size` bytes at `address`, little-endian, into the low bits of the result.
    fn read(&mut self, address: u32, size: AccessSize) -> Result<u32, BusError>;

    /// Writes the low `size` bytes of `value` at `address`, little-endian.
    fn write(&mut self, address: u32, value: u32, size: AccessSize) -> Result<(), BusError>;

    /// Reads one instruction halfword; flash and RAM answer it without side effects.
    fn fetch(&mut self, address: u32) -> Result<u16, BusError> {
        self.read(address, AccessSize::Halfword).map(|value| value as u16)
    }

    /// Extra cycles the memory holds the CPU for while it fetches the instruction word at `address`.
    fn fetch_wait_cycles(&mut self, _address: u32) -> u32 {
        0
    }

    /// Extra cycles the memory holds the CPU for during a data access at `address`.
    fn data_wait_cycles(&mut self, _address: u32) -> u32 {
        0
    }

    /// Moves emulated time on by the cycles the last instruction took; the CPU calls it after
    /// every instruction, before `interrupt_lines`, so peripherals read the right time.
    fn advance(&mut self, cycles: u32);

    /// The interrupt lines the chip holds high, one bit per line, if they changed since the
    /// last call; the CPU asks after every instruction and latches each high line as pending.
    fn interrupt_lines(&mut self) -> Option<u64>;

    /// Whether the CPU should hand control back before its next instruction, for a chip reset,
    /// a watchpoint or a due event.
    fn stop_requested(&self) -> bool {
        false
    }
}
