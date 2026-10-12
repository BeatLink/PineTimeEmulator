# SPDX-License-Identifier: GPL-3.0-or-later
"""The desktop window: the drawn watch showing the emulated screen, mouse and keys as inputs, and a side panel of controls."""

import ctypes
import os
import shutil
import subprocess
import time
from collections import deque
from dataclasses import dataclass, field
from typing import Callable

import sdl2
import sdl2.sdlttf as ttf

from . import engine as parts
from . import watch_body
from .png import write_rgb_png
from .shared import FrameReader, StatusReader

# ============================================================================
# Look and timing
# ============================================================================

PANEL_WIDTH = 280
STATUS_HEIGHT = 28
FONT_SIZE = 13
FRAME_INTERVAL = 1 / 60
BUZZ_AFTERGLOW = 0.25
SHAKE_PIXELS = 2

# A press shorter than this reaches the engine stretched to it, since a scripted click lasts no emulated time and the firmware polls too late to see it.
MINIMUM_PRESS_SECONDS = 0.08
IDLE_REDRAW_INTERVAL = 0.25

PANEL_BACKGROUND = (38, 40, 45)
STATUS_BACKGROUND = (24, 25, 28)
BUTTON_FACE = (58, 61, 68)
BUTTON_FACE_ACTIVE = (52, 110, 150)
BUTTON_BORDER = (82, 86, 95)
TEXT = (222, 224, 228)
TEXT_DIM = (150, 154, 162)
HEADING = (128, 196, 255)

# Wrist poses in mg; InfiniTime wakes on a roll from arm-down towards the face.
TILT_PRESETS = {
    "lowered": (0, 1000, 0),
    "flat": (0, 0, -1000),
    "looking": (0, -500, -866),
}

# wasp-os inverts the arrows: Down asks for what is below, which is an upward swipe.
ARROW_SWIPES = {
    sdl2.SDLK_DOWN: "up",
    sdl2.SDLK_UP: "down",
    sdl2.SDLK_LEFT: "right",
    sdl2.SDLK_RIGHT: "left",
}

HELP_LINES = [
    "Mouse   finger on the glass",
    "Arrows  swipe (Down: swipe up)",
    "Tab     hold the side button",
    "s       save a screenshot",
    "q, Esc  quit",
]


@dataclass
class PanelButton:
    rect: watch_body.Rect
    label: Callable[[], str]
    action: Callable[[], None]
    active: Callable[[], bool] = lambda: False


@dataclass
class PanelText:
    x: int
    y: int
    text: Callable[[], str]
    colour: tuple = TEXT


@dataclass
class Inputs:
    """What the window last asked the watch for, where the engine has no cheap way to report it."""
    heart_rate: int = 70
    tilt: str = "flat"
    paused: bool = False
    finger_down: bool = False
    finger_sent: tuple = (-1, -1)
    finger_pending: tuple = None
    side_button_held_by: set = field(default_factory=set)
    finger_pressed_at: float = 0.0
    side_button_pressed_at: float = 0.0


# ============================================================================
# Window
# ============================================================================

class Window:
    def __init__(self, emulator, scale=1, headless=False):
        self.emulator = emulator
        self.scale = max(1, int(scale))
        self.headless = headless
        self.inputs = Inputs()
        self.frames = FrameReader(emulator.frame_path)
        self.statuses = StatusReader(emulator.status_path)
        self.frame = None
        self.status = None
        self.last_motor_starts = 0
        self.buzz_until = 0.0
        self.frame_number = 0
        self.timed_actions = []
        self.speed_samples = deque()
        self.text_cache = {}
        self.quit_requested = False

        self.shape = watch_body.layout()
        self.watch_width = self.shape.width * self.scale
        self.watch_height = self.shape.height * self.scale
        self.panel_items, panel_height = self._build_panel()
        self.width = self.watch_width + PANEL_WIDTH
        self.height = max(self.watch_height, panel_height) + STATUS_HEIGHT
        self._open_sdl()

    # ------------------------------------------------------------------------
    # SDL setup
    # ------------------------------------------------------------------------

    def _open_sdl(self):
        if self.headless:
            os.environ["SDL_VIDEODRIVER"] = "dummy"
        if sdl2.SDL_Init(sdl2.SDL_INIT_VIDEO) != 0:
            raise RuntimeError(f"SDL could not start: {sdl2.SDL_GetError().decode()}")
        flags = sdl2.SDL_WINDOW_HIDDEN if self.headless else sdl2.SDL_WINDOW_SHOWN
        self.window = sdl2.SDL_CreateWindow(b"PineTime emulator", sdl2.SDL_WINDOWPOS_CENTERED, sdl2.SDL_WINDOWPOS_CENTERED,
                                            self.width, self.height, flags)
        renderer_flags = sdl2.SDL_RENDERER_SOFTWARE if self.headless else sdl2.SDL_RENDERER_ACCELERATED | sdl2.SDL_RENDERER_PRESENTVSYNC
        self.renderer = sdl2.SDL_CreateRenderer(self.window, -1, renderer_flags)
        if not self.renderer:
            self.renderer = sdl2.SDL_CreateRenderer(self.window, -1, sdl2.SDL_RENDERER_SOFTWARE)
        sdl2.SDL_SetRenderDrawBlendMode(self.renderer, sdl2.SDL_BLENDMODE_BLEND)

        width, height, plain, lit = watch_body.paint_bodies()
        self.body_texture = self._static_texture(width, height, plain)
        self.lit_body_texture = self._static_texture(width, height, lit)
        self.glow_texture = self._static_texture(*watch_body.paint_glow())
        self.screen_texture = sdl2.SDL_CreateTexture(self.renderer, sdl2.SDL_PIXELFORMAT_RGB24, sdl2.SDL_TEXTUREACCESS_STREAMING,
                                                     watch_body.SCREEN_SIZE, watch_body.SCREEN_SIZE)
        sdl2.SDL_SetTextureScaleMode(self.screen_texture, sdl2.SDL_ScaleModeNearest)
        black = bytes(watch_body.SCREEN_SIZE * watch_body.SCREEN_SIZE * 3)
        sdl2.SDL_UpdateTexture(self.screen_texture, None, black, watch_body.SCREEN_SIZE * 3)

        self.font = None
        font_path = find_font()
        if font_path and ttf.TTF_Init() == 0:
            self.font = ttf.TTF_OpenFont(font_path.encode(), FONT_SIZE)
        if not self.font:
            print("pinetime-emulator: no TrueType font found; set PINETIME_EMULATOR_FONT to show labels", flush=True)

    def _static_texture(self, width, height, rgba):
        texture = sdl2.SDL_CreateTexture(self.renderer, sdl2.SDL_PIXELFORMAT_ABGR8888, sdl2.SDL_TEXTUREACCESS_STATIC, width, height)
        sdl2.SDL_UpdateTexture(texture, None, rgba, width * 4)
        sdl2.SDL_SetTextureBlendMode(texture, sdl2.SDL_BLENDMODE_BLEND)
        sdl2.SDL_SetTextureScaleMode(texture, sdl2.SDL_ScaleModeLinear)
        return texture

    def close(self):
        if self.font:
            ttf.TTF_CloseFont(self.font)
            ttf.TTF_Quit()
        sdl2.SDL_DestroyRenderer(self.renderer)
        sdl2.SDL_DestroyWindow(self.window)
        sdl2.SDL_Quit()
        self.frames.close()
        self.statuses.close()

    # ------------------------------------------------------------------------
    # Main loop
    # ------------------------------------------------------------------------

    def run(self, screenshot_after=None, screenshot_path=None, timeout=None):
        """Runs until quit; with screenshot_after it saves the window once that much emulated time has passed and returns."""
        started = time.monotonic()
        last_drawn = 0.0
        while not self.quit_requested:
            loop_started = time.monotonic()
            changed = self._handle_events()
            changed |= self._run_timed_actions()
            self._flush_finger()
            changed |= self._read_outputs()
            changed |= loop_started < self.buzz_until + FRAME_INTERVAL
            # An unchanged window is redrawn only a few times a second, for the clock in the status line.
            draw = changed or loop_started - last_drawn > IDLE_REDRAW_INTERVAL or screenshot_after is not None
            if draw:
                self._draw()
                last_drawn = loop_started

            if screenshot_after is not None and self.status and self.status.emulated_seconds >= screenshot_after and self.frame:
                self.save_screenshot(screenshot_path)
                return True
            if not self.emulator.running():
                print(f"pinetime-emulator: the engine stopped; its log is {self.emulator.log_path}", flush=True)
                return False
            if timeout is not None and time.monotonic() - started > timeout:
                print("pinetime-emulator: timed out waiting for emulated time", flush=True)
                return False

            if draw:
                sdl2.SDL_RenderPresent(self.renderer)
            spare = FRAME_INTERVAL - (time.monotonic() - loop_started)
            if spare > 0:
                time.sleep(spare)
        return True

    def _read_outputs(self):
        """Takes in the newest frame and status; true when the picture changed."""
        changed = False
        frame = self.frames.read(self.frame.count if self.frame else None)
        if frame:
            if self.frame is None or frame.pixels != self.frame.pixels:
                sdl2.SDL_UpdateTexture(self.screen_texture, None, frame.pixels, frame.width * 3)
                changed = True
            changed |= self.frame is None or frame.backlight_level != self.frame.backlight_level
            self.frame = frame
        status = self.statuses.read()
        if status:
            now = time.monotonic()
            if status.motor_starts != self.last_motor_starts:
                self.last_motor_starts = status.motor_starts
                self.buzz_until = now + BUZZ_AFTERGLOW
            if status.motor_running:
                self.buzz_until = now + BUZZ_AFTERGLOW
            self.inputs.paused = status.paused
            self.speed_samples.append((now, status.emulated_seconds))
            while len(self.speed_samples) > 2 and now - self.speed_samples[0][0] > 2.0:
                self.speed_samples.popleft()
            changed |= self.status is None or (status.button_pressed, status.charger_connected, status.paused) != (
                self.status.button_pressed, self.status.charger_connected, self.status.paused)
            self.status = status
        return changed

    # ------------------------------------------------------------------------
    # Input
    # ------------------------------------------------------------------------

    def _handle_events(self):
        event = sdl2.SDL_Event()
        handled = False
        while sdl2.SDL_PollEvent(ctypes.byref(event)):
            handled = True
            kind = event.type
            if kind == sdl2.SDL_QUIT:
                self.quit_requested = True
            elif kind == sdl2.SDL_KEYDOWN and not event.key.repeat:
                self._key_down(event.key.keysym.sym, event.key.keysym.mod)
            elif kind == sdl2.SDL_KEYUP:
                if event.key.keysym.sym == sdl2.SDLK_TAB:
                    self._release_side_button("tab")
            elif kind == sdl2.SDL_MOUSEBUTTONDOWN and event.button.button == sdl2.SDL_BUTTON_LEFT:
                self._mouse_down(event.button.x, event.button.y)
            elif kind == sdl2.SDL_MOUSEMOTION:
                if self.inputs.finger_down:
                    self.inputs.finger_pending = self._screen_point(event.motion.x, event.motion.y)
            elif kind == sdl2.SDL_MOUSEBUTTONUP and event.button.button == sdl2.SDL_BUTTON_LEFT:
                self._mouse_up(event.button.x, event.button.y)
        return handled

    def _key_down(self, key, modifiers):
        shift = bool(modifiers & sdl2.KMOD_SHIFT)
        if key in (sdl2.SDLK_ESCAPE, sdl2.SDLK_q):
            self.quit_requested = True
        elif key in ARROW_SWIPES:
            self.send(f'{parts.TOUCH} Swipe "{ARROW_SWIPES[key]}"')
        elif key == sdl2.SDLK_TAB:
            self._press_side_button("tab")
        elif key == sdl2.SDLK_s:
            self.save_screenshot(None)
        elif key == sdl2.SDLK_LEFTBRACKET:
            self.change_battery(-10)
        elif key == sdl2.SDLK_RIGHTBRACKET:
            self.change_battery(+10)
        elif key == sdl2.SDLK_c:
            self.toggle_charger()
        elif key == sdl2.SDLK_COMMA:
            self.change_heart_rate(-5)
        elif key == sdl2.SDLK_PERIOD:
            self.change_heart_rate(+5)
        elif key == sdl2.SDLK_t:
            self.add_steps(1000 if shift else 100)
        elif key in (sdl2.SDLK_1, sdl2.SDLK_2, sdl2.SDLK_3):
            self.set_tilt(("lowered", "flat", "looking")[key - sdl2.SDLK_1])
        elif key == sdl2.SDLK_w:
            self.raise_wrist()
        elif key == sdl2.SDLK_r:
            self.reset()
        elif key == sdl2.SDLK_o:
            self.power_cycle()
        elif key == sdl2.SDLK_SPACE:
            self.toggle_pause()

    def _mouse_down(self, x, y):
        sx, sy = self._watch_point(x, y)
        if self.shape.screen.contains(sx, sy):
            point = self._screen_point(x, y)
            self.inputs.finger_down = True
            self.inputs.finger_pressed_at = time.monotonic()
            self.inputs.finger_pending = None
            self.inputs.finger_sent = point
            self.send(f"{parts.TOUCH} MoveTo {point[0]} {point[1]}")
            self.send(f"{parts.TOUCH} Press")
            return
        if self._near_side_button(sx, sy):
            self._press_side_button("mouse")
            return
        for item in self.panel_items:
            if isinstance(item, PanelButton) and item.rect.contains(x, y):
                item.action()
                return

    def _mouse_up(self, x, y):
        if self.inputs.finger_down:
            self.inputs.finger_pending = self._screen_point(x, y)
            self._flush_finger()
            self.inputs.finger_down = False
            self._after_minimum_press(self.inputs.finger_pressed_at, lambda: self.send(f"{parts.TOUCH} Release"))
        self._release_side_button("mouse")

    def _flush_finger(self):
        """Sends at most one move per window frame, so a fast drag never queues a backlog."""
        pending = self.inputs.finger_pending
        if self.inputs.finger_down and pending and pending != self.inputs.finger_sent:
            self.send(f"{parts.TOUCH} MoveTo {pending[0]} {pending[1]}")
            self.inputs.finger_sent = pending
        self.inputs.finger_pending = None

    def _watch_point(self, x, y):
        return x // self.scale, y // self.scale

    def _screen_point(self, x, y):
        sx, sy = x / self.scale - self.shape.screen.x, y / self.scale - self.shape.screen.y
        last = watch_body.SCREEN_SIZE - 1
        return max(0, min(last, int(sx))), max(0, min(last, int(sy)))

    def _near_side_button(self, sx, sy):
        button = self.shape.button
        grown = watch_body.Rect(button.x - 4, button.y - 6, button.width + 10, button.height + 12)
        return grown.contains(sx, sy)

    def _press_side_button(self, source):
        if not self.inputs.side_button_held_by:
            self.send(f"{parts.SIDE_BUTTON} Press")
            self.inputs.side_button_pressed_at = time.monotonic()
        self.inputs.side_button_held_by.add(source)

    def _release_side_button(self, source):
        if source in self.inputs.side_button_held_by:
            self.inputs.side_button_held_by.discard(source)
            if not self.inputs.side_button_held_by:
                self._after_minimum_press(self.inputs.side_button_pressed_at, lambda: self.send(f"{parts.SIDE_BUTTON} Release"))

    def _after_minimum_press(self, pressed_at, release):
        wait = MINIMUM_PRESS_SECONDS - (time.monotonic() - pressed_at)
        if wait <= 0:
            release()
        else:
            self.timed_actions.append((time.monotonic() + wait, release))

    # ------------------------------------------------------------------------
    # Actions on the watch
    # ------------------------------------------------------------------------

    def send(self, command):
        self.emulator.send(command)

    def change_battery(self, delta):
        current = self.status.battery_percent if self.status else 75
        self.send(f"{parts.BATTERY} Percent {max(0, min(100, current + delta))}")

    def toggle_charger(self):
        connected = self.status.charger_connected if self.status else False
        self.send(f"{parts.BATTERY} ChargerConnected {'false' if connected else 'true'}")

    def change_heart_rate(self, delta):
        self.inputs.heart_rate = max(30, min(220, self.inputs.heart_rate + delta))
        self.send(f"{parts.HEART_RATE} SetHeartRate {self.inputs.heart_rate}")

    def add_steps(self, count):
        self.send(f"{parts.ACCELEROMETER} AddSteps {count}")

    def set_tilt(self, name):
        self.inputs.tilt = name
        x, y, z = TILT_PRESETS[name]
        self.send(f"{parts.ACCELEROMETER} SetAcceleration {x} {y} {z}")

    def raise_wrist(self):
        self.set_tilt("lowered")
        self.timed_actions.append((time.monotonic() + 0.6, lambda: self.set_tilt("looking")))

    def reset(self):
        self.send("machine Reset")

    def power_cycle(self):
        self.send(f"{parts.POWER} PowerCycle")

    def toggle_pause(self):
        self.send("start" if self.inputs.paused else "pause")
        self.inputs.paused = not self.inputs.paused

    def _run_timed_actions(self):
        now = time.monotonic()
        due = [action for when, action in self.timed_actions if when <= now]
        self.timed_actions = [(when, action) for when, action in self.timed_actions if when > now]
        for action in due:
            action()
        return bool(due)

    # ------------------------------------------------------------------------
    # Screenshots
    # ------------------------------------------------------------------------

    def save_screenshot(self, path):
        """Saves the whole window as it is drawn now; without a path it picks a dated name in the current folder."""
        if path is None:
            path = time.strftime("pinetime-%Y%m%d-%H%M%S.png")
        pitch = self.width * 3
        pixels = ctypes.create_string_buffer(pitch * self.height)
        sdl2.SDL_RenderReadPixels(self.renderer, None, sdl2.SDL_PIXELFORMAT_RGB24, pixels, pitch)
        write_rgb_png(path, self.width, self.height, pixels.raw, pitch)
        print(f"pinetime-emulator: saved {path}", flush=True)

    # ------------------------------------------------------------------------
    # Drawing
    # ------------------------------------------------------------------------

    def _draw(self):
        self.frame_number += 1
        renderer = self.renderer
        self._fill(watch_body.Rect(0, 0, self.width, self.height), watch_body.BACKGROUND)

        buzzing = time.monotonic() < self.buzz_until
        shake = 0
        if buzzing:
            shake = SHAKE_PIXELS * self.scale * (1 if self.frame_number % 2 else -1)

        target = sdl2.SDL_Rect(shake, 0, self.watch_width, self.watch_height)
        button_lit = bool(self.inputs.side_button_held_by) or bool(self.status and self.status.button_pressed)
        sdl2.SDL_RenderCopy(renderer, self.lit_body_texture if button_lit else self.body_texture, None, target)
        if buzzing:
            sdl2.SDL_RenderCopy(renderer, self.glow_texture, None, target)

        screen = self.shape.screen
        screen_target = sdl2.SDL_Rect(screen.x * self.scale + shake, screen.y * self.scale,
                                      screen.width * self.scale, screen.height * self.scale)
        sdl2.SDL_RenderCopy(renderer, self.screen_texture, None, screen_target)

        self._draw_panel()
        self._draw_status_line()

    def _fill(self, rect, colour):
        sdl2.SDL_SetRenderDrawColor(self.renderer, *colour, 255)
        sdl2.SDL_RenderFillRect(self.renderer, sdl2.SDL_Rect(rect.x, rect.y, rect.width, rect.height))

    def _outline(self, rect, colour):
        sdl2.SDL_SetRenderDrawColor(self.renderer, *colour, 255)
        sdl2.SDL_RenderDrawRect(self.renderer, sdl2.SDL_Rect(rect.x, rect.y, rect.width, rect.height))

    def _text(self, x, y, text, colour=TEXT, centre_width=None):
        if not self.font or not text:
            return
        key = (text, colour)
        cached = self.text_cache.get(key)
        if cached is None:
            surface = ttf.TTF_RenderUTF8_Blended(self.font, text.encode(), sdl2.SDL_Color(*colour, 255))
            if not surface:
                return
            texture = sdl2.SDL_CreateTextureFromSurface(self.renderer, surface)
            cached = (texture, surface.contents.w, surface.contents.h)
            sdl2.SDL_FreeSurface(surface)
            if len(self.text_cache) > 400:
                for old_texture, _, _ in self.text_cache.values():
                    sdl2.SDL_DestroyTexture(old_texture)
                self.text_cache.clear()
            self.text_cache[key] = cached
        texture, width, height = cached
        if centre_width is not None:
            x += (centre_width - width) // 2
        sdl2.SDL_RenderCopy(self.renderer, texture, None, sdl2.SDL_Rect(x, y, width, height))

    def _draw_panel(self):
        self._fill(watch_body.Rect(self.watch_width, 0, PANEL_WIDTH, self.height - STATUS_HEIGHT), PANEL_BACKGROUND)
        for item in self.panel_items:
            if isinstance(item, PanelButton):
                face = BUTTON_FACE_ACTIVE if item.active() else BUTTON_FACE
                self._fill(item.rect, face)
                self._outline(item.rect, BUTTON_BORDER)
                self._text(item.rect.x, item.rect.y + (item.rect.height - FONT_SIZE - 3) // 2, item.label(), TEXT, item.rect.width)
            else:
                self._text(item.x, item.y, item.text(), item.colour)

    def _draw_status_line(self):
        top = self.height - STATUS_HEIGHT
        self._fill(watch_body.Rect(0, top, self.width, STATUS_HEIGHT), STATUS_BACKGROUND)
        self._text(10, top + 6, self.status_text(), TEXT_DIM)

    def status_text(self):
        status = self.status
        if status is None:
            return "Starting the emulator..."
        frame = self.frame
        battery = f"Battery {status.battery_percent}%"
        if status.charging:
            battery += " charging"
        elif status.charger_connected:
            battery += " on charger"
        backlight = f"Backlight {frame.backlight_level}/3" if frame else "Backlight -"
        if frame and (frame.sleeping or not frame.display_on):
            backlight += " (screen off)"
        speed = ""
        if len(self.speed_samples) >= 2:
            (wall_first, emu_first), (wall_last, emu_last) = self.speed_samples[0], self.speed_samples[-1]
            if wall_last > wall_first:
                speed = f" ({100 * (emu_last - emu_first) / (wall_last - wall_first):.0f}% of real time)"
        state = "  PAUSED" if status.paused else ""
        return f"{battery}  |  {backlight}  |  Emulated {status.emulated_seconds:.1f} s{speed}{state}"

    # ------------------------------------------------------------------------
    # Side panel layout
    # ------------------------------------------------------------------------

    def _build_panel(self):
        items = []
        left = self.watch_width + 14
        inner_width = PANEL_WIDTH - 28
        y = 12
        row_height = 26
        gap = 6

        def heading(text):
            nonlocal y
            items.append(PanelText(left, y, lambda: text, HEADING))
            y += 20

        def value(text_function):
            nonlocal y
            items.append(PanelText(left, y, text_function, TEXT))
            y += 20

        def buttons(*specs):
            nonlocal y
            count = len(specs)
            width = (inner_width - gap * (count - 1)) // count
            for index, spec in enumerate(specs):
                label, action = spec[0], spec[1]
                active = spec[2] if len(spec) > 2 else (lambda: False)
                label_function = label if callable(label) else (lambda text=label: text)
                rect = watch_body.Rect(left + index * (width + gap), y, width, row_height - 4)
                items.append(PanelButton(rect, label_function, action, active))
            y += row_height

        heading("Watch")
        buttons(("Reset  r", self.reset), ("Power cycle  o", self.power_cycle))
        buttons((lambda: "Resume  space" if self.inputs.paused else "Pause  space", self.toggle_pause, lambda: self.inputs.paused))
        y += 6

        heading("Battery")
        value(lambda: f"{self.status.battery_percent}%  {self.status.battery_volts:.2f} V" if self.status else "-")
        buttons(("-10%  [", lambda: self.change_battery(-10)), ("+10%  ]", lambda: self.change_battery(+10)))
        buttons((lambda: "Unplug charger  c" if self.status and self.status.charger_connected else "Plug charger  c",
                 self.toggle_charger, lambda: bool(self.status and self.status.charger_connected)))
        y += 6

        heading("Heart rate")
        value(lambda: f"{self.inputs.heart_rate} bpm")
        buttons(("-5  ,", lambda: self.change_heart_rate(-5)), ("+5  .", lambda: self.change_heart_rate(+5)))
        y += 6

        heading("Steps")
        value(lambda: f"{self.status.steps} steps" if self.status else "-")
        buttons(("+100  t", lambda: self.add_steps(100)), ("+1000  T", lambda: self.add_steps(1000)))
        y += 6

        heading("Wrist")
        buttons(("Down  1", lambda: self.set_tilt("lowered"), lambda: self.inputs.tilt == "lowered"),
                ("Flat  2", lambda: self.set_tilt("flat"), lambda: self.inputs.tilt == "flat"),
                ("Look  3", lambda: self.set_tilt("looking"), lambda: self.inputs.tilt == "looking"))
        buttons(("Raise wrist  w", self.raise_wrist),)
        y += 6

        heading("Keys")
        for line in HELP_LINES:
            items.append(PanelText(left, y, lambda line=line: line, TEXT_DIM))
            y += 18
        return items, y + 10


# ============================================================================
# Fonts
# ============================================================================

def find_font():
    """A TrueType font for the labels: $PINETIME_EMULATOR_FONT, else what fontconfig offers."""
    configured = os.environ.get("PINETIME_EMULATOR_FONT")
    if configured and os.path.isfile(configured):
        return configured
    if shutil.which("fc-match"):
        try:
            found = subprocess.run(["fc-match", "-f", "%{file}", "DejaVu Sans Mono:style=Book"],
                                   capture_output=True, text=True, timeout=5).stdout.strip()
            if found and os.path.isfile(found):
                return found
        except (OSError, subprocess.SubprocessError):
            pass
    return None
