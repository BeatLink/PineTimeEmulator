# Emulated hardware

What the emulator models, how it is wired, and what it leaves out. The chip's models are in
`crates/nrf52832`, the watch's parts in `crates/board`, and the CPU in `crates/cortex-m4`; each
crate carries its own tests.

## The CPU: Cortex-M4F

Thumb-2 with the DSP extension and single-precision FPU, decoded in Rust and cached by address.
NVIC with 48 interrupts and 3 priority bits, SCB, SysTick and the DWT cycle counter. Instructions
cost the cycles InfiniEmu charges, so a 64 MHz watch runs about 50 million instructions per
emulated second.

## The chip: nRF52832

| Address | Part | Notes |
|---|---|---|
| 0x00000000 | Flash, 512 KB | Starts erased (0xFF), kept across resets; the CPU writes it only with NVMC WEN set, and then only clears bits |
| 0x10000000 | FICR | Part 0x52832, 64 KB RAM, 512 KB flash, a fixed device address |
| 0x10001000 | UICR | Starts erased, kept across resets |
| 0x20000000 | RAM, 64 KB | Fixed noise after a power-on, kept across every other reset |
| 0x40000000 | POWER and CLOCK | RESETREAS, GPREGRET kept across resets, System OFF and GPIO wake |
| 0x40001000 | RADIO | Event timing, TIFS turnaround, address and device matching; frames reach the Bluetooth bridge when one is attached |
| 0x40002000 | UART and UARTE | No baud timing |
| 0x40003000, 0x40004000, 0x40023000 | SPI, SPIM, TWI, TWIM 0..2 | Chosen by ENABLE, with EasyDMA; one event per transfer at the configured bit rate |
| 0x40006000 | GPIOTE | Event and task channels, PORT event, PPI |
| 0x40007000 | SAADC | Real gain, reference and resolution |
| 0x40008000.. | TIMER0..4 | Exact compare and capture, SHORTS, PPI |
| 0x4000B000.. | RTC0..2 | TICK, OVRFLW, COMPARE, TRIGOVRFLW, computed from emulated time |
| 0x4000C000 | TEMP | |
| 0x4000D000 | RNG | |
| 0x4000E000 | ECB | Real AES-128 |
| 0x4000F000 | CCM and AAR | Real Bluetooth LE AES-CCM and address resolution |
| 0x40010000 | WDT | Resets the chip and records RESETREAS.DOG |
| 0x40012000 | QDEC | Registers only |
| 0x40013000 | COMP and LPCOMP | Registers only |
| 0x40014000.. | EGU0..5 | |
| 0x4001C000, 0x40021000, 0x40022000 | PWM0..2 | Sequences in emulated time |
| 0x4001E000 | NVMC | Page, UICR and full erase |
| 0x4001F000 | PPI | Tasks run at once, including the fixed channels 20..31 |
| 0x40020000 | MWU | Registers only |
| 0x50000000 | GPIO | Real pin levels: drive, pulls, open drain, SENSE and LATCH |

EasyDMA writes only to data RAM; a pointer anywhere else drops the write, as the chip cannot
reach flash.

## The board: PineTime

Monitor names are in brackets; the engine matches the last word, so `lcd` works as well.

| Part | Connection |
|---|---|
| Screen, ST7789 (`sysbus.serial0.spiBus.lcd`) | SPI0, CS P0.25, D/C P0.18, reset P0.26, backlight P0.14/22/23 |
| External flash, XT25F32B 4 MB (`sysbus.serial0.spiBus.externalFlash`) | SPI0, CS P0.05; at 0x60000000 for the monitor |
| Touch panel, CST816S (`sysbus.serial1.touch`) | I2C1 0x15, INT P0.28, reset P0.10 |
| Accelerometer, BMA425 (`sysbus.serial1.accelerometer`) | I2C1 0x18, INT P0.08 |
| Heart rate sensor, HRS3300 (`sysbus.serial1.heartRateSensor`) | I2C1 0x44 |
| Side button (`sysbus.gpio0.sideButton`) | P0.13, powered from P0.15 |
| Vibration motor (`sysbus.gpio0.vibrationMotor`) | P0.16, on while low |
| Battery and charger (`sysbus.gpio0.battery`) | AIN7 through /2, power present P0.19, charging P0.12 |

Board pull-ups hold P0.05, P0.12, P0.19 and P0.25 high while nothing drives them.

## Useful monitor commands

```text
mach create
sysbus LoadELF @image.elf
sysbus LoadBinary @image.bin 0x5000
emulation RunFor "3"
lcd SaveScreenshot @screen.png
touch Tap 120 120
touch Swipe "up"
sideButton Hold 1
battery Percent 50
battery ConnectCharger
heartRateSensor SetHeartRate 72
accelerometer AddSteps 100
externalFlash SaveBinary @flash.bin
powerClock PowerCycle
cpu PC
cpu Step 20
bluetooth Attach @/path/to/hci
bluetooth Status
```

## The Bluetooth bridge

A second machine, an nRF52840 running Zephyr's Bluetooth LE controller, shares the watch's air
and offers its HCI on a pseudo-terminal that `btattach` hands to BlueZ. The nRF52840 is a variant
of the same chip models, with 1 MB of flash, 256 KB of RAM, GPIO port P1, UARTE1 and the newer
radio events. See [bluetooth.md](bluetooth.md).

## Not emulated yet

- Radio whitening, collisions, Coded PHY and transmit power: frames either arrive whole or not at all.
- SPIS and TWIS, UART baud timing, PWM driving pins, MWU memory watching, QDEC counting and
  COMP/LPCOMP analog levels.
- An MPU, and late-arriving interrupts.
- NVMC write timing.
