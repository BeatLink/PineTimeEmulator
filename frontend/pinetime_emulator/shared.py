# SPDX-License-Identifier: GPL-3.0-or-later
"""Readers for the two memory-mapped files the engine writes: the screen frames and the status block.

Frame file (crates/emulator/src/sharing.rs, ShareFramesThrough), little-endian:
    0  u32  magic "PTFB"
    4  u32  version (1)
    8  u32  sequence: odd while a frame is being written
   12  u32  frame count, one per repaint
   16  u16  width, 18 u16 height
   20  u32  flags: 1 sleeping, 2 display on, 4 in reset, 8 inverted
   24  u32  backlight level 0..3
   28  u32  header size (64); the RGB pixels follow it, 3 bytes each

Status file (crates/emulator/src/sharing.rs, ShareStatusThrough), little-endian:
    0  u32  magic "PTST"
    4  u32  version (1)
    8  u32  sequence: odd while being written
   12  u32  flags: 1 motor running, 2 button pressed, 4 charger connected, 8 charging, 16 paused
   16  u32  motor start count
   20  i32  battery percent
   24  f64  battery volts
   32  u64  emulated time in microseconds
   40  u32  step count
"""

import mmap
import os
import struct
from dataclasses import dataclass

# ============================================================================
# Layout constants
# ============================================================================

FRAME_MAGIC = 0x42465450
STATUS_MAGIC = 0x54535450
FRAME_HEADER = struct.Struct("<IIIIHHIII")
STATUS_LAYOUT = struct.Struct("<IIIIIidQI")
READ_ATTEMPTS = 20


# ============================================================================
# Snapshots
# ============================================================================

@dataclass
class Frame:
    count: int
    width: int
    height: int
    sleeping: bool
    display_on: bool
    in_reset: bool
    backlight_level: int
    pixels: bytes


@dataclass
class Status:
    motor_running: bool
    button_pressed: bool
    charger_connected: bool
    charging: bool
    paused: bool
    motor_starts: int
    battery_percent: int
    battery_volts: float
    emulated_seconds: float
    steps: int


# ============================================================================
# Readers
# ============================================================================

class _MappedFile:
    """A read-only map of a file the engine creates, opened once the file exists and is complete."""

    def __init__(self, path, minimum_size):
        self.path = path
        self.minimum_size = minimum_size
        self.mapping = None

    def open(self):
        if self.mapping is not None:
            return True
        try:
            with open(self.path, "rb") as file:
                if os.fstat(file.fileno()).st_size < self.minimum_size:
                    return False
                self.mapping = mmap.mmap(file.fileno(), 0, access=mmap.ACCESS_READ)
        except (FileNotFoundError, ValueError):
            return False
        return True

    def close(self):
        if self.mapping is not None:
            self.mapping.close()
            self.mapping = None


class FrameReader(_MappedFile):
    def __init__(self, path):
        super().__init__(path, 64)

    def read(self, last_count=None):
        """The latest whole frame, or None when there is none yet or it is the one already read."""
        if not self.open():
            return None
        for _ in range(READ_ATTEMPTS):
            header = FRAME_HEADER.unpack_from(self.mapping, 0)
            magic, _version, sequence, count, width, height, flags, backlight, header_size = header
            if magic != FRAME_MAGIC:
                return None
            if sequence & 1:
                continue
            if last_count is not None and count == last_count:
                return None
            pixels = self.mapping[header_size:header_size + width * height * 3]
            if struct.unpack_from("<I", self.mapping, 8)[0] != sequence:
                continue
            return Frame(count, width, height, bool(flags & 1), bool(flags & 2), bool(flags & 4), backlight, pixels)
        return None


class StatusReader(_MappedFile):
    def __init__(self, path):
        super().__init__(path, STATUS_LAYOUT.size)

    def read(self):
        if not self.open():
            return None
        for _ in range(READ_ATTEMPTS):
            values = STATUS_LAYOUT.unpack_from(self.mapping, 0)
            magic, _version, sequence, flags, motor_starts, percent, volts, micros, steps = values
            if magic != STATUS_MAGIC:
                return None
            if sequence & 1:
                continue
            if struct.unpack_from("<I", self.mapping, 8)[0] != sequence:
                continue
            return Status(
                motor_running=bool(flags & 1),
                button_pressed=bool(flags & 2),
                charger_connected=bool(flags & 4),
                charging=bool(flags & 8),
                paused=bool(flags & 16),
                motor_starts=motor_starts,
                battery_percent=percent,
                battery_volts=volts,
                emulated_seconds=micros / 1e6,
                steps=steps,
            )
        return None
