// SPDX-License-Identifier: GPL-3.0-or-later
// The PineTime's side button: while pressed it joins P0.13 to the P0.15 power line; the space bar presses it.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.GPIOPort;
using Antmicro.Renode.Peripherals.Input;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class PineTime_SideButton : IGPIOReceiver, IKeyboard
    {
        public PineTime_SideButton(IMachine machine, NRF52832_GPIO port, int sensePin = 13)
        {
            this.machine = machine;
            this.port = port;
            this.sensePin = sensePin;
        }

        public bool Pressed { get; private set; }

        public bool Powered { get; private set; }

        public void Reset()
        {
            Powered = false;
            Update();
        }

        /// GPIO input 0 is the P0.15 power line.
        public void OnGPIO(int number, bool value)
        {
            if(number == PowerInput)
            {
                Powered = value;
                Update();
            }
        }

        // ====================================================================
        // Pressing
        // ====================================================================

        public void Press()
        {
            Pressed = true;
            Update();
        }

        public void Release()
        {
            Pressed = false;
            Update();
        }

        /// Presses now and lets go after the given time of emulated time.
        public void Hold(double seconds)
        {
            Press();
            machine.ScheduleAction(TimeInterval.FromSeconds(seconds), _ => Release());
        }

        public void PressAndRelease()
        {
            Hold(DefaultClickSeconds);
        }

        // ====================================================================
        // Keyboard
        // ====================================================================

        public void Press(KeyScanCode scanCode)
        {
            if(scanCode == ButtonKey)
            {
                Press();
            }
        }

        public void Release(KeyScanCode scanCode)
        {
            if(scanCode == ButtonKey)
            {
                Release();
            }
        }

        // The switch connects the sense pin to the power line, so an open switch leaves the pin to its pulls.
        private void Update()
        {
            if(Pressed)
            {
                port.DriveExternally(sensePin, Powered);
            }
            else
            {
                port.ReleaseExternalDrive(sensePin);
            }
        }

        private const int PowerInput = 0;
        private const KeyScanCode ButtonKey = KeyScanCode.Space;
        private const double DefaultClickSeconds = 0.1;

        private readonly IMachine machine;
        private readonly NRF52832_GPIO port;
        private readonly int sensePin;
    }
}
