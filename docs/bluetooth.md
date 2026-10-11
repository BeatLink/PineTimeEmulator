# Bluetooth to the computer

The emulated watch's radio can reach the computer's own Bluetooth stack, so `bluetoothctl`,
Gadgetbridge-style tools and companion apps see it like a real watch.

## How it works

```text
emulated PineTime ──radio──▶ Renode BLE medium ◀──radio── emulated nRF52840 (Zephyr hci_uart)
                                                              │ UART1, H4 HCI
                                                              ▼
                                                     pseudo-terminal on the host
                                                              │ btattach
                                                              ▼
                                                     BlueZ adapter hciN
```

The second machine runs Zephyr's `hci_uart` sample, a complete Bluetooth LE controller, which
Antmicro builds for Renode's own Bluetooth examples (`nix/hci-controller.nix` pins it). Both radios
share one simulated medium, and the controller's HCI goes out through a pseudo-terminal. `btattach`
turns that terminal into a normal BlueZ adapter.

Both machines use this repository's radio model (`NRF52832_RADIO`), which times every radio event
to the bit and turns around in exactly 150 µs, as the controllers inside InfiniTime and Zephyr
expect. The bridge's platform, `platform/bluetooth-bridge.repl`, also uses this repository's
CCM/AAR, TIMER and RTC models: the Linux kernel connects through an accept list with address
resolution, which Zephyr's controller does in that hardware.

With InfiniTime 1.16.1 on the watch, `gatttool -i hciN -t random -b C0:DE:0A:1B:2C:3D --char-read
--uuid=0x2a00` reads "InfiniTime" through BlueZ.

## Using it

```sh
nix run . -- --infinitime --bluetooth
```

The command prints the terminal's path and the line that attaches it:

```sh
sudo btattach -B /run/user/1000/pinetime-emulator-XXXX/hci -P h4 -S 1000000
```

`btattach` needs root, and the adapter exists while it runs. BlueZ may make the new adapter its
default, so select your usual adapter again in `bluetoothctl` if you rely on the default. Stop
`btattach` to remove the adapter.

Without the window, include `scripts/bluetooth-bridge.resc` after `scripts/pinetime.resc`; set
`$hci_pty` first, with no machine selected, to choose the terminal path.

## Testing without BlueZ

`tools/hci_scan.py` speaks HCI to the terminal directly, so the bridge can be checked without
root and without touching the computer's adapters:

```sh
python3 tools/hci_scan.py /path/to/hci --seconds 30
python3 tools/hci_scan.py /path/to/hci --connect C0:DE:0A:1B:2C:3D
```

The first lists the advertisers it hears; the second connects and reads the GATT device name.

## Notes

- The controller ignores its UART for about a second after start-up; `hci_scan.py` repeats HCI
  Reset until it answers, and BlueZ retries by itself.
- The emulated watch's address comes from its FICR: `C0:DE:0A:1B:2C:3D`.
- Emulated time runs close to real time, which keeps BlueZ's command timeouts happy. A heavily
  loaded computer slows the emulation, and the link may drop.
