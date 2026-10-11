// The nRF52832's die temperature sensor: each measurement reports the temperature set from the monitor, in quarter degrees.

using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class NRF52832_TEMP : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_TEMP(IMachine machine, double temperature = 25.0) : base(machine)
        {
            IRQ = new GPIO();
            defaultTemperature = temperature;
            this.temperature = temperature;
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            temperature = defaultTemperature;
            measuredQuarterDegrees = 0;
            dataReady = false;
            interruptEnabled = false;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Monitor interface
        // ====================================================================

        // Sets the die temperature the next measurement reports.
        public void SetTemperature(double celsius)
        {
            temperature = celsius;
        }

        public double Temperature => temperature;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            Registers.TaskStart.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) Measure(); }, name: "TASKS_START")
                .WithReservedBits(1, 31);

            // A measurement completes at once, so there is nothing left to stop.
            Registers.TaskStop.Define(this)
                .WithFlag(0, FieldMode.Write, name: "TASKS_STOP")
                .WithReservedBits(1, 31);

            Registers.EventDataReady.Define(this)
                .WithFlag(0, valueProviderCallback: _ => dataReady,
                    writeCallback: (_, value) => { dataReady = value; UpdateInterrupts(); }, name: "EVENTS_DATARDY")
                .WithReservedBits(1, 31);

            Registers.InterruptEnableSet.Define(this)
                .WithFlag(0, valueProviderCallback: _ => interruptEnabled,
                    writeCallback: (_, value) => { if(value) interruptEnabled = true; UpdateInterrupts(); }, name: "DATARDY")
                .WithReservedBits(1, 31);

            Registers.InterruptEnableClear.Define(this)
                .WithFlag(0, valueProviderCallback: _ => interruptEnabled,
                    writeCallback: (_, value) => { if(value) interruptEnabled = false; UpdateInterrupts(); }, name: "DATARDY")
                .WithReservedBits(1, 31);

            Registers.Temperature.Define(this)
                .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => unchecked((uint)measuredQuarterDegrees), name: "TEMP");

            // The calibration constants of the linear temperature model: plain storage with the chip's reset values.
            DefineCalibration(Registers.SlopeA0, 0x320, 12, "A0");
            DefineCalibration(Registers.SlopeA1, 0x343, 12, "A1");
            DefineCalibration(Registers.SlopeA2, 0x35D, 12, "A2");
            DefineCalibration(Registers.SlopeA3, 0x400, 12, "A3");
            DefineCalibration(Registers.SlopeA4, 0x47F, 12, "A4");
            DefineCalibration(Registers.SlopeA5, 0x37B, 12, "A5");
            DefineCalibration(Registers.InterceptB0, 0x3FCC, 14, "B0");
            DefineCalibration(Registers.InterceptB1, 0x3F98, 14, "B1");
            DefineCalibration(Registers.InterceptB2, 0x3F98, 14, "B2");
            DefineCalibration(Registers.InterceptB3, 0x0012, 14, "B3");
            DefineCalibration(Registers.InterceptB4, 0x006A, 14, "B4");
            DefineCalibration(Registers.InterceptB5, 0x3DD0, 14, "B5");
            DefineCalibration(Registers.EndpointT0, 0xE2, 8, "T0");
            DefineCalibration(Registers.EndpointT1, 0x00, 8, "T1");
            DefineCalibration(Registers.EndpointT2, 0x14, 8, "T2");
            DefineCalibration(Registers.EndpointT3, 0x19, 8, "T3");
            DefineCalibration(Registers.EndpointT4, 0x50, 8, "T4");
        }

        private void DefineCalibration(Registers register, uint resetValue, int width, string name)
        {
            register.Define(this, resetValue)
                .WithValueField(0, width, name: name)
                .WithIgnoredBits(width, 32 - width);
        }

        // ====================================================================
        // Measuring
        // ====================================================================

        private void Measure()
        {
            measuredQuarterDegrees = (int)Math.Round(temperature * 4);
            dataReady = true;
            EventTriggered?.Invoke((uint)Registers.EventDataReady);
            UpdateInterrupts();
        }

        private void UpdateInterrupts()
        {
            IRQ.Set(dataReady && interruptEnabled);
        }

        private readonly double defaultTemperature;

        private double temperature;
        private int measuredQuarterDegrees;
        private bool dataReady;
        private bool interruptEnabled;

        private enum Registers
        {
            TaskStart = 0x000,
            TaskStop = 0x004,
            EventDataReady = 0x100,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Temperature = 0x508,
            SlopeA0 = 0x520,
            SlopeA1 = 0x524,
            SlopeA2 = 0x528,
            SlopeA3 = 0x52C,
            SlopeA4 = 0x530,
            SlopeA5 = 0x534,
            InterceptB0 = 0x540,
            InterceptB1 = 0x544,
            InterceptB2 = 0x548,
            InterceptB3 = 0x54C,
            InterceptB4 = 0x550,
            InterceptB5 = 0x554,
            EndpointT0 = 0x560,
            EndpointT1 = 0x564,
            EndpointT2 = 0x568,
            EndpointT3 = 0x56C,
            EndpointT4 = 0x570,
        }
    }
}
