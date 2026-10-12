# To do

Open work on the emulator. Remove an item in the same change that finishes it.

- MCUBoot's start-up loops take about 20 emulated seconds, close to a real watch; Renode ran them
  about twice as fast, so suites tuned on Renode may need longer waits.
- wasp-os writes registers the models do not define: TIMER0 at Compare+X and CompareValue+X, and
  SAADC around ResultAmount. Check the offsets against the nRF52832 register map.
