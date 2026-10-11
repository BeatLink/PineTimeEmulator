// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's POWER and CLOCK blocks, which share 0x40000000 and one interrupt.

using System;
using System.Collections.Generic;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.GPIOPort;
using Antmicro.Renode.Peripherals.Memory;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class NRF52832_PowerClock : BasicDoubleWordPeripheral, IKnownSize, IGPIOReceiver, INRFEventProvider
    {
        public NRF52832_PowerClock(IMachine machine, NRF52832_GPIO gpio = null, MappedMemory ram = null) : base(machine)
        {
            this.ram = ram;
            IRQ = new GPIO();
            DefineRegisters();
            if(gpio != null)
            {
                gpio.Detect += WakeFromSystemOff;
            }
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        public bool InSystemOff { get; private set; }

        // GPREGRET and RESETREAS survive every reset except a power cycle.
        public override void Reset()
        {
            var kept_retention_register = retentionRegister?.Value ?? 0;
            var kept_retention_register_2 = retentionRegister2?.Value ?? 0;

            base.Reset();
            IRQ.Unset();
            InSystemOff = false;

            retentionRegister.Value = kept_retention_register;
            retentionRegister2.Value = kept_retention_register_2;
        }

        // ====================================================================
        // Resets and power
        // ====================================================================

        /// GPIO input 0 is the CPU's SYSRESETREQ line.
        public void OnGPIO(int number, bool value)
        {
            if(number == SystemResetRequestInput && value)
            {
                ResetMachine(ResetReason.SoftReset);
            }
        }

        public void ResetMachine(ResetReason reason)
        {
            this.Log(LogLevel.Info, "Resetting the chip, reason {0}", reason);
            resetReasons |= (uint)reason;
            machine.RequestReset();
        }

        /// Starts the chip as if its battery had just been connected: RAM holds noise and the retained registers are cleared.
        public void PowerCycle()
        {
            ram?.InitWithRandomData();
            resetReasons = 0;
            retentionRegister.Value = 0;
            retentionRegister2.Value = 0;
            machine.RequestReset();
        }

        /// Leaves System OFF as a GPIO DETECT signal would.
        public void WakeFromSystemOff()
        {
            if(InSystemOff)
            {
                ResetMachine(ResetReason.WokeFromSystemOffByGpio);
            }
        }

        private void EnterSystemOff()
        {
            this.Log(LogLevel.Info, "Entering System OFF");
            InSystemOff = true;
            foreach(var cpu in machine.SystemBus.GetCPUs())
            {
                cpu.IsHalted = true;
            }
        }

        // ====================================================================
        // Events and interrupts
        // ====================================================================

        private void TriggerEvent(IFlagRegisterField eventFlag, Registers eventRegister)
        {
            eventFlag.Value = true;
            EventTriggered?.Invoke((uint)eventRegister);
            UpdateInterrupt();
        }

        private void UpdateInterrupt()
        {
            var pending =
                (highFrequencyClockStarted.Value && highFrequencyClockStartedInterruptEnabled.Value)
                || (lowFrequencyClockStarted.Value && lowFrequencyClockStartedInterruptEnabled.Value)
                || (calibrationDone.Value && calibrationDoneInterruptEnabled.Value)
                || (calibrationTimerTimeout.Value && calibrationTimerTimeoutInterruptEnabled.Value)
                || (powerFailureWarning.Value && powerFailureWarningInterruptEnabled.Value)
                || (sleepEntered.Value && sleepEnteredInterruptEnabled.Value)
                || (sleepExited.Value && sleepExitedInterruptEnabled.Value);
            IRQ.Set(pending);
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask(Registers.StartHighFrequencyClock, () =>
            {
                highFrequencyClockRunning = true;
                highFrequencyClockFromCrystal = true;
                TriggerEvent(highFrequencyClockStarted, Registers.HighFrequencyClockStarted);
            });
            DefineTask(Registers.StopHighFrequencyClock, () => highFrequencyClockFromCrystal = false);
            DefineTask(Registers.StartLowFrequencyClock, () =>
            {
                lowFrequencyClockRunning = true;
                lowFrequencyClockSourceInUse = lowFrequencyClockSource.Value;
                TriggerEvent(lowFrequencyClockStarted, Registers.LowFrequencyClockStarted);
            });
            DefineTask(Registers.StopLowFrequencyClock, () => lowFrequencyClockRunning = false);
            DefineTask(Registers.StartCalibration, () => TriggerEvent(calibrationDone, Registers.CalibrationDone));
            DefineTask(Registers.StartCalibrationTimer, () => TriggerEvent(calibrationTimerTimeout, Registers.CalibrationTimerTimeout));
            DefineTask(Registers.StopCalibrationTimer, () => { });
            DefineTask(Registers.ConstantLatency, () => { });
            DefineTask(Registers.LowPower, () => { });

            highFrequencyClockStarted = DefineEvent(Registers.HighFrequencyClockStarted);
            lowFrequencyClockStarted = DefineEvent(Registers.LowFrequencyClockStarted);
            calibrationDone = DefineEvent(Registers.CalibrationDone);
            calibrationTimerTimeout = DefineEvent(Registers.CalibrationTimerTimeout);
            powerFailureWarning = DefineEvent(Registers.PowerFailureWarning);
            sleepEntered = DefineEvent(Registers.SleepEntered);
            sleepExited = DefineEvent(Registers.SleepExited);

            Registers.InterruptEnableSet.Define(this)
                .WithFlag(0, out highFrequencyClockStartedInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "HFCLKSTARTED")
                .WithFlag(1, out lowFrequencyClockStartedInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "LFCLKSTARTED")
                .WithFlag(2, out powerFailureWarningInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "POFWARN")
                .WithFlag(3, out calibrationDoneInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "DONE")
                .WithFlag(4, out calibrationTimerTimeoutInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "CTTO")
                .WithFlag(5, out sleepEnteredInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "SLEEPENTER")
                .WithFlag(6, out sleepExitedInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "SLEEPEXIT")
                .WithReservedBits(7, 25)
                .WithWriteCallback((_, __) => UpdateInterrupt());

            Registers.InterruptEnableClear.Define(this)
                .WithFlag(0, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => highFrequencyClockStartedInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) highFrequencyClockStartedInterruptEnabled.Value = false; }, name: "HFCLKSTARTED")
                .WithFlag(1, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => lowFrequencyClockStartedInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) lowFrequencyClockStartedInterruptEnabled.Value = false; }, name: "LFCLKSTARTED")
                .WithFlag(2, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => powerFailureWarningInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) powerFailureWarningInterruptEnabled.Value = false; }, name: "POFWARN")
                .WithFlag(3, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => calibrationDoneInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) calibrationDoneInterruptEnabled.Value = false; }, name: "DONE")
                .WithFlag(4, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => calibrationTimerTimeoutInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) calibrationTimerTimeoutInterruptEnabled.Value = false; }, name: "CTTO")
                .WithFlag(5, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => sleepEnteredInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) sleepEnteredInterruptEnabled.Value = false; }, name: "SLEEPENTER")
                .WithFlag(6, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => sleepExitedInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) sleepExitedInterruptEnabled.Value = false; }, name: "SLEEPEXIT")
                .WithReservedBits(7, 25)
                .WithWriteCallback((_, __) => UpdateInterrupt());

            Registers.ResetReason.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => resetReasons,
                    writeCallback: (_, cleared) => resetReasons &= ~(uint)cleared, name: "RESETREAS");

            Registers.RamStatus.Define(this)
                .WithValueField(0, 4, FieldMode.Read, valueProviderCallback: _ => 0xF, name: "RAMBLOCK")
                .WithReservedBits(4, 28);

            Registers.HighFrequencyClockRun.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => highFrequencyClockRunning, name: "STATUS")
                .WithReservedBits(1, 31);

            Registers.HighFrequencyClockStatus.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => highFrequencyClockFromCrystal, name: "SRC")
                .WithReservedBits(1, 15)
                .WithFlag(16, FieldMode.Read, valueProviderCallback: _ => highFrequencyClockFromCrystal, name: "STATE")
                .WithReservedBits(17, 15);

            Registers.LowFrequencyClockRun.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => lowFrequencyClockRunning, name: "STATUS")
                .WithReservedBits(1, 31);

            Registers.LowFrequencyClockStatus.Define(this)
                .WithValueField(0, 2, FieldMode.Read, valueProviderCallback: _ => lowFrequencyClockSourceInUse, name: "SRC")
                .WithReservedBits(2, 14)
                .WithFlag(16, FieldMode.Read, valueProviderCallback: _ => lowFrequencyClockRunning, name: "STATE")
                .WithReservedBits(17, 15);

            Registers.LowFrequencyClockSourceCopy.Define(this)
                .WithValueField(0, 2, FieldMode.Read, valueProviderCallback: _ => lowFrequencyClockSourceInUse, name: "SRC")
                .WithReservedBits(2, 30);

            Registers.SystemOff.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, enter) => { if(enter) EnterSystemOff(); }, name: "SYSTEMOFF")
                .WithReservedBits(1, 31);

            Registers.PowerFailureComparator.Define(this)
                .WithFlag(0, name: "POF")
                .WithValueField(1, 4, name: "THRESHOLD")
                .WithReservedBits(5, 27);

            Registers.RetentionRegister.Define(this)
                .WithValueField(0, 8, out retentionRegister, name: "GPREGRET")
                .WithReservedBits(8, 24);

            Registers.RetentionRegister2.Define(this)
                .WithValueField(0, 8, out retentionRegister2, name: "GPREGRET2")
                .WithReservedBits(8, 24);

            Registers.LowFrequencyClockSource.Define(this)
                .WithValueField(0, 2, out lowFrequencyClockSource, name: "SRC")
                .WithReservedBits(2, 14)
                .WithFlag(16, name: "BYPASS")
                .WithFlag(17, name: "EXTERNAL")
                .WithReservedBits(18, 14);

            Registers.CalibrationTimerInterval.Define(this)
                .WithValueField(0, 7, name: "CTIV")
                .WithReservedBits(7, 25);

            Registers.TraceConfig.Define(this)
                .WithValueField(0, 2, name: "TRACEPORTSPEED")
                .WithReservedBits(2, 14)
                .WithValueField(16, 2, name: "TRACEMUX")
                .WithReservedBits(18, 14);

            Registers.DcDcEnable.Define(this)
                .WithFlag(0, name: "DCDCEN")
                .WithReservedBits(1, 31);

            // Nordic's errata workarounds write these undocumented registers.
            Registers.ErrataWorkaround1.Define(this).WithValueField(0, 32, name: "ERRATA_53C");
            Registers.ErrataWorkaround2.Define(this).WithValueField(0, 32, name: "ERRATA_EE4");
            Registers.ErrataWorkaround3.Define(this).WithValueField(0, 32, name: "ERRATA_54C");
            Registers.ErrataWorkaround4.Define(this).WithValueField(0, 32, name: "ERRATA_C34");

            for(var block = 0; block < RamBlockCount; block++)
            {
                var block_power = new DoubleWordRegister(this, 0xFFFF).WithValueField(0, 32, name: $"RAM{block}.POWER");
                RegistersCollection.AddRegister((long)Registers.RamPower + block * RamBlockStride, block_power);
                RegistersCollection.AddRegister((long)Registers.RamPowerSet + block * RamBlockStride, new DoubleWordRegister(this)
                    .WithValueField(0, 32, FieldMode.Write, writeCallback: (_, value) => block_power.Write(0, (uint)(block_power.Read() | value))));
                RegistersCollection.AddRegister((long)Registers.RamPowerClear + block * RamBlockStride, new DoubleWordRegister(this)
                    .WithValueField(0, 32, FieldMode.Write, writeCallback: (_, value) => block_power.Write(0, (uint)(block_power.Read() & ~value))));
            }
        }

        private void DefineTask(Registers task, Action action)
        {
            task.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, start) => { if(start) action(); })
                .WithReservedBits(1, 31);
        }

        private IFlagRegisterField DefineEvent(Registers eventRegister)
        {
            eventRegister.Define(this)
                .WithFlag(0, out var eventFlag)
                .WithReservedBits(1, 31)
                .WithWriteCallback((_, __) => UpdateInterrupt());
            return eventFlag;
        }

        private const int SystemResetRequestInput = 0;

        private readonly MappedMemory ram;
        private const int RamBlockCount = 8;
        private const long RamBlockStride = 0x10;

        private uint resetReasons;
        private bool highFrequencyClockRunning;
        private bool highFrequencyClockFromCrystal;
        private bool lowFrequencyClockRunning;
        private ulong lowFrequencyClockSourceInUse;

        private IFlagRegisterField highFrequencyClockStarted;
        private IFlagRegisterField lowFrequencyClockStarted;
        private IFlagRegisterField calibrationDone;
        private IFlagRegisterField calibrationTimerTimeout;
        private IFlagRegisterField powerFailureWarning;
        private IFlagRegisterField sleepEntered;
        private IFlagRegisterField sleepExited;
        private IFlagRegisterField highFrequencyClockStartedInterruptEnabled;
        private IFlagRegisterField lowFrequencyClockStartedInterruptEnabled;
        private IFlagRegisterField calibrationDoneInterruptEnabled;
        private IFlagRegisterField calibrationTimerTimeoutInterruptEnabled;
        private IFlagRegisterField powerFailureWarningInterruptEnabled;
        private IFlagRegisterField sleepEnteredInterruptEnabled;
        private IFlagRegisterField sleepExitedInterruptEnabled;
        private IValueRegisterField retentionRegister;
        private IValueRegisterField retentionRegister2;
        private IValueRegisterField lowFrequencyClockSource;

        public enum ResetReason : uint
        {
            ResetPin = 1 << 0,
            Watchdog = 1 << 1,
            SoftReset = 1 << 2,
            Lockup = 1 << 3,
            WokeFromSystemOffByGpio = 1 << 16,
            WokeFromSystemOffByLowPowerComparator = 1 << 17,
            WokeFromSystemOffByDebugger = 1 << 18,
            WokeFromSystemOffByNfc = 1 << 19,
        }

        private enum Registers
        {
            StartHighFrequencyClock = 0x000,
            StopHighFrequencyClock = 0x004,
            StartLowFrequencyClock = 0x008,
            StopLowFrequencyClock = 0x00C,
            StartCalibration = 0x010,
            StartCalibrationTimer = 0x014,
            StopCalibrationTimer = 0x018,
            ConstantLatency = 0x078,
            LowPower = 0x07C,
            HighFrequencyClockStarted = 0x100,
            LowFrequencyClockStarted = 0x104,
            PowerFailureWarning = 0x108,
            CalibrationDone = 0x10C,
            CalibrationTimerTimeout = 0x110,
            SleepEntered = 0x114,
            SleepExited = 0x118,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            ResetReason = 0x400,
            HighFrequencyClockRun = 0x408,
            HighFrequencyClockStatus = 0x40C,
            LowFrequencyClockRun = 0x414,
            LowFrequencyClockStatus = 0x418,
            LowFrequencyClockSourceCopy = 0x41C,
            RamStatus = 0x428,
            SystemOff = 0x500,
            PowerFailureComparator = 0x510,
            RetentionRegister = 0x51C,
            RetentionRegister2 = 0x520,
            LowFrequencyClockSource = 0x518,
            CalibrationTimerInterval = 0x538,
            TraceConfig = 0x55C,
            ErrataWorkaround1 = 0x53C,
            DcDcEnable = 0x578,
            RamPower = 0x900,
            RamPowerSet = 0x904,
            RamPowerClear = 0x908,
            ErrataWorkaround3 = 0x54C,
            ErrataWorkaround4 = 0xC34,
            ErrataWorkaround2 = 0xEE4,
        }
    }
}
