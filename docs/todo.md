# To do

Open work on the emulator. Remove an item in the same change that finishes it.

- NeoTime's `boot_chain.robot` test "Holding The Button Resets Into The Bootloader" fails with "The
  watchdog did not reset the watch", on main as well as with the flash write guard (NeoTime build
  from the crash-record worktree, 2026-10-12). Find whether the watchdog or side button model, the
  test, or that build is at fault.
- wasp-os writes registers the models do not define: TIMER0 at Compare+X and CompareValue+X, and
  SAADC around ResultAmount. Check the offsets against the nRF52832 register map and add what is
  missing.
- wasp-os runs about 15 times slower than real time; NeoTime matters more, so measure NeoTime first.
- Including a model's .cs file again after a failed test stops Renode with "assembly already
  loaded"; a fresh Renode is needed.
