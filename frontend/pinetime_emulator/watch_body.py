# SPDX-License-Identifier: GPL-3.0-or-later
"""Paints the watch body procedurally: a dark rounded case, the glass around the 240x240 screen, strap stubs and the side button."""

import math
from dataclasses import dataclass

# ============================================================================
# Geometry at scale 1
# ============================================================================

SCREEN_SIZE = 240
BEZEL = 12
CASE_MARGIN = 30
CASE_SIZE = SCREEN_SIZE + 2 * (BEZEL + CASE_MARGIN)
CASE_RADIUS = 52
STRAP_LENGTH = 54
STRAP_WIDTH = 190
BUTTON_DEPTH = 9
BUTTON_HEIGHT = 46
SIDE_ROOM = 34


@dataclass(frozen=True)
class Rect:
    x: int
    y: int
    width: int
    height: int

    def contains(self, px, py):
        return self.x <= px < self.x + self.width and self.y <= py < self.y + self.height


@dataclass(frozen=True)
class Layout:
    width: int
    height: int
    case: Rect
    screen: Rect
    button: Rect


def layout():
    width = CASE_SIZE + 2 * SIDE_ROOM
    height = CASE_SIZE + 2 * STRAP_LENGTH
    case = Rect(SIDE_ROOM, STRAP_LENGTH, CASE_SIZE, CASE_SIZE)
    inset = CASE_MARGIN + BEZEL
    screen = Rect(case.x + inset, case.y + inset, SCREEN_SIZE, SCREEN_SIZE)
    button = Rect(case.x + CASE_SIZE - 3, case.y + (CASE_SIZE - BUTTON_HEIGHT) // 2, BUTTON_DEPTH + 3, BUTTON_HEIGHT)
    return Layout(width, height, case, screen, button)


# ============================================================================
# Colours
# ============================================================================

BACKGROUND = (30, 32, 36)
STRAP = (22, 23, 26)
STRAP_EDGE = (44, 46, 52)
CASE_TOP = (74, 77, 84)
CASE_BOTTOM = (40, 42, 47)
CASE_RIM = (100, 104, 112)
GLASS = (6, 6, 8)
BUTTON = (92, 95, 102)
BUTTON_LIT = (80, 196, 255)
GLOW = (255, 170, 60)


# ============================================================================
# Shape coverage
# ============================================================================

SUBROWS = 4


def _row_edges(rect, radius, y):
    """The left and right edges of a rounded rectangle at four heights inside pixel row y, or [] when the row misses it."""
    edges = []
    centre_x = rect.x + rect.width / 2
    centre_y = rect.y + rect.height / 2
    straight_half = rect.width / 2 - radius
    for sub in range(SUBROWS):
        sample_y = y + (sub + 0.5) / SUBROWS
        if not rect.y <= sample_y <= rect.y + rect.height:
            continue
        rise = max(0.0, abs(sample_y - centre_y) - (rect.height / 2 - radius))
        if rise > radius:
            continue
        half = straight_half + math.sqrt(radius * radius - rise * rise)
        edges.append((centre_x - half, centre_x + half))
    return edges


def _row_coverage(rect, radius, y, width):
    """Yields (first x, last x, coverage) runs for one row: partial edge pixels singly, the solid middle as one run."""
    edges = _row_edges(rect, radius, y)
    if not edges:
        return
    inner_left = math.ceil(max(left for left, _ in edges))
    inner_right = math.floor(min(right for _, right in edges))
    outer_left = max(0, math.floor(min(left for left, _ in edges)))
    outer_right = min(width, math.ceil(max(right for _, right in edges)))
    full_rows = len(edges) == SUBROWS

    def coverage(x):
        return sum(max(0.0, min(x + 1, right) - max(x, left)) for left, right in edges) / SUBROWS

    if not full_rows or inner_left >= inner_right:
        for x in range(outer_left, outer_right):
            yield x, x + 1, coverage(x)
        return
    for x in range(outer_left, inner_left):
        yield x, x + 1, coverage(x)
    yield max(0, inner_left), min(width, inner_right), 1.0
    for x in range(inner_right, outer_right):
        yield x, x + 1, coverage(x)


def _fill_rounded(pixels, width, rect, radius, colour_for_row, alpha=1.0):
    """Paints an anti-aliased rounded rectangle over opaque RGBA pixels."""
    for y in range(max(0, rect.y), rect.y + rect.height):
        colour = colour_for_row(y)
        solid = bytes((*colour, 255))
        for first, last, cover in _row_coverage(rect, radius, y, width):
            amount = cover * alpha
            if amount >= 0.999:
                pixels[(y * width + first) * 4:(y * width + last) * 4] = solid * (last - first)
            elif amount > 0:
                for x in range(first, last):
                    offset = (y * width + x) * 4
                    for channel in range(3):
                        pixels[offset + channel] = round(pixels[offset + channel] * (1 - amount) + colour[channel] * amount)


def _mix(first, second, amount):
    return tuple(round(a + (b - a) * amount) for a, b in zip(first, second))


# ============================================================================
# Painting
# ============================================================================

def paint_bodies():
    """Returns (width, height, plain RGBA, lit RGBA): the whole watch with the side button unlit and lit."""
    shape = layout()
    return shape.width, shape.height, _paint_body(BUTTON), _paint_body(BUTTON_LIT)


def _paint_body(button_colour):
    shape = layout()
    width = shape.width
    pixels = bytearray(bytes((*BACKGROUND, 255)) * (width * shape.height))

    strap_x = (width - STRAP_WIDTH) // 2
    _fill_rounded(pixels, width, Rect(strap_x, 0, STRAP_WIDTH, shape.height), 0, lambda y: STRAP_EDGE)
    _fill_rounded(pixels, width, Rect(strap_x + 2, 0, STRAP_WIDTH - 4, shape.height), 0, lambda y: STRAP)

    _fill_rounded(pixels, width, shape.button, 4, lambda y: button_colour)

    case = shape.case
    _fill_rounded(pixels, width, case, CASE_RADIUS, lambda y: CASE_RIM)
    inner = Rect(case.x + 2, case.y + 2, case.width - 4, case.height - 4)

    def case_colour(y):
        return _mix(CASE_TOP, CASE_BOTTOM, min(1.0, max(0.0, (y - case.y) / case.height)))

    _fill_rounded(pixels, width, inner, CASE_RADIUS - 2, case_colour)

    glass = Rect(shape.screen.x - BEZEL, shape.screen.y - BEZEL, SCREEN_SIZE + 2 * BEZEL, SCREEN_SIZE + 2 * BEZEL)
    _fill_rounded(pixels, width, glass, 14, lambda y: GLASS)
    return bytes(pixels)


def paint_glow():
    """Returns (width, height, RGBA bytes) of an amber halo around the case, shown while the motor runs."""
    shape = layout()
    width = shape.width
    pixels = bytearray(width * shape.height * 4)
    reach = 18
    for step in range(reach, -1, -1):
        alpha = 200 * (1 - step / reach) ** 2
        grown = Rect(shape.case.x - step, shape.case.y - step, shape.case.width + 2 * step, shape.case.height + 2 * step)
        for y in range(max(0, grown.y), min(shape.height, grown.y + grown.height)):
            for first, last, cover in _row_coverage(grown, CASE_RADIUS + step, y, width):
                level = round(alpha * cover)
                if cover >= 0.999:
                    # Each smaller ring is brighter, so its solid middle simply replaces what is there.
                    pixels[(y * width + first) * 4:(y * width + last) * 4] = bytes((*GLOW, level)) * (last - first)
                    continue
                offset = (y * width + first) * 4
                if level > pixels[offset + 3]:
                    pixels[offset:offset + 4] = bytes((*GLOW, level))

    # The halo shows only outside the case, which the body texture already draws.
    inside = Rect(shape.case.x + 3, shape.case.y + 3, shape.case.width - 6, shape.case.height - 6)
    for y in range(inside.y, inside.y + inside.height):
        for first, last, cover in _row_coverage(inside, CASE_RADIUS - 3, y, width):
            if cover >= 0.999:
                pixels[(y * width + first) * 4:(y * width + last) * 4] = bytes(4 * (last - first))
    return width, shape.height, bytes(pixels)
