# Installing without Nix

The flake builds and wraps everything for you. Without Nix you install the same pieces yourself:
the Rust engine, the Python window, a firmware image to run, and the Bluetooth controller if you
want the watch on your computer's Bluetooth. This page lists every dependency and where each one
comes from.

It is written for Linux on x86_64 or aarch64, the systems the flake supports. Other systems are
untested.

## The AppImage

To run the watch window without building anything, take `PineTime_Emulator-x86_64.AppImage` or
`PineTime_Emulator-aarch64.AppImage` from the
[releases](https://github.com/BeatLink/PineTimeEmulator/releases). It carries Python, SDL2, the
engine, InfiniTime 1.16.1, the Bluetooth controller and a font, and needs glibc 2.28 or newer
(Debian 10, Ubuntu 20.04, Fedora 29 and later) and FUSE, as every AppImage does.

```sh
chmod +x PineTime_Emulator-x86_64.AppImage
./PineTime_Emulator-x86_64.AppImage                     # InfiniTime, the same as --infinitime
./PineTime_Emulator-x86_64.AppImage --image FILE        # any other firmware; the options are the ones below
./PineTime_Emulator-x86_64.AppImage --infinitime --bluetooth
./PineTime_Emulator-x86_64.AppImage engine              # pinetime-engine, the monitor on standard input
```

Without FUSE, add `--appimage-extract-and-run`. `--bluetooth` still needs BlueZ's `btattach` on
the computer, as [below](#bluetooth). `nix build .#appimage` builds the same file; the AppImage
workflow builds both and attaches them to a release when a `v` tag is pushed.

For tests, the Robot suites and your own changes, build from source as the rest of this page
describes.

## What you need

| For | Needs |
| --- | --- |
| The engine, `pinetime-engine` | a current stable Rust with cargo, from [rustup](https://rustup.rs) |
| The window, `pinetime-emulator` | Python 3.10 or newer with `venv` and `pip`; SDL2 and SDL2_ttf shared libraries; fontconfig (`fc-match`) and the DejaVu Sans Mono font |
| `cargo test` | A C compiler (the `capstone` disassembler the CPU tests compare against builds from C); Arm's `arm-none-eabi-as`, `-ld` and `-objcopy`, which assemble the CPU tests' programs |
| `pytest tests/` | pytest, and an InfiniTime ELF (see [firmware to run](#firmware-to-run)) |
| Robot suites | robotframework |
| `--bluetooth` | `curl` to fetch the controller; BlueZ's `btattach` and a kernel with `hci_uart` (H4) to hand it to BlueZ |
| Building InfiniTime yourself | see [building InfiniTime](#building-infinitime-from-source) |

The engine itself uses only the Rust standard library and `libc`; it needs no system libraries.

### Distribution packages

Debian and Ubuntu:

```sh
sudo apt install build-essential curl git python3 python3-venv python3-pip \
    libsdl2-2.0-0 libsdl2-ttf-2.0-0 fontconfig fonts-dejavu-core \
    binutils-arm-none-eabi bluez
```

Fedora:

```sh
sudo dnf install gcc curl git python3 python3-pip \
    SDL2 SDL2_ttf fontconfig dejavu-sans-mono-fonts \
    arm-none-eabi-binutils-cs bluez
```

Arch Linux:

```sh
sudo pacman -S --needed base-devel curl git python python-pip \
    sdl2 sdl2_ttf fontconfig ttf-dejavu \
    arm-none-eabi-binutils bluez bluez-utils
```

Then Rust; distribution packages of it are often too old, so use rustup:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

`binutils-arm-none-eabi` and `bluez` are needed only for `cargo test` and `--bluetooth`.

## Building

```sh
git clone <this repository> PineTimeEmulator
cd PineTimeEmulator

# The engine: target/release/pinetime-engine
cargo build --release -p pinetime-emulator

# The window, in its own Python environment, with PySDL2 from PyPI
python3 -m venv .venv
.venv/bin/pip install ./frontend
.venv/bin/pip install pytest robotframework   # only for the tests and the Robot suites
```

The flake's wrapper sets a few environment variables that point the window at its parts. Set
them yourself, for example in `~/.profile` or a small script:

```sh
export PATH="$PWD/.venv/bin:$PWD/target/release:$PATH"
export PINETIME_EMULATOR_ENGINE="$PWD/target/release/pinetime-engine"
```

| Variable | What it names | Without it |
| --- | --- | --- |
| `PINETIME_EMULATOR_ENGINE` | the `pinetime-engine` binary | the one on `PATH` |
| `PINETIME_EMULATOR_INFINITIME` | the InfiniTime ELF that `--infinitime` loads | `--infinitime` refuses to start; use `--image` |
| `PINETIME_EMULATOR_HCI_CONTROLLER` | the Bluetooth controller firmware | `--bluetooth` refuses to start |
| `PINETIME_EMULATOR_FONT` | a TrueType font for the window's labels | `fc-match "DejaVu Sans Mono"`; with no font the labels are left out |
| `PYSDL2_DLL_PATH` | one folder holding `libSDL2` and `libSDL2_ttf` | the system's library path, which is right on most distributions |

If SDL2 is not packaged for you, `.venv/bin/pip install pysdl2-dll` brings PyPI's own copies of
SDL2 and SDL2_ttf instead.

## Firmware to run

The window runs any image you give it with `--image FILE[@ADDRESS]`: `.elf`, `.out` and `.hex`
carry their own addresses, a `.bin` needs `@ADDRESS`.

### InfiniTime from its release

InfiniTime's release has one hex with its MCUboot bootloader and the app together, which runs
as it is:

```sh
mkdir -p firmware && cd firmware
curl -LO https://github.com/InfiniTimeOrg/InfiniTime/releases/download/1.16.1/bootloader_infinitime-1.16.1.hex
cd ..
pinetime-emulator --image firmware/bootloader_infinitime-1.16.1.hex
```

MCUboot shows its logo for about 20 emulated seconds before InfiniTime starts, as on a real watch.

### BattleTime

Build BattleTime's images and load the bootloader at 0x0 and the firmware at 0x8000; BattleTime's
`docs/testing.md` has the steps, including the external flash image that installs its apps.

### Building InfiniTime from source

`--infinitime`, `pytest tests/` and suites that start from a bare app want InfiniTime linked at
0x0 with no bootloader, which the release does not include. Build it as `nix/infinitime.nix` does.
You need:

- CMake, GNU make, `patch`, `git`;
- Arm GNU Toolchain 13 (`arm-none-eabi-gcc` 13.x), from
  [developer.arm.com](https://developer.arm.com/downloads/-/arm-gnu-toolchain-downloads)
  or your distribution (`gcc-arm-none-eabi` on Debian and Ubuntu, `arm-none-eabi-gcc` and
  `arm-none-eabi-newlib` on Arch and Fedora);
- Nordic's nRF5 SDK 15.3.0, from
  <https://nsscprodmedia.blob.core.windows.net/prod/software-and-other-downloads/sdks/nrf5/binaries/nrf5sdk153059ac345.zip>
  (the old developer.nordicsemi.com links return 403);
- Node.js with npm, for `lv_font_conv`;
- Python 3 with `cbor`, `click`, `cryptography`, `intelhex` and `pillow`.

```sh
python3 -m venv infinitime-venv
infinitime-venv/bin/pip install cbor click cryptography intelhex pillow
npm install --global lv_font_conv@1.5.2

curl -L -o nrf5-sdk.zip https://nsscprodmedia.blob.core.windows.net/prod/software-and-other-downloads/sdks/nrf5/binaries/nrf5sdk153059ac345.zip
unzip -q nrf5-sdk.zip -d nrf5-sdk     # gives nrf5-sdk/nRF5_SDK_15.3.0_59ac345

git clone --branch 1.16.1 --recurse-submodules https://github.com/InfiniTimeOrg/InfiniTime
cd InfiniTime
PATH="$PWD/../infinitime-venv/bin:$PATH" cmake -B build \
    -DCMAKE_BUILD_TYPE=Release \
    -DARM_NONE_EABI_TOOLCHAIN_PATH=/path/to/arm-gnu-toolchain-13 \
    -DNRF5_SDK_PATH="$PWD/../nrf5-sdk/nRF5_SDK_15.3.0_59ac345" \
    -DBUILD_RESOURCES=1
PATH="$PWD/../infinitime-venv/bin:$PATH" cmake --build build -j"$(nproc)" --target pinetime-app pinetime-mcuboot-app

export PINETIME_EMULATOR_INFINITIME="$PWD/build/src/pinetime-app-1.16.1.out"
```

`ARM_NONE_EABI_TOOLCHAIN_PATH` is the folder holding the toolchain's `bin/`, such as `/usr` for a
distribution package. The build also leaves `build/src/pinetime-mcuboot-app-image-1.16.1.bin`,
which loads at 0x8000 behind the bootloader. Suites that start from InfiniTime's bootloader
5.0.4 take `bootloader/bootloader-5.0.4.bin` from the same checkout.

## Bluetooth

`--bluetooth` runs a second emulated chip as a Bluetooth controller (see
[bluetooth.md](bluetooth.md)). Fetch its firmware once and check it:

```sh
curl -L -o firmware/nrf52840-hci-uart.elf \
    https://dl.antmicro.com/projects/renode/nrf52840dk--zephyr-ble_hci_uart.elf-s_2437336-4bf9cccb70ca95fb12667896eec3f777f39f90ee
echo "642546f3a5347de7649a6068f82a6505bd663bf5b14c7cdc36cf99bb79426f26  firmware/nrf52840-hci-uart.elf" | sha256sum -c
export PINETIME_EMULATOR_HCI_CONTROLLER="$PWD/firmware/nrf52840-hci-uart.elf"
```

`btattach` needs root and the kernel's `hci_uart` module; load it with `sudo modprobe hci_uart`
if your distribution does not do so by itself.

## Running

```sh
pinetime-emulator --image firmware/bootloader_infinitime-1.16.1.hex   # the watch window
pinetime-emulator --infinitime --bluetooth                            # with your own InfiniTime build and the controller
pinetime-engine                                                       # the monitor on standard input
cargo test                                                            # every model, CPU and engine test
pytest tests/                                                         # the window test
```

`pinetime-test` is a small wrapper the flake makes around Robot Framework. Run Robot directly
instead, from the repository root:

```sh
robot --pythonpath robot --variable RENODEKEYWORDS:"$PWD/robot/pinetime.resource" \
    --outputdir results SUITE.robot
```

`--outputdir` stands for `pinetime-test -r` and `--test NAME` for `-f`.
