"""A small PNG writer and reader for 8-bit RGB images, so screenshots need nothing beyond the standard library."""

import struct
import zlib

SIGNATURE = b"\x89PNG\r\n\x1a\n"


def _chunk(kind, data):
    body = kind + data
    return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body) & 0xFFFFFFFF)


def write_rgb_png(path, width, height, rgb, pitch=None):
    """Saves rows of packed RGB bytes; pitch is the byte length of a source row when it has padding."""
    pitch = pitch or width * 3
    rows = bytearray()
    for y in range(height):
        rows.append(0)
        rows += rgb[y * pitch:y * pitch + width * 3]
    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    with open(path, "wb") as file:
        file.write(SIGNATURE)
        file.write(_chunk(b"IHDR", header))
        file.write(_chunk(b"IDAT", zlib.compress(bytes(rows), 6)))
        file.write(_chunk(b"IEND", b""))


def read_rgb_png(path):
    """Returns (width, height, rgb bytes) for an 8-bit RGB PNG with any row filters."""
    with open(path, "rb") as file:
        data = file.read()
    if not data.startswith(SIGNATURE):
        raise ValueError(f"{path} is not a PNG")
    position = len(SIGNATURE)
    compressed = bytearray()
    width = height = 0
    while position < len(data):
        length, kind = struct.unpack_from(">I4s", data, position)
        body = data[position + 8:position + 8 + length]
        if kind == b"IHDR":
            width, height, depth, colour_type = struct.unpack_from(">IIBB", body)
            if depth != 8 or colour_type != 2:
                raise ValueError("Only 8-bit RGB PNGs are supported")
        elif kind == b"IDAT":
            compressed += body
        position += 12 + length

    raw = zlib.decompress(bytes(compressed))
    stride = width * 3
    pixels = bytearray(height * stride)
    previous = bytearray(stride)
    for y in range(height):
        filter_type = raw[y * (stride + 1)]
        row = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            left = row[i - 3] if i >= 3 else 0
            up = previous[i]
            up_left = previous[i - 3] if i >= 3 else 0
            if filter_type == 1:
                row[i] = (row[i] + left) & 0xFF
            elif filter_type == 2:
                row[i] = (row[i] + up) & 0xFF
            elif filter_type == 3:
                row[i] = (row[i] + (left + up) // 2) & 0xFF
            elif filter_type == 4:
                guess = left + up - up_left
                distances = (abs(guess - left), abs(guess - up), abs(guess - up_left))
                predictor = (left, up, up_left)[distances.index(min(distances))]
                row[i] = (row[i] + predictor) & 0xFF
        pixels[y * stride:(y + 1) * stride] = row
        previous = row
    return width, height, bytes(pixels)
