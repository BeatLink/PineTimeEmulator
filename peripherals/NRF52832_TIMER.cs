// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832 timer/counter: a 16 MHz prescaled counter, or a task-driven counter, with compare, capture and shortcuts, worked out from virtual time.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Exceptions;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.Miscellaneous;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Timers
{
    public sealed class NRF52832_TIMER : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_TIMER(IMachine machine, int numberOfCompareChannels = 4) : base(machine)
        {
            if(numberOfCompareChannels < 1 || numberOfCompareChannels > MaximumCompareChannels)
            {
                throw new ConstructionException($"The TIMER has 1 to {MaximumCompareChannels} compare channels, not {numberOfCompareChannels}");
            }
            compareChannelCount = numberOfCompareChannels;
            compareValues = new uint[compareChannelCount];
            channelMask = ((uint)1 << compareChannelCount) - 1;
            alarmHandler = OnAlarm;
            IRQ = new GPIO();
            DefineRegisters();
            Reset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        public override void Reset()
        {
            base.Reset();
            running = false;
            anchorTime = Now();
            baseTick = 0;
            baseCount = 0;
            processedTicks = 0;
            mode = Mode.Timer;
            bitMode = BitMode.Bits16;
            prescaler = DefaultPrescaler;
            shortcuts = 0;
            interruptEnabled = 0;
            eventsPending = 0;
            Array.Clear(compareValues, 0, compareValues.Length);
            DisarmAlarm();
            IRQ.Unset();
        }

        // ====================================================================
        // Counting
        // ====================================================================

        // In timer mode the counter is the base count plus the ticks since the base tick, and ticks count from the anchor time.
        private ulong Now()
        {
            return machine.ClockSource.CurrentValue.Ticks;
        }

        private bool CountsTime => running && mode == Mode.Timer;

        // One 16 MHz tick lasts 125 half-nanoseconds, doubled for each prescaler step; values above 9 behave as 9.
        private ulong HalfNanosecondsPerTick => HalfNanosecondsPerFastTick << (int)Math.Min(prescaler, MaximumPrescaler);

        private ulong CurrentTick()
        {
            return MultiplyThenDivide(Now() - anchorTime, HalfNanosecondsPerNanosecond, HalfNanosecondsPerTick);
        }

        private uint CounterMask
        {
            get
            {
                switch(bitMode)
                {
                case BitMode.Bits08:
                    return 0xFF;
                case BitMode.Bits24:
                    return 0xFFFFFF;
                case BitMode.Bits32:
                    return 0xFFFFFFFF;
                default:
                    return 0xFFFF;
                }
            }
        }

        private uint CounterAt(ulong tick)
        {
            return (uint)((baseCount + (tick - baseTick)) & CounterMask);
        }

        private uint CurrentCounter()
        {
            return CountsTime ? CounterAt(CurrentTick()) : baseCount;
        }

        // Restarts the tick count from now, keeping the counter value, so a new prescaler or mode applies from this moment.
        private void Reanchor()
        {
            baseCount = CurrentCounter();
            anchorTime = Now();
            baseTick = 0;
            processedTicks = 0;
        }

        // Exact floor(value * numerator / denominator) without overflowing 64 bits.
        private static ulong MultiplyThenDivide(ulong value, ulong numerator, ulong denominator)
        {
            return value / denominator * numerator + value % denominator * numerator / denominator;
        }

        // Exact ceiling(value * numerator / denominator) without overflowing 64 bits.
        private static ulong MultiplyThenDivideRoundingUp(ulong value, ulong numerator, ulong denominator)
        {
            var remainderProduct = value % denominator * numerator;
            return value / denominator * numerator + (remainderProduct + denominator - 1) / denominator;
        }

        // ====================================================================
        // Tasks
        // ====================================================================

        private void StartTask()
        {
            Synchronize();
            if(!running)
            {
                running = true;
                anchorTime = Now();
                baseTick = 0;
                processedTicks = 0;
            }
            ScheduleNextEvent();
        }

        private void StopTask()
        {
            Synchronize();
            if(running)
            {
                baseCount = CurrentCounter();
                running = false;
            }
            ScheduleNextEvent();
        }

        // Clearing keeps the phase of the prescaled clock, so periodic clears never drift.
        private void ClearTask()
        {
            Synchronize();
            if(CountsTime)
            {
                baseTick = CurrentTick();
                processedTicks = baseTick;
            }
            baseCount = 0;
            ScheduleNextEvent();
        }

        // In counter mode each COUNT adds one; in timer mode it does nothing.
        private void CountTask()
        {
            Synchronize();
            if(!running || mode == Mode.Timer)
            {
                return;
            }
            baseCount = (baseCount + 1) & CounterMask;
            ApplyCompares(baseCount, () => baseCount = 0);
            UpdateInterrupt();
        }

        private void CaptureTask(int channel)
        {
            Synchronize();
            compareValues[channel] = CurrentCounter();
            ScheduleNextEvent();
        }

        // ====================================================================
        // Events
        // ====================================================================

        // Catches up on every compare due by now, so register changes apply only from this moment on.
        private void Synchronize()
        {
            SynchronizeCpuTime();
            if(CountsTime)
            {
                ProcessEventsUntil(CurrentTick());
            }
        }

        private void ProcessEventsUntil(ulong currentTick)
        {
            while(CountsTime)
            {
                var nextTick = NextEventTick();
                if(nextTick > currentTick)
                {
                    break;
                }
                processedTicks = nextTick;
                ApplyCompares(CounterAt(nextTick), () =>
                {
                    baseTick = nextTick;
                    baseCount = 0;
                });
            }
            if(currentTick > processedTicks)
            {
                processedTicks = currentTick;
            }
            UpdateInterrupt();
        }

        // A compare fires when the counter steps onto CC, so writing CC equal to the counter does not fire until the counter wraps.
        private ulong NextEventTick()
        {
            var nextTick = NoEvent;
            var counter = CounterAt(processedTicks);
            var modulus = (ulong)CounterMask + 1;
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                var target = compareValues[channel];
                if(target > CounterMask || !IsWorthScheduling(channel))
                {
                    continue;
                }
                var distance = (target - counter) & CounterMask;
                nextTick = Math.Min(nextTick, processedTicks + (distance == 0 ? modulus : distance));
            }
            return nextTick;
        }

        // A pending compare with no shortcut and no PPI listener cannot change anything until it is cleared.
        private bool IsWorthScheduling(int channel)
        {
            var hasShortcut = (shortcuts & (((uint)1 << channel) | ((uint)1 << (channel + StopShortcutShift)))) != 0;
            return hasShortcut || EventTriggered != null || (eventsPending & ((uint)1 << channel)) == 0;
        }

        // Raises every compare matching the counter, then runs their CLEAR and STOP shortcuts at that same instant.
        private void ApplyCompares(uint counter, Action clearCounter)
        {
            var clear = false;
            var stop = false;
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                if(compareValues[channel] != counter)
                {
                    continue;
                }
                eventsPending |= (uint)1 << channel;
                EventTriggered?.Invoke((uint)Registers.Compare + (uint)channel * 4);
                clear |= (shortcuts & ((uint)1 << channel)) != 0;
                stop |= (shortcuts & ((uint)1 << (channel + StopShortcutShift))) != 0;
            }
            if(clear)
            {
                clearCounter();
            }
            if(stop)
            {
                baseCount = clear ? 0 : counter;
                running = false;
            }
        }

        private void UpdateInterrupt()
        {
            IRQ.Set(((eventsPending << InterruptBitShift) & interruptEnabled) != 0);
        }

        // ====================================================================
        // Virtual time alarm
        // ====================================================================

        // One alarm on the machine clock wakes the model at the next compare, so the counter never ticks one step at a time.
        private void ScheduleNextEvent()
        {
            var nextTick = CountsTime ? NextEventTick() : NoEvent;
            if(nextTick == NoEvent)
            {
                DisarmAlarm();
                return;
            }
            var eventTime = anchorTime + MultiplyThenDivideRoundingUp(nextTick, HalfNanosecondsPerTick, HalfNanosecondsPerNanosecond);
            var now = Now();
            ArmAlarm(eventTime > now ? eventTime - now : 1);
        }

        private void OnAlarm()
        {
            insideAlarm = true;
            try
            {
                if(CountsTime)
                {
                    ProcessEventsUntil(CurrentTick());
                }
                ScheduleNextEvent();
            }
            finally
            {
                insideAlarm = false;
            }
        }

        private void ArmAlarm(ulong delayNanoseconds)
        {
            machine.ClockSource.ExchangeClockEntryWith(alarmHandler,
                entry => entry.With(period: delayNanoseconds, value: 0, enabled: true),
                () => new ClockEntry(delayNanoseconds, NanosecondsPerSecond, alarmHandler, this, "alarm", enabled: true, direction: Direction.Ascending, workMode: WorkMode.OneShot));
            RequestReturnOnCurrentCpu();
        }

        private void DisarmAlarm()
        {
            machine.ClockSource.ExchangeClockEntryWith(alarmHandler,
                entry => entry.With(enabled: false),
                () => new ClockEntry(1, NanosecondsPerSecond, alarmHandler, this, "alarm", enabled: false, direction: Direction.Ascending, workMode: WorkMode.OneShot));
        }

        // Brings virtual time up to the instruction that is touching the registers, so CAPTURE reads the exact count.
        private void SynchronizeCpuTime()
        {
            if(insideAlarm)
            {
                return;
            }
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu) && cpu.OnPossessedThread)
            {
                cpu.SyncTime();
            }
        }

        // Makes the running CPU stop at the new alarm instead of the end of its time slice.
        private void RequestReturnOnCurrentCpu()
        {
            if(EmulationManager.Instance.CurrentEmulation.TryGetExecutionContext(out var _, out var cpu))
            {
                (cpu as IControllableCPU)?.RequestReturn();
            }
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask((long)Registers.Start, "TASKS_START", StartTask);
            DefineTask((long)Registers.Stop, "TASKS_STOP", StopTask);
            DefineTask((long)Registers.Count, "TASKS_COUNT", CountTask);
            DefineTask((long)Registers.Clear, "TASKS_CLEAR", ClearTask);
            // SHUTDOWN is deprecated on the nRF52832 and stops the timer like STOP.
            DefineTask((long)Registers.Shutdown, "TASKS_SHUTDOWN", StopTask);
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                var index = channel;
                DefineTask((long)Registers.Capture + index * 4, $"TASKS_CAPTURE[{index}]", () => CaptureTask(index));
            }

            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                var bit = (uint)1 << channel;
                RegistersCollection.AddRegister((long)Registers.Compare + channel * 4, new DoubleWordRegister(this)
                    .WithFlag(0, valueProviderCallback: _ =>
                    {
                        SynchronizeCpuTime();
                        return (eventsPending & bit) != 0;
                    }, writeCallback: (_, value) =>
                    {
                        Synchronize();
                        eventsPending = value ? eventsPending | bit : eventsPending & ~bit;
                        ScheduleNextEvent();
                        UpdateInterrupt();
                    }, name: $"EVENTS_COMPARE[{channel}]")
                    .WithReservedBits(1, 31));
            }

            Registers.Shortcuts.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => shortcuts, writeCallback: (_, value) =>
                {
                    Synchronize();
                    shortcuts = (uint)value & (channelMask | (channelMask << StopShortcutShift));
                    ScheduleNextEvent();
                }, name: "SHORTS");

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnabled, writeCallback: (_, value) =>
                {
                    interruptEnabled |= (uint)value & (channelMask << InterruptBitShift);
                    UpdateInterrupt();
                }, name: "INTENSET");

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnabled, writeCallback: (_, value) =>
                {
                    interruptEnabled &= ~((uint)value & (channelMask << InterruptBitShift));
                    UpdateInterrupt();
                }, name: "INTENCLR");

            Registers.Mode.Define(this)
                .WithValueField(0, 2, valueProviderCallback: _ => (ulong)mode, writeCallback: (_, value) =>
                {
                    ChangeSetting(() => mode = (Mode)value);
                }, name: "MODE")
                .WithReservedBits(2, 30);

            Registers.BitMode.Define(this)
                .WithValueField(0, 2, valueProviderCallback: _ => (ulong)bitMode, writeCallback: (_, value) =>
                {
                    ChangeSetting(() =>
                    {
                        bitMode = (BitMode)value;
                        baseCount &= CounterMask;
                    });
                }, name: "BITMODE")
                .WithReservedBits(2, 30);

            Registers.Prescaler.Define(this, DefaultPrescaler)
                .WithValueField(0, 4, valueProviderCallback: _ => prescaler, writeCallback: (_, value) =>
                {
                    ChangeSetting(() => prescaler = value);
                }, name: "PRESCALER")
                .WithReservedBits(4, 28);

            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                var index = channel;
                RegistersCollection.AddRegister((long)Registers.CompareValue + index * 4, new DoubleWordRegister(this)
                    .WithValueField(0, 32, valueProviderCallback: _ => compareValues[index], writeCallback: (_, value) =>
                    {
                        Synchronize();
                        compareValues[index] = (uint)value;
                        ScheduleNextEvent();
                    }, name: $"CC[{index}]"));
            }
        }

        // The chip expects MODE, BITMODE and PRESCALER to change only while stopped; a running timer picks them up from now.
        private void ChangeSetting(Action change)
        {
            Synchronize();
            if(running)
            {
                this.Log(LogLevel.Debug, "Timer setting changed while running; applying it from now");
            }
            Reanchor();
            change();
            ScheduleNextEvent();
        }

        private void DefineTask(long offset, string name, Action task)
        {
            RegistersCollection.AddRegister(offset, new DoubleWordRegister(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, trigger) => { if(trigger) task(); }, name: name)
                .WithReservedBits(1, 31));
        }

        private const ulong NanosecondsPerSecond = 1000000000;
        private const ulong HalfNanosecondsPerFastTick = 125;
        private const ulong HalfNanosecondsPerNanosecond = 2;
        private const ulong MaximumPrescaler = 9;
        private const uint DefaultPrescaler = 4;
        private const int MaximumCompareChannels = 6;
        private const int StopShortcutShift = 8;
        private const int InterruptBitShift = 16;
        private const ulong NoEvent = ulong.MaxValue;

        private readonly int compareChannelCount;
        private readonly uint[] compareValues;
        private readonly uint channelMask;
        private readonly Action alarmHandler;

        private bool running;
        private bool insideAlarm;
        private ulong anchorTime;
        private ulong baseTick;
        private uint baseCount;
        private ulong processedTicks;
        private Mode mode;
        private BitMode bitMode;
        private ulong prescaler;
        private uint shortcuts;
        private uint interruptEnabled;
        private uint eventsPending;

        private enum Mode
        {
            Timer = 0,
            Counter = 1,
            LowPowerCounter = 2,
        }

        private enum BitMode
        {
            Bits16 = 0,
            Bits08 = 1,
            Bits24 = 2,
            Bits32 = 3,
        }

        private enum Registers
        {
            Start = 0x000,
            Stop = 0x004,
            Count = 0x008,
            Clear = 0x00C,
            Shutdown = 0x010,
            Capture = 0x040,
            Compare = 0x140,
            Shortcuts = 0x200,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Mode = 0x504,
            BitMode = 0x508,
            Prescaler = 0x510,
            CompareValue = 0x540,
        }
    }
}
