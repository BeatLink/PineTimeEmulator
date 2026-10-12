# SPDX-License-Identifier: GPL-3.0-or-later
"""Starts the emulator engine as a child process and drives its monitor over a TCP socket."""

import os
import re
import shutil
import socket
import subprocess
import threading
import time
from collections import deque
from pathlib import Path

# ============================================================================
# Monitor paths of the watch parts
# ============================================================================

LCD = "sysbus.serial0.spiBus.lcd"
TOUCH = "sysbus.serial1.touch"
SIDE_BUTTON = "sysbus.gpio0.sideButton"
BATTERY = "sysbus.gpio0.battery"
ACCELEROMETER = "sysbus.serial1.accelerometer"
HEART_RATE = "sysbus.serial1.heartRateSensor"
POWER = "sysbus.powerClock"
BRIDGE = "sysbus.gpio0.windowBridge"

ELF_SUFFIXES = {".elf", ".out", ".axf"}
EXTERNAL_FLASH_ADDRESS = 0x60000000

# The prompt the engine prints after every reply.
_PROMPT = re.compile(rb"\(pinetime\) ")


# ============================================================================
# Firmware images
# ============================================================================

class ImageError(ValueError):
    pass


def parse_image(text):
    """Splits FILE[@ADDRESS] into a path and an optional address; .bin files need the address."""
    path, address = text, None
    if "@" in text:
        head, tail = text.rsplit("@", 1)
        try:
            address = int(tail, 0)
            path = head
        except ValueError:
            pass
    suffix = Path(path).suffix.lower()
    if suffix == ".bin" and address is None:
        raise ImageError(f"{path}: a .bin image needs a load address, as FILE@0xADDRESS")
    if suffix in ELF_SUFFIXES | {".hex"} and address is not None:
        raise ImageError(f"{path}: .elf and .hex images carry their own addresses")
    if not Path(path).is_file():
        raise ImageError(f"{path}: no such file")
    return str(Path(path).resolve()), address


def load_command(path, address):
    suffix = Path(path).suffix.lower()
    if suffix in ELF_SUFFIXES:
        return f"sysbus LoadELF {quote_path(path)}"
    if suffix == ".hex":
        return f"sysbus LoadHEX {quote_path(path)}"
    return f"sysbus LoadBinary {quote_path(path)} {address:#x}"


def quote_path(path):
    return f"@{path}"


def quote_string(text):
    return '"' + text.replace("\\", "\\\\").replace('"', '\\"') + '"'


# ============================================================================
# Engine process and monitor
# ============================================================================


class Engine:
    """One pinetime-engine process with the watch built in it, controlled through its monitor port."""

    def __init__(self, runtime_directory, engine_command=None, verbose=False, bluetooth=None):
        self.runtime_directory = Path(runtime_directory)
        # A (pseudo-terminal link, controller firmware) pair, or None for no Bluetooth bridge.
        self.bluetooth = bluetooth
        self.engine_command = engine_command or os.environ.get("PINETIME_EMULATOR_ENGINE") or shutil.which("pinetime-engine") or "pinetime-engine"
        self.verbose = verbose
        self.process = None
        self.connection = None
        self.send_lock = threading.Lock()
        self.recent_output = deque(maxlen=200)
        self.frame_path = self.runtime_directory / "frame"
        self.status_path = self.runtime_directory / "status"
        self.log_path = self.runtime_directory / "engine.log"

    # ------------------------------------------------------------------------
    # Start and stop
    # ------------------------------------------------------------------------

    def start(self, images, flash_image=None, connect_timeout=30):
        port = _free_port()
        self.log_file = open(self.log_path, "wb")
        self.process = subprocess.Popen(
            self._engine_arguments(port),
            stdin=subprocess.DEVNULL, stdout=self.log_file, stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        self._connect(port, connect_timeout)
        threading.Thread(target=self._drain, daemon=True).start()
        for command in self._session_commands(images, flash_image):
            self.send(command)

    def _engine_arguments(self, port):
        arguments = [self.engine_command, "--port", str(port)]
        if self.bluetooth:
            link, controller = self.bluetooth
            arguments += ["--bluetooth", str(link), "--hci-controller", str(controller)]
        return arguments

    def _session_commands(self, images, flash_image):
        commands = ["mach create"]
        for path, address in images:
            commands.append(load_command(path, address))
        if flash_image:
            commands.append(f"sysbus LoadBinary {quote_path(flash_image)} {EXTERNAL_FLASH_ADDRESS:#x}")
        commands += [
            f"{LCD} ShareFramesThrough {quote_string(str(self.frame_path))}",
            f"{BRIDGE} ShareStatusThrough {quote_string(str(self.status_path))}",
            "start",
        ]
        return commands

    def _connect(self, port, timeout):
        deadline = time.monotonic() + timeout
        while True:
            if self.process.poll() is not None:
                raise RuntimeError(f"the engine exited with code {self.process.returncode}; see {self.log_path}")
            try:
                self.connection = socket.create_connection(("127.0.0.1", port), timeout=1)
                self.connection.settimeout(None)
                self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
                return
            except OSError:
                if time.monotonic() > deadline:
                    raise RuntimeError(f"the engine did not open port {port}; see {self.log_path}")
                time.sleep(0.1)

    def stop(self):
        if self.connection is not None:
            try:
                self.connection.sendall(b"quit\n")
            except OSError:
                pass
        if self.process is not None:
            try:
                self.process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait()
        if self.connection is not None:
            self.connection.close()
            self.connection = None

    def running(self):
        return self.process is not None and self.process.poll() is None

    # ------------------------------------------------------------------------
    # Monitor commands
    # ------------------------------------------------------------------------

    def send(self, command):
        """Queues a monitor command without waiting for its reply, so mouse drags stay live."""
        if self.verbose:
            print(f"> {command}", flush=True)
        with self.send_lock:
            try:
                self.connection.sendall(command.encode() + b"\n")
            except OSError:
                pass

    def _drain(self):
        pending = b""
        while True:
            try:
                chunk = self.connection.recv(65536)
            except OSError:
                return
            if not chunk:
                return
            pending += _PROMPT.sub(b"\n", chunk)
            *lines, pending = re.split(rb"\r?\n", pending)
            for raw_line in lines:
                line = raw_line.decode(errors="replace").strip()
                if not line:
                    continue
                self.recent_output.append(line)
                if self.verbose or _looks_like_error(line):
                    print(f"engine: {line}", flush=True)


def _looks_like_error(line):
    lowered = line.lower()
    return "error" in lowered or "could not" in lowered or "exception" in lowered or "no such" in lowered


def _free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]
