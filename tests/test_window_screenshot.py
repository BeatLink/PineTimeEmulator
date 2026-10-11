"""Runs pinetime-emulator headless on InfiniTime and checks the saved window shows the drawn watch with a lit screen.

Needs `pinetime-emulator` on PATH with its InfiniTime build, as `nix develop` provides; it is skipped otherwise.
"""

import shutil
import struct
import subprocess
import zlib

import pytest

# The screen's place in the window at scale 1, from frontend/pinetime_emulator/watch_body.py.
SCREEN_LEFT = 34 + 30 + 12
SCREEN_TOP = 54 + 30 + 12
SCREEN_SIZE = 240
WATCH_WIDTH = 392
WATCH_HEIGHT = 432
CASE_POINT = (34 + 10, 54 + 162)


def read_png(path):
    """Decodes the 8-bit RGB, filter-0 PNGs the emulator writes."""
    with open(path, "rb") as file:
        data = file.read()
    assert data[:8] == b"\x89PNG\r\n\x1a\n"
    position, compressed = 8, b""
    while position < len(data):
        length, kind = struct.unpack_from(">I4s", data, position)
        body = data[position + 8:position + 8 + length]
        if kind == b"IHDR":
            width, height = struct.unpack_from(">II", body)
        elif kind == b"IDAT":
            compressed += body
        position += 12 + length
    raw = zlib.decompress(compressed)
    stride = width * 3 + 1
    assert all(raw[y * stride] == 0 for y in range(height))
    return width, height, b"".join(raw[y * stride + 1:(y + 1) * stride] for y in range(height))


def pixel(image, x, y):
    width, _, rgb = image
    offset = (y * width + x) * 3
    return tuple(rgb[offset:offset + 3])


@pytest.mark.skipif(shutil.which("pinetime-emulator") is None, reason="pinetime-emulator is not on PATH")
def test_headless_screenshot_shows_infinitime_in_the_watch(tmp_path):
    path = tmp_path / "window.png"
    result = subprocess.run(
        ["pinetime-emulator", "--infinitime", "--screenshot-after", "4", str(path)],
        capture_output=True, text=True, timeout=600,
    )
    assert result.returncode == 0, result.stdout + result.stderr

    image = read_png(path)
    width, height, _ = image
    assert width > WATCH_WIDTH and height > WATCH_HEIGHT

    # The case is drawn in its grey, not left as background.
    red, green, blue = pixel(image, *CASE_POINT)
    assert 40 <= red <= 110 and abs(red - blue) < 20

    # InfiniTime's watch face has lit pixels in several colours on the screen.
    colours = set()
    lit = 0
    for y in range(SCREEN_TOP, SCREEN_TOP + SCREEN_SIZE, 3):
        for x in range(SCREEN_LEFT, SCREEN_LEFT + SCREEN_SIZE, 3):
            colour = pixel(image, x, y)
            if sum(colour) > 60:
                lit += 1
                colours.add(colour)
    assert lit > 200, "the screen area is dark"
    assert len(colours) >= 2, "the screen shows a single flat colour"
