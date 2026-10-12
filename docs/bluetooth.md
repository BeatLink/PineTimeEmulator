# Bluetooth to the computer

The emulated watch's radio can reach the computer's own Bluetooth stack, so `bluetoothctl`,
`gatttool` and companion apps see it like a real watch.

## How it works

```text
emulated PineTime (nRF52832) ──RADIO frames──▶ ◀──RADIO frames── emulated nRF52840 (Zephyr hci_uart)
                                                                    │ UARTE1, H4 HCI
                                                                    ▼
                                                           pseudo-terminal on the host
                                                                    │ btattach
                                                                    ▼
                                                           BlueZ adapter hciN
```

The bridge is a second machine in the same engine: an nRF52840 (`Variant::Nrf52840` in
`crates/nrf52832`) running Zephyr's `hci_uart` sample, a complete Bluetooth LE controller, which
Antmicro builds for Renode's Bluetooth examples. `nix/hci-controller.nix` pins that build, and
`$PINETIME_EMULATOR_HCI_CONTROLLER` names it in the package and the development shell. The
controller's UARTE1 goes out through a pseudo-terminal (`crates/emulator/src/pseudo_terminal.rs`)
behind a symlink, and `btattach` turns that terminal into a normal BlueZ adapter.

`crates/emulator/src/bluetooth.rs` runs the watch and the bridge in lockstep. Each RADIO hands
the frames it sends to an `Outbox`, and the two outboxes are swapped at the end of every step.
While either radio is on, a step is 16 µs, shorter than the time from START to the end of the
access address, so every frame reaches the other radio before it needs it; with both radios
off, steps stretch to the next moment either machine could act plus a radio ramp-up, at most
1 ms. The RADIO model times every event to the bit, turns around in exactly 150 µs, and matches
access and device addresses, as the controllers inside InfiniTime and Zephyr expect. The bridge
also uses the CCM/AAR, TIMER and RTC models, because the Linux kernel connects through an accept
list with address resolution, which Zephyr's controller does in that hardware.

With InfiniTime 1.16.1 on the watch, `gatttool` through BlueZ reads its name ("InfiniTime"),
battery level and firmware version ("1.16.1").

## Using it

```sh
nix run . -- --infinitime --bluetooth
```

The command prints the terminal's path and the line that attaches it:

```sh
sudo btattach -B /run/user/1000/pinetime-emulator-XXXX/hci -P h4 -S 1000000
```

`btattach` needs root, and the adapter exists while it runs. BlueZ may make the new adapter its
default, so pass `-i hciN` to tools, or select your usual adapter again in `bluetoothctl`. Stop
`btattach` to remove the adapter; the engine then puts a fresh terminal behind the same link, so
`btattach` can attach again.

Without the window, start the engine with the bridge, or attach it from the monitor:

```sh
pinetime-engine --bluetooth /path/to/hci [--hci-controller controller.elf] ...
```

```text
bluetooth Attach @/path/to/hci [@controller.elf]
bluetooth Status
bluetooth Terminal
bluetooth Detach
```

Both take the controller from `$PINETIME_EMULATOR_HCI_CONTROLLER` when none is given. The bridge
stays across `mach create`, which gives the new watch a fresh bridge at the same link.

## Testing without BlueZ

`tools/hci_scan.py` speaks HCI to the terminal directly, so the bridge can be checked without
root and without touching the computer's adapters:

```sh
python3 tools/hci_scan.py /path/to/hci --seconds 30
python3 tools/hci_scan.py /path/to/hci --connect C0:DE:0A:1B:2C:3D
```

The first lists the advertisers it hears; the second connects and reads the GATT device name.
`crates/emulator/tests/bluetooth_bridge.rs` checks the air between the two radios and the monitor
commands without any firmware.

## Notes

- The controller ignores its UART for about a second after start-up; `hci_scan.py` repeats HCI
  Reset until it answers, and BlueZ retries by itself.
- The emulated watch's address comes from its FICR: `C0:DE:0A:1B:2C:3D`.
- Emulated time runs close to real time, which keeps BlueZ's command timeouts happy. A heavily
  loaded computer slows the emulation, and the link may drop.
- Output the controller sends while no program holds the terminal open is dropped, as on a UART
  with nothing listening.
