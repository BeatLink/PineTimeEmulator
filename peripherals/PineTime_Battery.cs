// The PineTime's battery and charger: the cell voltage reaches AIN7 through a /2 divider, P0.19 goes low while powered and P0.12 low while charging.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Analog;
using Antmicro.Renode.Peripherals.GPIOPort;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class PineTime_Battery : IGPIOSender
    {
        public PineTime_Battery(IMachine machine, NRF52832_SAADC saadc, NRF52832_GPIO port, double volts = 3.9)
        {
            this.machine = machine;
            this.saadc = saadc;
            this.port = port;
            this.volts = volts;
            Apply();
            machine.MachineReset += _ => Apply();
        }

        public void Reset()
        {
        }

        // ====================================================================
        // Battery
        // ====================================================================

        public double Volts
        {
            get => volts;
            set
            {
                volts = Math.Max(0, Math.Min(FullVolts, value));
                Apply();
            }
        }

        /// The charge as a share of the usable range, from 3.0 V (empty) to 4.2 V (full).
        public int Percent
        {
            get => (int)Math.Round(Math.Max(0, Math.Min(1, (volts - EmptyVolts) / (FullVolts - EmptyVolts))) * 100);
            set => Volts = EmptyVolts + (FullVolts - EmptyVolts) * Math.Max(0, Math.Min(100, value)) / 100.0;
        }

        // ====================================================================
        // Charger
        // ====================================================================

        public bool ChargerConnected
        {
            get => chargerConnected;
            set
            {
                chargerConnected = value;
                Apply();
                this.Log(LogLevel.Info, chargerConnected ? "Charger connected" : "Charger removed");
            }
        }

        /// While on the charger the cell is reported as charging until it is full.
        public bool Charging => chargerConnected && volts < FullVolts;

        public void ConnectCharger()
        {
            ChargerConnected = true;
        }

        public void DisconnectCharger()
        {
            ChargerConnected = false;
        }

        // Both status lines are open drain from the charger chip, so they are released rather than driven high.
        private void Apply()
        {
            saadc.SetInputVoltage(BatteryAnalogInput, volts * DividerRatio);
            DriveLowOrRelease(PowerPresentPin, chargerConnected);
            DriveLowOrRelease(ChargingPin, Charging);
        }

        private void DriveLowOrRelease(int pin, bool low)
        {
            if(low)
            {
                port.DriveExternally(pin, false);
            }
            else
            {
                port.ReleaseExternalDrive(pin);
            }
        }

        private const int BatteryAnalogInput = 7;
        private const double DividerRatio = 0.5;
        private const int PowerPresentPin = 19;
        private const int ChargingPin = 12;
        private const double EmptyVolts = 3.0;
        private const double FullVolts = 4.2;

        private readonly IMachine machine;
        private readonly NRF52832_SAADC saadc;
        private readonly NRF52832_GPIO port;
        private double volts;
        private bool chargerConnected;
    }
}
