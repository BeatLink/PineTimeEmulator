# To do

Open work on the emulator. Remove an item in the same change that finishes it.

- MCUBoot's start-up loops take about 20 emulated seconds, close to a real watch; Renode ran them
  about twice as fast, so suites tuned on Renode may need longer waits.
- wasp-os writes registers the models do not define: TIMER0 at Compare+X and CompareValue+X, and
  SAADC around ResultAmount. Check the offsets against the nRF52832 register map.
- Model the Cortex-M4 memory protection unit (MPU_TYPE, MPU_CTRL, the region registers and
  MemManage faults on data and instruction accesses). BattleTime's stack overflow test in
  `crash_record.robot` skips until it exists; the CPU raises MemManage only for execution from
  0xE0000000 up today.
- A `pinetime-engine --bluetooth` that is killed instead of told to `quit` leaves its
  pseudo-terminal link behind; remove it when the engine is stopped by a signal too.
