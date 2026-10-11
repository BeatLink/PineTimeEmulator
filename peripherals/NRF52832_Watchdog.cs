// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832 watchdog: once started it cannot be stopped, and it resets the chip unless every enabled reload register is written in time.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Miscellaneous;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Timers
{
    public sealed class NRF52832_Watchdog : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_Watchdog(IMachine machine, NRF52832_PowerClock powerClock) : base(machine)
        {
            this.powerClock = powerClock;
            IRQ = new GPIO();
            countdown = new LimitTimer(machine.ClockSource, LowFrequencyClockHertz, this, "countdown",
                limit: (ulong)DefaultReloadValue + 1, direction: Direction.Descending, eventEnabled: true);
            countdown.LimitReached += OnTimeout;
            DefineRegisters();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        public bool Running => countdown.Enabled;

        public override void Reset()
        {
            base.Reset();
            countdown.Reset();
            IRQ.Unset();
            reloadRequestsPending = 0;
            resetScheduled = false;
        }

        // ====================================================================
        // Counting down
        // ====================================================================

        private void Start()
        {
            if(countdown.Enabled)
            {
                return;
            }

            this.Log(LogLevel.Info, "Watchdog started, {0} ticks of 32768 Hz", reloadValue.Value + 1);
            countdown.Limit = reloadValue.Value + 1;
            countdown.Value = countdown.Limit;
            reloadRequestsPending = (uint)reloadRequestsEnabled.Value;
            countdown.Enabled = true;
        }

        private void Reload(int reloadRegister)
        {
            reloadRequestsPending &= ~(1u << reloadRegister);
            if(reloadRequestsPending != 0 || !countdown.Enabled || resetScheduled)
            {
                return;
            }

            countdown.Value = countdown.Limit;
            reloadRequestsPending = (uint)reloadRequestsEnabled.Value;
        }

        // With the interrupt enabled the chip gets two 32 kHz ticks to react before the reset.
        private void OnTimeout()
        {
            if(resetScheduled || !timeoutInterruptEnabled.Value)
            {
                powerClock.ResetMachine(NRF52832_PowerClock.ResetReason.Watchdog);
                return;
            }

            this.Log(LogLevel.Info, "Watchdog timed out; resetting in two ticks");
            timeoutEvent.Value = true;
            EventTriggered?.Invoke((uint)Registers.Timeout);
            IRQ.Set(true);
            resetScheduled = true;
            countdown.Limit = TicksBeforeResetAfterInterrupt;
            countdown.Value = TicksBeforeResetAfterInterrupt;
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            Registers.Start.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, start) => { if(start) Start(); }, name: "TASKS_START")
                .WithReservedBits(1, 31);

            Registers.Timeout.Define(this)
                .WithFlag(0, out timeoutEvent, name: "EVENTS_TIMEOUT")
                .WithReservedBits(1, 31)
                .WithWriteCallback((_, __) => IRQ.Set(timeoutEvent.Value && timeoutInterruptEnabled.Value));

            Registers.InterruptEnableSet.Define(this)
                .WithFlag(0, out timeoutInterruptEnabled, FieldMode.Read | FieldMode.Set, name: "TIMEOUT")
                .WithReservedBits(1, 31);

            Registers.InterruptEnableClear.Define(this)
                .WithFlag(0, FieldMode.Read | FieldMode.WriteOneToClear, valueProviderCallback: _ => timeoutInterruptEnabled.Value,
                    writeCallback: (_, clear) => { if(clear) timeoutInterruptEnabled.Value = false; }, name: "TIMEOUT")
                .WithReservedBits(1, 31);

            Registers.RunStatus.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => countdown.Enabled, name: "RUNSTATUS")
                .WithReservedBits(1, 31);

            Registers.RequestStatus.Define(this)
                .WithValueField(0, 8, FieldMode.Read, valueProviderCallback: _ => countdown.Enabled ? reloadRequestsPending : (uint)reloadRequestsEnabled.Value, name: "RR")
                .WithReservedBits(8, 24);

            Registers.CounterReloadValue.Define(this, DefaultReloadValue)
                .WithValueField(0, 32, out reloadValue, writeCallback: WarnIfRunning, name: "CRV");

            Registers.ReloadRequestEnable.Define(this, 0x1)
                .WithValueField(0, 8, out reloadRequestsEnabled, writeCallback: WarnIfRunning, name: "RREN")
                .WithReservedBits(8, 24);

            Registers.Config.Define(this, 0x1)
                .WithFlag(0, name: "SLEEP")
                .WithReservedBits(1, 2)
                .WithFlag(3, name: "HALT")
                .WithReservedBits(4, 28);

            for(var reload_register = 0; reload_register < ReloadRegisterCount; reload_register++)
            {
                var index = reload_register;
                RegistersCollection.AddRegister((long)Registers.ReloadRequest + index * 4, new DoubleWordRegister(this)
                    .WithValueField(0, 32, FieldMode.Write, writeCallback: (_, value) =>
                    {
                        if(value == ReloadMagicValue)
                        {
                            Reload(index);
                        }
                    }, name: $"RR[{index}]"));
            }
        }

        private void WarnIfRunning(ulong oldValue, ulong newValue)
        {
            if(countdown.Enabled)
            {
                this.Log(LogLevel.Warning, "The firmware changed a watchdog setting while it runs; the chip ignores that");
            }
        }

        private const long LowFrequencyClockHertz = 32768;
        private const uint DefaultReloadValue = 0xFFFFFFFF;
        private const ulong TicksBeforeResetAfterInterrupt = 2;
        private const int ReloadRegisterCount = 8;
        private const uint ReloadMagicValue = 0x6E524635;

        private readonly NRF52832_PowerClock powerClock;
        private readonly LimitTimer countdown;
        private uint reloadRequestsPending;
        private bool resetScheduled;
        private IFlagRegisterField timeoutEvent;
        private IFlagRegisterField timeoutInterruptEnabled;
        private IValueRegisterField reloadValue;
        private IValueRegisterField reloadRequestsEnabled;

        private enum Registers
        {
            Start = 0x000,
            Timeout = 0x100,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            RunStatus = 0x400,
            RequestStatus = 0x404,
            CounterReloadValue = 0x504,
            ReloadRequestEnable = 0x508,
            Config = 0x50C,
            ReloadRequest = 0x600,
        }
    }
}
