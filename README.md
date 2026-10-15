# PineTimeEmulator

A full hardware emulator for the [PineTime](https://pine64.org/devices/pinetime/) smartwatch,
written in Rust. It emulates the Cortex-M4 CPU, the nRF52832 chip and the parts of the watch
around it, so the images you would flash on a real watch run unchanged: InfiniTime, wasp-os,
BattleTime, the bootloaders and Nordic's SoftDevice. An idle watch runs several times faster than
real time.

## Status

InfiniTime 1.16.1 and BattleTime boot to their faces, and BattleTime's bootloader, update, recovery and
migration suites pass. See [docs/hardware.md](docs/hardware.md) for what is emulated,
[docs/rust-engine.md](docs/rust-engine.md) for how the engine is built, and
[docs/bluetooth.md](docs/bluetooth.md) for reaching the computer's Bluetooth, and
[docs/todo.md](docs/todo.md) for open work.

## Running it

```sh
nix run . -- --infinitime              # the watch window, running InfiniTime
nix run . -- --infinitime --bluetooth  # the same, with a controller btattach can hand to BlueZ
nix develop
cargo test                             # every model, CPU and engine test
pinetime-engine                        # the monitor on standard input
pinetime-test SUITE.robot              # Robot suites, as renode-test ran them
```

`pinetime-engine` takes the commands of Renode's monitor that the window and the tests use, such
as `sysbus LoadELF @image.elf`, `emulation RunFor "3"` and `sysbus.serial0.spiBus.lcd
SaveScreenshot @screen.png`; see [docs/hardware.md](docs/hardware.md).

Firmware images for suites to start from: `.#infinitime-app` (InfiniTime 1.16.1), `.#infinitime-mcuboot`
and `.#infinitime-bootloader` (InfiniTime's bootloader 5.0.4, laid out as in an InfiniTime checkout).

## Layout

```text
PineTimeEmulator/
├── flake.nix          packages, checks and the development shell
├── crates/            the engine: core, cortex-m4, nrf52832, board, emulator
├── frontend/          the desktop window (Python, SDL2)
├── robot/             Robot Framework keywords for pinetime-test
├── scripts/           pinetime.resc, the script suites include to build a watch
├── nix/               package definitions
├── tools/             hci_scan.py, which checks the Bluetooth bridge without BlueZ
└── tests/             the window test
```

## License

The Rust engine in `crates/` is GPL-3.0-only, since its CPU is ported from
[InfiniEmu](https://github.com/pipe01/InfiniEmu), which is GPL-3.0. Everything else is
GPL-3.0-or-later. See [COPYING](COPYING).
