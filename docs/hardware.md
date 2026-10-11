# Emulated hardware

What the emulator models, how it is wired, and what it leaves out. Each model is a C# file in
`peripherals/` that Renode compiles when `scripts/pinetime.resc` starts, with its tests in
`tests/`.

## The chip: nRF52832

`platform/nrf52832.repl`.

| Address | Part | Model | Notes |
|---|---|---|---|
| 0x00000000 | Flash, 512 KB | Renode memory | Starts erased (0xFF), kept across resets |
| 0x10000000 | FICR | Renode memory | Part 0x52832, 64 KB RAM, 512 KB flash, a fixed device address |
| 0x10001000 | UICR | Renode memory | Starts erased, kept across resets |
| 0x20000000 | RAM, 64 KB | Renode memory | Random contents after `powerClock PowerCycle` |
| 0x40000000 | POWER and CLOCK | `NRF52832_PowerClock` | RESETREAS, GPREGRET kept across resets, System OFF and GPIO wake |
| 0x40001000 | RADIO | Renode `NRF52840_Radio` | Bluetooth LE between emulated machines |
| 0x40002000 | UART | Renode `NRF52840_UART` | |
| 0x40003000, 0x40004000 | SPI, SPIM, TWI, TWIM 0 and 1 | `NRF52832_SerialInterface` | All four modes, chosen by ENABLE, with EasyDMA |
| 0x40006000 | GPIOTE | `NRF52832_GPIOTE` | Event and task channels, PORT event, PPI |
| 0x40007000 | SAADC | `NRF52832_SAADC` | Real gain, reference and resolution |
| 0x40008000.. | TIMER0..4 | `NRF52832_TIMER` | Exact compare and capture, SHORTS, PPI |
| 0x4000B000.. | RTC0..2 | `NRF52832_RTC` | TICK, OVRFLW, COMPARE, TRIGOVRFLW |
| 0x4000C000 | TEMP | `NRF52832_TEMP` | |
| 0x4000D000 | RNG | Renode `NRF52840_RNG` | |
| 0x4000E000 | ECB | Renode `NRF52840_ECB` | |
| 0x4000F000 | CCM and AAR | `NRF52832_CCM_AAR` | Real Bluetooth LE AES-CCM and address resolution |
| 0x40010000 | WDT | `NRF52832_Watchdog` | Resets the chip and records RESETREAS.DOG |
| 0x40012000 | QDEC | `NRF52832_QDEC` | Registers only |
| 0x40013000 | COMP and LPCOMP | `NRF52832_COMP_LPCOMP` | Registers only |
| 0x40014000.. | EGU0..5 | Renode `NRF52840_EGU` | |
| 0x4001C000, 0x40021000, 0x40022000 | PWM0..2 | `NRF52832_PWM` | Sequences in emulated time |
| 0x4001E000 | NVMC | `NRF52832_NVMC` | Page, UICR and full erase |
| 0x4001F000 | PPI | Renode `NRF52840_PPI` | |
| 0x40020000 | MWU | `NRF52832_MWU` | Registers only |
| 0x40023000 | SPIM2 | Renode `NRF52840_SPI` | |
| 0x50000000 | GPIO | `NRF52832_GPIO` | Real pin levels: drive, pulls, open drain, SENSE and LATCH |
| 0xE0001000 | DWT | Renode `DWT` | Cycle counter at 64 MHz |

## The board: PineTime

`platform/pinetime.repl`. Monitor names are in brackets.

| Part | Connection | Model |
|---|---|---|
| Screen, ST7789 (`sysbus.serial0.spiBus.lcd`) | SPI0, CS P0.25, D/C P0.18, reset P0.26, backlight P0.14/22/23 | `PineTime_ST7789` |
| External flash, XT25F32B 4 MB (`sysbus.serial0.spiBus.externalFlash`) | SPI0, CS P0.05 | `PineTime_XT25F32B` |
| Touch panel, CST816S (`sysbus.serial1.touch`) | I2C1 0x15, INT P0.28, reset P0.10 | `PineTime_CST816S` |
| Accelerometer, BMA425 (`sysbus.serial1.accelerometer`) | I2C1 0x18, INT P0.08 | `PineTime_BMA42x` |
| Heart rate sensor, HRS3300 (`sysbus.serial1.heartRateSensor`) | I2C1 0x44 | `PineTime_HRS3300` |
| Side button (`sysbus.gpio0.sideButton`) | P0.13, powered from P0.15 | `PineTime_SideButton` |
| Vibration motor (`sysbus.gpio0.vibrationMotor`) | P0.16, on while low | Renode `LED` |
| Battery and charger (`sysbus.gpio0.battery`) | AIN7 through /2, power present P0.19, charging P0.12 | `PineTime_Battery` |

Board pull-ups hold P0.05, P0.12, P0.19 and P0.25 high while nothing drives them.

## Useful monitor commands

```text
sysbus.serial0.spiBus.lcd SaveScreenshot @screen.png
sysbus.serial1.touch Tap 120 120
sysbus.serial1.touch Swipe "up"
sysbus.gpio0.sideButton Hold 1
sysbus.gpio0.battery Percent 50
sysbus.gpio0.battery ConnectCharger
sysbus.serial1.heartRateSensor SetHeartRate 72
sysbus.serial1.accelerometer AddSteps 100
sysbus.serial0.spiBus.externalFlash SaveImage @flash.bin
powerClock PowerCycle
```

## Renode fixes

Renode 1.17 decodes the Thumb-2 instruction `MOV.W Rd, SP` as illegal on Cortex-M4, which stops
Nordic's SoftDevice. `nix/renode.nix` builds Renode with `nix/renode-thumb2-mov-from-sp.patch`,
which fixes it.

## Not emulated yet

- Bluetooth to the computer: the radio talks only to other emulated machines.
- Timing of the CPU itself: instructions take no time beyond the clock, so speed-sensitive code
  runs faster or slower than on a watch.
- PPI tasks run at the next time quantum, 20 µs at most, rather than at once.
- NVMC write timing, and the rule that programming flash only clears bits.
- MWU memory watching, QDEC counting and COMP/LPCOMP analog levels.
- The external flash sits at 0x60000000 on the system bus so tests can read it; real firmware never
  touches that address.
