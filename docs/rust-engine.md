# Rust engine

The emulator is a Rust program that runs PineTime firmware images unchanged. It replaces Renode.
Its CPU is a port of [InfiniEmu](https://github.com/pipe01/InfiniEmu)'s Cortex-M4 (GPL-3.0), so
the Rust crates are GPL-3.0-only. The chip and board models carry over the behaviour of the
C# models this repository had for Renode, with InfiniEmu as a second reference.

## Crates

```text
crates/
├── core/        pinetime-core: emulated time, the scheduler, the Bus trait, the board ports
├── cortex-m4/   pinetime-cortex-m4: the CPU, its NVIC, SCB, SysTick, DWT and FPU
├── nrf52832/    pinetime-nrf52832: memory and every nRF52832 peripheral
├── board/       pinetime-board: the screen, flash, touch, sensors, button, battery and motor
└── emulator/    pinetime-emulator: the machine, image loading, the monitor and the window link
```

Each crate depends only on the crates above it in this list, and builds and tests on its own
with `cargo test -p <name>`.

## Time

Time is counted in cycles of the 64 MHz CPU clock (`core::time`). Every alarm is an event in the
`Scheduler`, named by an `EventKey` of owner and token; scheduling a queued key moves it. The
machine runs the CPU until the next event is due, then hands each due event to its owner. A
sleeping CPU (WFI, or WFE with no event pending) skips straight to the next event, so an idle
watch costs almost nothing. Instructions cost the cycles InfiniEmu's CPU reports.

The machine paces emulated time against the wall clock when a window is attached, and runs flat
out for tests.

## CPU (`cortex-m4`)

- Decodes Thumb and Thumb-2 (ARMv7E-M with DSP and FPv4-SP) itself, without Capstone, and keeps
  decoded instructions in a cache keyed by address, checked against the raw halfwords on fetch.
- Handles the private peripheral bus (0xE000_0000..0xE010_0000) itself; everything else goes to
  the `Bus`.
- After each instruction, asks `Bus::interrupt_lines` and latches every high line as pending.
- `run(bus, cycle_budget)` executes until the budget is spent, a sleep, a lockup, a reset request
  or `Bus::stop_requested`.
- Exposes registers, VTOR and reset for the monitor, and reports SYSRESETREQ to the machine.

## Chip (`nrf52832`)

- Flash (512 KB), RAM (64 KB), FICR, UICR, and every peripheral the C# models had: CLOCK/POWER,
  GPIO, GPIOTE, PPI, RTC0..2, TIMER0..4, WDT, NVMC, SPIM/SPIS/TWIM/TWIS/SPI/TWI on the shared
  serial blocks, SAADC, TEMP, RNG, ECB, CCM/AAR, RADIO, QDEC, COMP/LPCOMP, PWM, MWU, EGU, and
  the errata identification registers.
- The CPU writes flash only through the NVMC with write enable set; EasyDMA reaches RAM only.
- Peripherals raise interrupt lines and PPI events; PPI tasks run at once, not at a later quantum.
- Talks to the board only through `BoardPorts`, handing it a `QueuedContext` and applying the pin
  changes the board queued when the call returns.

## Board (`board`)

Implements `BoardPorts` with the PineTime's wiring: ST7789 screen and XT25F32B flash on SPI
(chip selects P0.25 and P0.05), CST816S touch (0x15, INT P0.28, reset P0.10), BMA421 or BMA425
(0x18, INT1 P0.08) and HRS3300 (0x44) on I2C, the side button (P0.13, enabled by P0.15), the
battery on AIN7 with charger lines P0.12 and P0.19, the motor on P0.16 and the backlight on
P0.14, P0.22 and P0.23. Board pull-ups hold P0.05, P0.12, P0.19 and P0.25 high when undriven.

## Emulator (`emulator`)

- Builds the watch, loads ELF, Intel HEX and raw images, and runs it.
- Speaks the subset of Renode's monitor language the window, the tools and NeoTime's tests use,
  over a TCP port or standard input, so those callers keep working.
- Shares the screen and the status block with the window through memory-mapped files (see
  `frontend/pinetime_emulator/shared.py`).
- Reads the CPU's stack and reset address from the vector table when the watch first runs, so
  images loaded after `mach create` are the ones it starts, unless the monitor set PC or SP.

## Debugging firmware

- `cpu Step 20` runs twenty instructions and prints the PC, SP and xPSR after each.
- `sysbus LogAllPeripheralsAccess true` prints every peripheral access with its cycle on standard
  error; `sysbus LogSpiTraffic true` prints every SPI byte and the part it reached.
- `sysbus AddWatchpointLog ADDRESS DoubleWord Write @FILE` appends `address pc value` to FILE on
  each write; `pinetime-test` turns NeoTime's Python watchpoint hooks into this.
