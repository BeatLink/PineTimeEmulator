# SPDX-License-Identifier: GPL-3.0-or-later
"""The pinetime-emulator command: starts the emulator engine with the watch and opens the window onto it."""

import argparse
import os
import shutil
import signal
import sys
import tempfile
from pathlib import Path

from .engine import Engine, ImageError, parse_image

# ============================================================================
# Arguments
# ============================================================================


def build_parser():
    parser = argparse.ArgumentParser(
        prog="pinetime-emulator",
        description="Runs PineTime firmware in the emulator and shows the watch in a window.",
    )
    parser.add_argument("--infinitime", action="store_true",
                        help="load the InfiniTime app ELF this package was built with")
    parser.add_argument("--image", action="append", default=[], metavar="FILE[@ADDRESS]",
                        help="load an image into the chip; .elf/.out and .hex carry addresses, .bin needs @ADDRESS (repeatable)")
    parser.add_argument("--flash-image", metavar="FILE",
                        help="load a raw image into the 4 MB external SPI flash")
    parser.add_argument("--bluetooth", action="store_true",
                        help="add an emulated Bluetooth controller on a pseudo-terminal that btattach can hand to BlueZ")
    parser.add_argument("--scale", type=int, default=1, metavar="N",
                        help="draw the watch N times larger")
    parser.add_argument("--screenshot-after", nargs=2, metavar=("SECONDS", "PATH"),
                        help="run without a window, save the window image once SECONDS of emulated time have passed, then quit")
    parser.add_argument("--engine", metavar="COMMAND", help="the pinetime-engine command to run")
    parser.add_argument("--verbose", action="store_true", help="echo monitor commands and replies")
    return parser


def collect_images(arguments, parser):
    images = []
    if arguments.infinitime:
        elf = os.environ.get("PINETIME_EMULATOR_INFINITIME")
        if not elf:
            parser.error("--infinitime needs $PINETIME_EMULATOR_INFINITIME; run through `nix run . -- --infinitime`")
        images.append(parse_image(elf))
    for text in arguments.image:
        images.append(parse_image(text))
    if not images:
        parser.error("give firmware with --infinitime or --image")
    return images


def runtime_parent():
    """The folder for the run's shared frame and Bluetooth link; a sandbox names one the host can see."""
    candidates = (os.environ.get("PINETIME_EMULATOR_RUNTIME_DIR"), os.environ.get("XDG_RUNTIME_DIR"), "/dev/shm")
    for candidate in candidates:
        if candidate and os.path.isdir(candidate) and os.access(candidate, os.W_OK):
            return candidate
    return None


# ============================================================================
# Entry point
# ============================================================================


def main(argv=None):
    parser = build_parser()
    arguments = parser.parse_args(argv)
    try:
        images = collect_images(arguments, parser)
        if arguments.flash_image and not Path(arguments.flash_image).is_file():
            raise ImageError(f"{arguments.flash_image}: no such file")
    except ImageError as error:
        parser.error(str(error))

    screenshot_after = screenshot_path = None
    if arguments.screenshot_after:
        try:
            screenshot_after = float(arguments.screenshot_after[0])
        except ValueError:
            parser.error("--screenshot-after needs a number of seconds")
        screenshot_path = os.path.abspath(arguments.screenshot_after[1])

    controller = os.environ.get("PINETIME_EMULATOR_HCI_CONTROLLER")
    if arguments.bluetooth and not controller:
        parser.error("--bluetooth needs $PINETIME_EMULATOR_HCI_CONTROLLER; run through `nix run . -- --bluetooth`")

    runtime_directory = tempfile.mkdtemp(prefix="pinetime-emulator-", dir=runtime_parent())
    bluetooth = (os.path.join(runtime_directory, "hci"), controller) if arguments.bluetooth else None
    emulator = Engine(runtime_directory, arguments.engine, arguments.verbose, bluetooth)
    flash_image = str(Path(arguments.flash_image).resolve()) if arguments.flash_image else None

    # The window module loads SDL, so a bad command line fails fast without it.
    from .window import Window

    window = None
    succeeded = False
    previous_sigterm = signal.signal(signal.SIGTERM, lambda *_: sys.exit(1))
    try:
        emulator.start(images, flash_image)
        if bluetooth:
            print("pinetime-emulator: the Bluetooth controller is on", bluetooth[0])
            print(f"pinetime-emulator: hand it to BlueZ with: sudo btattach -B {bluetooth[0]} -P h4 -S 1000000")
        window = Window(emulator, arguments.scale, headless=screenshot_after is not None)
        timeout = None if screenshot_after is None else 120 + 20 * screenshot_after
        succeeded = window.run(screenshot_after, screenshot_path, timeout)
    except KeyboardInterrupt:
        succeeded = True
    except RuntimeError as error:
        print(f"pinetime-emulator: {error}", file=sys.stderr)
    finally:
        signal.signal(signal.SIGTERM, previous_sigterm)
        if window is not None:
            window.close()
        emulator.stop()
        if not succeeded:
            for line in list(emulator.recent_output)[-15:]:
                print(f"engine: {line}", file=sys.stderr)
            keep = Path(tempfile.gettempdir()) / "pinetime-emulator-engine.log"
            try:
                shutil.copyfile(emulator.log_path, keep)
                print(f"pinetime-emulator: the engine's log is saved at {keep}", file=sys.stderr)
            except OSError:
                pass
        shutil.rmtree(runtime_directory, ignore_errors=True)
    return 0 if succeeded else 1


if __name__ == "__main__":
    sys.exit(main())
