// SPDX-License-Identifier: GPL-3.0-only
//! Loads and stores: single, dual, exclusive and multiple, with unaligned access support.
//! Ported from InfiniEmu's cpu.c.

use pinetime_core::{AccessSize, Bus};

use super::{Fault, Outcome, Trap};
use crate::cpu::CortexM4;
use crate::decode::{LoadKind, StoreSize};
use crate::private_bus;

// ============================================================================
// Bus access
// ============================================================================

fn load_size(kind: LoadKind) -> AccessSize {
    match kind {
        LoadKind::Word => AccessSize::Word,
        LoadKind::Halfword | LoadKind::SignedHalfword => AccessSize::Halfword,
        LoadKind::Byte | LoadKind::SignedByte => AccessSize::Byte,
    }
}

fn store_size(size: StoreSize) -> AccessSize {
    match size {
        StoreSize::Word => AccessSize::Word,
        StoreSize::Halfword => AccessSize::Halfword,
        StoreSize::Byte => AccessSize::Byte,
    }
}

fn extend_loaded(kind: LoadKind, value: u32) -> u32 {
    match kind {
        LoadKind::SignedByte => value as i8 as i32 as u32,
        LoadKind::SignedHalfword => value as i16 as i32 as u32,
        _ => value,
    }
}

impl CortexM4 {
    /// One aligned access, routed to the private bus or the outside bus.
    #[inline(always)]
    pub(crate) fn read_memory<B: Bus>(
        &mut self,
        bus: &mut B,
        address: u32,
        size: AccessSize,
    ) -> Result<u32, Trap> {
        if (private_bus::START..private_bus::END).contains(&address) {
            if !self.private_bus_allows(address) {
                return Err(Trap::Fault(Fault::DataBus(address)));
            }
            return Ok(self.read_private_bus(address, size));
        }
        self.data_wait_cycles += bus.data_wait_cycles(address);
        bus.read(address, size)
            .map_err(|_| Trap::Fault(Fault::DataBus(address)))
    }

    #[inline(always)]
    pub(crate) fn write_memory<B: Bus>(
        &mut self,
        bus: &mut B,
        address: u32,
        value: u32,
        size: AccessSize,
    ) -> Result<(), Trap> {
        if (private_bus::START..private_bus::END).contains(&address) {
            if !self.private_bus_allows(address) {
                return Err(Trap::Fault(Fault::DataBus(address)));
            }
            self.write_private_bus(address, value, size);
            return Ok(());
        }
        self.data_wait_cycles += bus.data_wait_cycles(address);
        bus.write(address, value & size.mask(), size)
            .map_err(|_| Trap::Fault(Fault::DataBus(address)))
    }

    /// A word access that must be aligned, as LDM, LDRD, LDREX and exception frames need.
    #[inline]
    pub(crate) fn read_word<B: Bus>(&mut self, bus: &mut B, address: u32) -> Result<u32, Trap> {
        if address & 3 != 0 {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        self.read_memory(bus, address, AccessSize::Word)
    }

    #[inline]
    pub(crate) fn write_word<B: Bus>(&mut self, bus: &mut B, address: u32, value: u32) -> Result<(), Trap> {
        if address & 3 != 0 {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        self.write_memory(bus, address, value, AccessSize::Word)
    }

    /// An access LDR, LDRH, STR and STRH make: unaligned ones go byte by byte unless CCR traps them.
    #[inline(always)]
    fn read_unaligned<B: Bus>(&mut self, bus: &mut B, address: u32, size: AccessSize) -> Result<u32, Trap> {
        if address & (size.bytes() - 1) == 0 {
            return self.read_memory(bus, address, size);
        }
        if self.scb.unaligned_trap() {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        let mut value = 0;
        for byte in 0..size.bytes() {
            value |= self.read_memory(bus, address.wrapping_add(byte), AccessSize::Byte)? << (8 * byte);
        }
        Ok(value)
    }

    #[inline(always)]
    fn write_unaligned<B: Bus>(
        &mut self,
        bus: &mut B,
        address: u32,
        value: u32,
        size: AccessSize,
    ) -> Result<(), Trap> {
        if address & (size.bytes() - 1) == 0 {
            return self.write_memory(bus, address, value, size);
        }
        if self.scb.unaligned_trap() {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        for byte in 0..size.bytes() {
            self.write_memory(
                bus,
                address.wrapping_add(byte),
                (value >> (8 * byte)) & 0xFF,
                AccessSize::Byte,
            )?;
        }
        Ok(())
    }

    // ========================================================================
    // Single loads and stores
    // ========================================================================

    #[inline(always)]
    fn load_into<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: LoadKind,
        target: u8,
        address: u32,
    ) -> Result<(), Trap> {
        let value = extend_loaded(kind, self.read_unaligned(bus, address, load_size(kind))?);
        self.write_loaded_register(target, value)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn execute_load_immediate<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: LoadKind,
        target: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    ) -> Outcome {
        let base_value = self.read_register(base);
        let offset_address = base_value.wrapping_add(offset as u32);
        let address = if index { offset_address } else { base_value };
        let value = extend_loaded(kind, self.read_unaligned(bus, address, load_size(kind))?);
        if writeback {
            self.write_register(base, offset_address);
        }
        self.write_loaded_register(target, value)?;
        Ok(2)
    }

    #[inline(always)]
    pub(crate) fn execute_load_literal<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: LoadKind,
        target: u8,
        address: u32,
    ) -> Outcome {
        self.load_into(bus, kind, target, address)?;
        Ok(2)
    }

    #[inline(always)]
    pub(crate) fn execute_load_register<B: Bus>(
        &mut self,
        bus: &mut B,
        kind: LoadKind,
        target: u8,
        base: u8,
        offset_register: u8,
        shift: u8,
    ) -> Outcome {
        let address = self
            .read_register(base)
            .wrapping_add(self.read_register(offset_register) << shift);
        self.load_into(bus, kind, target, address)?;
        Ok(2)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    pub(crate) fn execute_store_immediate<B: Bus>(
        &mut self,
        bus: &mut B,
        size: StoreSize,
        source: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    ) -> Outcome {
        let base_value = self.read_register(base);
        let offset_address = base_value.wrapping_add(offset as u32);
        let address = if index { offset_address } else { base_value };
        let value = self.read_register(source);
        self.write_unaligned(bus, address, value, store_size(size))?;
        if writeback {
            self.write_register(base, offset_address);
        }
        Ok(2)
    }

    #[inline(always)]
    pub(crate) fn execute_store_register<B: Bus>(
        &mut self,
        bus: &mut B,
        size: StoreSize,
        source: u8,
        base: u8,
        offset_register: u8,
        shift: u8,
    ) -> Outcome {
        let address = self
            .read_register(base)
            .wrapping_add(self.read_register(offset_register) << shift);
        let value = self.read_register(source);
        self.write_unaligned(bus, address, value, store_size(size))?;
        Ok(2)
    }

    // ========================================================================
    // Dual loads and stores
    // ========================================================================

    fn dual_addresses(&self, base: u8, offset: i32, index: bool) -> (u32, u32) {
        let mut base_value = self.read_register(base);
        if base == 15 {
            base_value &= !3;
        }
        let offset_address = base_value.wrapping_add(offset as u32);
        (if index { offset_address } else { base_value }, offset_address)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_load_dual<B: Bus>(
        &mut self,
        bus: &mut B,
        first: u8,
        second: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    ) -> Outcome {
        let (address, offset_address) = self.dual_addresses(base, offset, index);
        let low = self.read_word(bus, address)?;
        let high = self.read_word(bus, address.wrapping_add(4))?;
        if writeback {
            self.write_register(base, offset_address);
        }
        self.write_register(first, low);
        self.write_register(second, high);
        Ok(3)
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn execute_store_dual<B: Bus>(
        &mut self,
        bus: &mut B,
        first: u8,
        second: u8,
        base: u8,
        offset: i32,
        index: bool,
        writeback: bool,
    ) -> Outcome {
        let (address, offset_address) = self.dual_addresses(base, offset, index);
        let low = self.read_register(first);
        let high = self.read_register(second);
        self.write_word(bus, address, low)?;
        self.write_word(bus, address.wrapping_add(4), high)?;
        if writeback {
            self.write_register(base, offset_address);
        }
        Ok(2)
    }

    // ========================================================================
    // Exclusive loads and stores
    // ========================================================================

    pub(crate) fn execute_load_exclusive<B: Bus>(
        &mut self,
        bus: &mut B,
        size: StoreSize,
        target: u8,
        base: u8,
        offset: u32,
    ) -> Outcome {
        let access = store_size(size);
        let address = self.read_register(base).wrapping_add(offset);
        if address & (access.bytes() - 1) != 0 {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        let value = self.read_memory(bus, address, access)?;
        self.exclusive_monitor_open = true;
        self.write_register(target, value);
        Ok(2)
    }

    pub(crate) fn execute_store_exclusive<B: Bus>(
        &mut self,
        bus: &mut B,
        size: StoreSize,
        status: u8,
        source: u8,
        base: u8,
        offset: u32,
    ) -> Outcome {
        let access = store_size(size);
        let address = self.read_register(base).wrapping_add(offset);
        if address & (access.bytes() - 1) != 0 {
            return Err(Trap::Fault(Fault::Unaligned));
        }
        if self.exclusive_monitor_open {
            let value = self.read_register(source);
            self.write_memory(bus, address, value, access)?;
            self.exclusive_monitor_open = false;
            self.write_register(status, 0);
        } else {
            self.write_register(status, 1);
        }
        Ok(2)
    }

    // ========================================================================
    // Multiple loads and stores
    // ========================================================================

    pub(crate) fn execute_load_multiple<B: Bus>(
        &mut self,
        bus: &mut B,
        base: u8,
        registers: u16,
        writeback: bool,
        decrement_before: bool,
    ) -> Outcome {
        let count = registers.count_ones();
        let base_value = self.read_register(base);
        let start = if decrement_before {
            base_value.wrapping_sub(4 * count)
        } else {
            base_value
        };
        let mut address = start;
        let mut loaded_program_counter = None;
        let mut remaining = registers;
        while remaining != 0 {
            let register = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            let value = self.read_word(bus, address)?;
            if register == 15 {
                loaded_program_counter = Some(value);
            } else {
                self.write_register(register, value);
            }
            address = address.wrapping_add(4);
        }
        if writeback && registers & (1 << base) == 0 {
            let final_base = if decrement_before {
                start
            } else {
                base_value.wrapping_add(4 * count)
            };
            self.write_register(base, final_base);
        }
        if let Some(value) = loaded_program_counter {
            self.branch_exchange(value)?;
        }
        Ok(1 + count)
    }

    pub(crate) fn execute_store_multiple<B: Bus>(
        &mut self,
        bus: &mut B,
        base: u8,
        registers: u16,
        writeback: bool,
        decrement_before: bool,
    ) -> Outcome {
        let count = registers.count_ones();
        let base_value = self.read_register(base);
        let start = if decrement_before {
            base_value.wrapping_sub(4 * count)
        } else {
            base_value
        };
        let mut address = start;
        let mut remaining = registers;
        while remaining != 0 {
            let register = remaining.trailing_zeros() as u8;
            remaining &= remaining - 1;
            let value = self.read_register(register);
            self.write_word(bus, address, value)?;
            address = address.wrapping_add(4);
        }
        if writeback {
            let final_base = if decrement_before {
                start
            } else {
                base_value.wrapping_add(4 * count)
            };
            self.write_register(base, final_base);
        }
        Ok(1 + count)
    }

    // ========================================================================
    // Table branch
    // ========================================================================

    pub(crate) fn execute_table_branch<B: Bus>(
        &mut self,
        bus: &mut B,
        halfword: bool,
        base: u8,
        index: u8,
    ) -> Outcome {
        let base_value = self.read_register(base);
        let index_value = self.read_register(index);
        let offset = if halfword {
            self.read_unaligned(
                bus,
                base_value.wrapping_add(index_value << 1),
                AccessSize::Halfword,
            )?
        } else {
            self.read_memory(bus, base_value.wrapping_add(index_value), AccessSize::Byte)?
        };
        self.pc = self.registers[15].wrapping_add(2 * offset);
        Ok(3)
    }
}
