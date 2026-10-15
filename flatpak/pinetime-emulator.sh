#!/bin/sh
# SPDX-License-Identifier: GPL-3.0-or-later
# Starts the watch window inside the Flatpak, or the bare engine when the first argument is "engine".

parts=/app/share/pinetime-emulator

export PINETIME_EMULATOR_ENGINE="${PINETIME_EMULATOR_ENGINE:-/app/bin/pinetime-engine}"
export PINETIME_EMULATOR_INFINITIME="${PINETIME_EMULATOR_INFINITIME:-$parts/bootloader_infinitime-1.16.1.hex}"
export PINETIME_EMULATOR_HCI_CONTROLLER="${PINETIME_EMULATOR_HCI_CONTROLLER:-$parts/nrf52840-hci-uart.elf}"
export PINETIME_EMULATOR_RUNTIME_DIR="$XDG_RUNTIME_DIR/pinetime-emulator"
mkdir -p "$PINETIME_EMULATOR_RUNTIME_DIR"

if [ "$1" = "engine" ]; then
    shift
    exec "$PINETIME_EMULATOR_ENGINE" "$@"
fi

# Without --device=all the sandbox has its own terminals, so the link would name one the host cannot reach.
for argument in "$@"; do
    if [ "$argument" = "--bluetooth" ] && [ -L /dev/ptmx ]; then
        echo "pinetime-emulator: --bluetooth needs the host's terminals; run it with" >&2
        echo "  flatpak run --device=all io.github.beatlink.PineTimeEmulator $*" >&2
        exit 2
    fi
done

# Opened from a desktop menu with no arguments, it runs the InfiniTime it carries.
if [ $# -eq 0 ]; then
    set -- --infinitime
fi

exec python3 -s -m pinetime_emulator "$@"
