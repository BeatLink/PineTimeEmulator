// SPDX-License-Identifier: GPL-3.0-or-later
// Test-only devices for the GPIO tests: a probe that records pin levels, a driver that pushes a level in, and a DETECT counter.

using Antmicro.Renode.Core;
using Antmicro.Renode.Peripherals.GPIOPort;

namespace Antmicro.Renode.Peripherals.Mocks
{
    // Remembers the last level seen on each of its inputs.
    public class GpioTestProbe : IGPIOReceiver
    {
        public void OnGPIO(int number, bool value)
        {
            if(number >= 0 && number < levels.Length)
            {
                levels[number] = value;
            }
        }

        // Keeps its levels, because a connection only reports changes.
        public void Reset()
        {
        }

        public bool Level(int input)
        {
            return levels[input];
        }

        private readonly bool[] levels = new bool[32];
    }

    // Drives one line into the port, as an outside chip would.
    public class GpioTestDriver : IGPIOSender
    {
        public GpioTestDriver()
        {
            Line = new GPIO();
        }

        public GPIO Line { get; }

        public void Reset()
        {
        }

        public void Drive(bool level)
        {
            Line.Set(level);
        }
    }

    // Counts rising edges of the port's DETECT signal.
    public class GpioTestDetectCounter : IGPIOReceiver
    {
        public GpioTestDetectCounter(NRF52832_GPIO port)
        {
            port.Detect += () => Count++;
        }

        // Takes no inputs; it is a receiver only so it can sit under the port.
        public void OnGPIO(int number, bool value)
        {
        }

        public void Reset()
        {
        }

        public int Count { get; private set; }
    }
}
