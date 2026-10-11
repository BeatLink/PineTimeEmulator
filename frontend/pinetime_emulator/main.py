# SPDX-License-Identifier: GPL-3.0-or-later
"""The pinetime-emulator command: starts Renode with the watch and opens the window onto it."""

import argparse
import os
import shutil
import signal
import sys
import tempfile
from pathlib import Path

from .renode import ImageError, Renode, parse_image

# ============================================================================
# Arguments
# ============================================================================


def build_parser():
    parser = argparse.ArgumentParser(
        prog="pinetime-emulator",
        description="Runs PineTime firmware in Renode and shows the watch in a window.",
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
    parser.add_argument("--root", metavar="DIR",
                        help="the PineTimeEmulator tree holding scripts/, platform/ and peripherals/")
    parser.add_argument("--renode", metavar="COMMAND", help="the Renode command to run")
    parser.add_argument("--verbose", action="store_true", help="echo monitor commands and replies")
    return parser


def find_root(given):
    """The emulator tree: --root, then $PINETIME_EMULATOR_ROOT, then the source tree this package sits in."""
    candidates = [given, os.environ.get("PINETIME_EMULATOR_ROOT"), Path(__file__).resolve().parents[2]]
    for candidate in candidates:
        if candidate and (Path(candidate) / "scripts" / "window.resc").is_file():
            return Path(candidate).resolve()
    raise SystemExit("pinetime-emulator: cannot find scripts/window.resc; pass --root")


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
    for candidate in (os.environ.get("XDG_RUNTIME_DIR"), "/dev/shm"):
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

    root = find_root(arguments.root)
    runtime_directory = tempfile.mkdtemp(prefix="pinetime-emulator-", dir=runtime_parent())
    bluetooth = None
    if arguments.bluetooth:
        controller = os.environ.get("PINETIME_EMULATOR_HCI_CONTROLLER")
        if not controller:
            parser.error("--bluetooth needs $PINETIME_EMULATOR_HCI_CONTROLLER; run through `nix run . -- --bluetooth`")
        bluetooth = (os.path.join(runtime_directory, "hci"), controller)
    emulator = Renode(root, runtime_directory, arguments.renode, arguments.verbose, bluetooth)
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
                print(f"renode: {line}", file=sys.stderr)
            keep = Path(tempfile.gettempdir()) / "pinetime-emulator-renode.log"
            try:
                shutil.copyfile(emulator.log_path, keep)
                print(f"pinetime-emulator: Renode's log is saved at {keep}", file=sys.stderr)
            except OSError:
                pass
        shutil.rmtree(runtime_directory, ignore_errors=True)
    return 0 if succeeded else 1


if __name__ == "__main__":
    sys.exit(main())
