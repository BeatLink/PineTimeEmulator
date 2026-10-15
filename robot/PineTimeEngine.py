# SPDX-License-Identifier: GPL-3.0-or-later
"""Robot Framework keywords that run tests against pinetime-engine, named like Renode's so suites move over unchanged."""

import os
import re
import shutil
import socket
import subprocess
import time

PROMPT = b"(pinetime) "

# Renode watchpoint hooks run Python; the one the BattleTime suites use logs each write to a file.
_LOGGING_HOOK = re.compile(
    r"""^sysbus AddWatchpointHook (\S+) (\S+) (\S+) "open\('([^']+)', 'a'\)\.write\('%x %x %x\\\\?n' % \(\S+, self\.GetCurrentCPU\(\)\.PC\.RawValue, value\)\)"$"""
)


class PineTimeEngine:
    ROBOT_LIBRARY_SCOPE = "GLOBAL"

    def __init__(self, engine_command=None):
        self.engine_command = engine_command or os.environ.get("PINETIME_EMULATOR_ENGINE") or shutil.which("pinetime-engine") or "pinetime-engine"
        self.process = None
        self.connection = None
        self.pending = b""

    # ------------------------------------------------------------------------
    # Suite and test lifetime
    # ------------------------------------------------------------------------

    def setup(self):
        """Starts the engine and connects to its monitor."""
        port = _free_port()
        self.process = subprocess.Popen([self.engine_command, "--port", str(port)], stdin=subprocess.DEVNULL)
        deadline = time.monotonic() + 30
        while True:
            try:
                self.connection = socket.create_connection(("127.0.0.1", port), timeout=1)
                break
            except OSError:
                if self.process.poll() is not None or time.monotonic() > deadline:
                    raise RuntimeError("pinetime-engine did not open its monitor port")
                time.sleep(0.05)
        self.connection.settimeout(None)
        self._read_reply()

    def teardown(self):
        if self.connection is not None:
            try:
                self.connection.sendall(b"quit\n")
            except OSError:
                pass
            self.connection.close()
            self.connection = None
        if self.process is not None:
            try:
                self.process.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self.process.kill()
            self.process = None

    def reset_emulation(self):
        """Throws the watch away; the next test builds a new one."""
        self.execute_command("mach create")

    def test_teardown(self):
        pass

    # ------------------------------------------------------------------------
    # Commands
    # ------------------------------------------------------------------------

    def execute_command(self, command):
        """Runs one monitor command and returns its output; an error reply fails the keyword."""
        hook = _LOGGING_HOOK.match(command.strip())
        if hook:
            address, width, access, path = hook.groups()
            command = f"sysbus AddWatchpointLog {address} {width} {access} @{path}"
        self.connection.sendall(command.encode() + b"\n")
        reply = self._read_reply()
        if reply.startswith("error:"):
            raise AssertionError(f"{command}: {reply[len('error:'):].strip()}")
        return reply

    def _read_reply(self):
        while PROMPT not in self.pending:
            chunk = self.connection.recv(1 << 20)
            if not chunk:
                raise RuntimeError("pinetime-engine closed its monitor")
            self.pending += chunk
        reply, self.pending = self.pending.split(PROMPT, 1)
        return reply.decode(errors="replace").strip()


def _free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]
