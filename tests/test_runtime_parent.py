# SPDX-License-Identifier: GPL-3.0-or-later
"""Checks where the window keeps its run's shared frame and Bluetooth link."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent / "frontend"))

from pinetime_emulator.main import runtime_parent  # noqa: E402


def test_the_named_runtime_folder_comes_first(tmp_path, monkeypatch):
    named = tmp_path / "named"
    named.mkdir()
    monkeypatch.setenv("PINETIME_EMULATOR_RUNTIME_DIR", str(named))
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert runtime_parent() == str(named)


def test_a_missing_named_folder_falls_back_to_the_session_folder(tmp_path, monkeypatch):
    monkeypatch.setenv("PINETIME_EMULATOR_RUNTIME_DIR", str(tmp_path / "missing"))
    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    assert runtime_parent() == str(tmp_path)
