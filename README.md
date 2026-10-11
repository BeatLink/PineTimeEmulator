# PineTimeEmulator

A full hardware emulator for the [PineTime](https://pine64.org/devices/pinetime/) smartwatch, built
on [Renode](https://renode.io). It emulates the nRF52832 chip and the parts of the watch around it,
so the images you would flash on a real watch run unchanged: InfiniTime, wasp-os, NeoTime, the
bootloaders and Nordic's SoftDevice.

## Status

Work in progress. The MBR, wasp-os bootloader and SoftDevice already start; the rest of the
watch is being added. See [docs/hardware.md](docs/hardware.md) for what is emulated.

## Running it

```sh
nix develop
renode-test tests/            # run every model test
```

## Layout

```text
PineTimeEmulator/
├── flake.nix          development shell
├── platform/          the chip and board descriptions Renode loads
├── peripherals/       C# models Renode compiles at start-up
├── scripts/           Renode scripts that build and start the watch
└── tests/             Robot Framework tests, one file per model
```

## License

To be decided. `platform/nrf52.svd` is Nordic's register description under its own BSD-3-Clause
licence.
