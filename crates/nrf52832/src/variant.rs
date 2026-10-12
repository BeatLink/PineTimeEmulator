// SPDX-License-Identifier: GPL-3.0-only
//! The two chips this crate models: the PineTime's nRF52832, and the nRF52840 the Bluetooth
//! bridge's controller firmware is built for.

// ============================================================================
// Variant
// ============================================================================

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Variant {
    #[default]
    Nrf52832,
    /// Twice the flash, four times the RAM, GPIO port P1, UARTE1 and the newer radio events.
    Nrf52840,
}

impl Variant {
    pub const fn flash_size(self) -> usize {
        match self {
            Variant::Nrf52832 => 0x8_0000,
            Variant::Nrf52840 => 0x10_0000,
        }
    }

    pub const fn ram_size(self) -> usize {
        match self {
            Variant::Nrf52832 => 0x1_0000,
            Variant::Nrf52840 => 0x4_0000,
        }
    }

    pub const fn is_nrf52840(self) -> bool {
        matches!(self, Variant::Nrf52840)
    }

    /// The FICR words that differ from erased: page size and count, device address, part, RAM and flash.
    pub(crate) const fn ficr_words(self) -> &'static [(usize, u32)] {
        match self {
            Variant::Nrf52832 => &NRF52832_FICR_WORDS,
            Variant::Nrf52840 => &NRF52840_FICR_WORDS,
        }
    }

    /// The part and revision numbers older Nordic SDKs read from 0xF0000FE0 to pick errata workarounds.
    pub(crate) const fn errata_words(self) -> &'static [(usize, u32)] {
        match self {
            Variant::Nrf52832 => &NRF52832_ERRATA_WORDS,
            Variant::Nrf52840 => &NRF52840_ERRATA_WORDS,
        }
    }
}

// ============================================================================
// Identification
// ============================================================================

// The watch's random static address is C0:DE:0A:1B:2C:3D.
const NRF52832_FICR_WORDS: [(usize, u32); 14] = [
    (0x010, 0x1000),
    (0x014, 0x80),
    (0x060, 0x5049_4E45),
    (0x064, 0x5449_4D45),
    (0x0A0, 0x1),
    (0x0A4, 0x0A1B_2C3D),
    (0x0A8, 0xC0DE),
    (0x100, 0x52832),
    (0x104, 0x4141_4530),
    (0x108, 0x2000),
    (0x10C, 0x40),
    (0x110, 0x200),
    (0x130, 0x6),
    (0x134, 0x5),
];

// The bridge's random static address is E8:40:AA:BB:CC:DD once the controller sets the top two bits.
const NRF52840_FICR_WORDS: [(usize, u32); 14] = [
    (0x010, 0x1000),
    (0x014, 0x100),
    (0x060, 0x4252_4944),
    (0x064, 0x4745_4852),
    (0x0A0, 0x1),
    (0x0A4, 0xAABB_CCDD),
    (0x0A8, 0xE840),
    (0x100, 0x52840),
    (0x104, 0x4141_4430),
    (0x108, 0x2004),
    (0x10C, 0x100),
    (0x110, 0x400),
    (0x130, 0x8),
    (0x134, 0x3),
];

const NRF52832_ERRATA_WORDS: [(usize, u32); 4] = [(0xFE0, 0x06), (0xFE4, 0x00), (0xFE8, 0x50), (0xFEC, 0x00)];

const NRF52840_ERRATA_WORDS: [(usize, u32); 4] = [(0xFE0, 0x08), (0xFE4, 0x00), (0xFE8, 0x50), (0xFEC, 0x00)];
