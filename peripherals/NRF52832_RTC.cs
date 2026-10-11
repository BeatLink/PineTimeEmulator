// The nRF52832 real-time counter: a 24-bit counter on the 32768 Hz clock with tick, overflow and compare events, worked out from virtual time.

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
    public sealed class NRF52832_RTC : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_RTC(IMachine machine, int numberOfCompareChannels = 4) : base(machine)
        {
            if(numberOfCompareChannels < 1 || numberOfCompareChannels > MaximumCompareChannels)
            {
                throw new ConstructionException($"The RTC has 1 to {MaximumCompareChannels} compare channels, not {numberOfCompareChannels}");
            }
            compareChannelCount = numberOfCompareChannels;
            compareValues = new uint[compareChannelCount];
            enableMask = TickBit | OverflowBit | ((((uint)1 << compareChannelCount) - 1) << CompareBitShift);
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
            prescaler = 0;
            interruptEnabled = 0;
            eventRoutingEnabled = 0;
            eventsPending = 0;
            Array.Clear(compareValues, 0, compareValues.Length);
            DisarmAlarm();
            IRQ.Unset();
        }

        // ====================================================================
        // Counting
        // ====================================================================

        // While running, the counter is the base count plus the ticks since the base tick, and ticks count from the anchor time.
        private ulong Now()
        {
            return machine.ClockSource.CurrentValue.Ticks;
        }

        // One tick lasts (PRESCALER + 1) / 32768 seconds, so ticks = nanoseconds * 32768 / this.
        private ulong TickDenominator => NanosecondsPerSecond * (prescaler + 1);

        private ulong CurrentTick()
        {
            return MultiplyThenDivide(Now() - anchorTime, LowFrequencyClockHertz, TickDenominator);
        }

        private uint CounterAt(ulong tick)
        {
            return (uint)((baseCount + (tick - baseTick)) & CounterMask);
        }

        private uint CurrentCounter()
        {
            return running ? CounterAt(CurrentTick()) : baseCount;
        }

        // Moves the counter to a new value without losing the phase of the 32768 Hz clock.
        private void SetCounter(uint value)
        {
            if(running)
            {
                baseTick = CurrentTick();
                processedTicks = baseTick;
            }
            baseCount = value & CounterMask;
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

        private void ClearTask()
        {
            Synchronize();
            SetCounter(0);
            ScheduleNextEvent();
        }

        private void TriggerOverflowTask()
        {
            Synchronize();
            SetCounter(TriggerOverflowValue);
            ScheduleNextEvent();
        }

        // ====================================================================
        // Events
        // ====================================================================

        // Catches up on every event due by now, so register changes apply only from this moment on.
        private void Synchronize()
        {
            SynchronizeCpuTime();
            if(running)
            {
                ProcessEventsUntil(CurrentTick());
            }
        }

        private void ProcessEventsUntil(ulong currentTick)
        {
            while(running)
            {
                var nextTick = NextEventTick();
                if(nextTick > currentTick)
                {
                    break;
                }
                processedTicks = nextTick;
                FireEventsAt(nextTick);
            }
            if(currentTick > processedTicks)
            {
                processedTicks = currentTick;
            }
            UpdateInterrupt();
        }

        // A compare fires when the counter steps onto CC, so a CC equal to the counter waits a full wrap and CC = COUNTER + 1 fires on the next tick.
        private ulong NextEventTick()
        {
            var nextTick = NoEvent;
            var counter = CounterAt(processedTicks);
            if(IsWorthScheduling(TickBit))
            {
                nextTick = processedTicks + 1;
            }
            if(IsWorthScheduling(OverflowBit))
            {
                nextTick = Math.Min(nextTick, processedTicks + TicksUntil(counter, 0));
            }
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                if(IsWorthScheduling(CompareBit(channel)))
                {
                    nextTick = Math.Min(nextTick, processedTicks + TicksUntil(counter, compareValues[channel]));
                }
            }
            return nextTick;
        }

        private static ulong TicksUntil(uint counter, uint target)
        {
            var distance = (target - counter) & CounterMask;
            return distance == 0 ? CounterMask + 1 : distance;
        }

        // An event is only generated while enabled, and a pending event with no PPI route cannot change anything until it is cleared.
        private bool IsWorthScheduling(uint bit)
        {
            if(((interruptEnabled | eventRoutingEnabled) & bit) == 0)
            {
                return false;
            }
            var routedToPpi = (eventRoutingEnabled & bit) != 0 && EventTriggered != null;
            return routedToPpi || (eventsPending & bit) == 0;
        }

        private void FireEventsAt(ulong tick)
        {
            var counter = CounterAt(tick);
            FireEvent(TickBit, (uint)Registers.Tick);
            if(counter == 0)
            {
                FireEvent(OverflowBit, (uint)Registers.Overflow);
            }
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                if(counter == compareValues[channel])
                {
                    FireEvent(CompareBit(channel), (uint)Registers.Compare + (uint)channel * 4);
                }
            }
        }

        // INTEN or EVTEN lets the event set its register; only EVTEN passes it on to the PPI.
        private void FireEvent(uint bit, uint eventOffset)
        {
            if(((interruptEnabled | eventRoutingEnabled) & bit) == 0)
            {
                return;
            }
            eventsPending |= bit;
            if((eventRoutingEnabled & bit) != 0)
            {
                EventTriggered?.Invoke(eventOffset);
            }
        }

        private void UpdateInterrupt()
        {
            IRQ.Set((eventsPending & interruptEnabled) != 0);
        }

        private static uint CompareBit(int channel)
        {
            return (uint)1 << (CompareBitShift + channel);
        }

        // ====================================================================
        // Virtual time alarm
        // ====================================================================

        // One alarm on the machine clock wakes the model at the next event, so a quiet RTC costs nothing per tick.
        private void ScheduleNextEvent()
        {
            var nextTick = running ? NextEventTick() : NoEvent;
            if(nextTick == NoEvent)
            {
                DisarmAlarm();
                return;
            }
            var eventTime = anchorTime + MultiplyThenDivideRoundingUp(nextTick, TickDenominator, LowFrequencyClockHertz);
            var now = Now();
            ArmAlarm(eventTime > now ? eventTime - now : 1);
        }

        private void OnAlarm()
        {
            insideAlarm = true;
            try
            {
                if(running)
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

        // Brings virtual time up to the instruction that is touching the registers.
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
            DefineTask(Registers.Start, "TASKS_START", StartTask);
            DefineTask(Registers.Stop, "TASKS_STOP", StopTask);
            DefineTask(Registers.Clear, "TASKS_CLEAR", ClearTask);
            DefineTask(Registers.TriggerOverflow, "TASKS_TRIGOVRFLW", TriggerOverflowTask);

            DefineEvent((long)Registers.Tick, TickBit, "EVENTS_TICK");
            DefineEvent((long)Registers.Overflow, OverflowBit, "EVENTS_OVRFLW");
            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                DefineEvent((long)Registers.Compare + channel * 4, CompareBit(channel), $"EVENTS_COMPARE[{channel}]");
            }

            DefineEnableRegister(Registers.InterruptEnableSet, "INTENSET", () => interruptEnabled, value => interruptEnabled |= value);
            DefineEnableRegister(Registers.InterruptEnableClear, "INTENCLR", () => interruptEnabled, value => interruptEnabled &= ~value);
            DefineEnableRegister(Registers.EventEnable, "EVTEN", () => eventRoutingEnabled, value => eventRoutingEnabled = value);
            DefineEnableRegister(Registers.EventEnableSet, "EVTENSET", () => eventRoutingEnabled, value => eventRoutingEnabled |= value);
            DefineEnableRegister(Registers.EventEnableClear, "EVTENCLR", () => eventRoutingEnabled, value => eventRoutingEnabled &= ~value);

            Registers.Counter.Define(this)
                .WithValueField(0, 24, FieldMode.Read, valueProviderCallback: _ =>
                {
                    SynchronizeCpuTime();
                    return CurrentCounter();
                }, name: "COUNTER")
                .WithReservedBits(24, 8);

            Registers.Prescaler.Define(this)
                .WithValueField(0, 12, valueProviderCallback: _ => prescaler, writeCallback: (_, value) =>
                {
                    if(running)
                    {
                        this.Log(LogLevel.Debug, "PRESCALER is read-only while the RTC runs; ignoring 0x{0:X}", value);
                        return;
                    }
                    prescaler = value;
                }, name: "PRESCALER")
                .WithReservedBits(12, 20);

            for(var channel = 0; channel < compareChannelCount; channel++)
            {
                var index = channel;
                RegistersCollection.AddRegister((long)Registers.CompareValue + index * 4, new DoubleWordRegister(this)
                    .WithValueField(0, 24, valueProviderCallback: _ => compareValues[index], writeCallback: (_, value) =>
                    {
                        Synchronize();
                        compareValues[index] = (uint)value;
                        ScheduleNextEvent();
                    }, name: $"CC[{index}]")
                    .WithReservedBits(24, 8));
            }
        }

        private void DefineTask(Registers register, string name, Action task)
        {
            register.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, trigger) => { if(trigger) task(); }, name: name)
                .WithReservedBits(1, 31);
        }

        private void DefineEvent(long offset, uint bit, string name)
        {
            RegistersCollection.AddRegister(offset, new DoubleWordRegister(this)
                .WithFlag(0, valueProviderCallback: _ => { SynchronizeCpuTime(); return (eventsPending & bit) != 0; }, writeCallback: (_, value) =>
                {
                    Synchronize();
                    eventsPending = value ? eventsPending | bit : eventsPending & ~bit;
                    ScheduleNextEvent();
                    UpdateInterrupt();
                }, name: name)
                .WithReservedBits(1, 31));
        }

        private void DefineEnableRegister(Registers register, string name, Func<uint> read, Action<uint> write)
        {
            register.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => read(), writeCallback: (_, value) =>
                {
                    Synchronize();
                    write((uint)value & enableMask);
                    ScheduleNextEvent();
                    UpdateInterrupt();
                }, name: name);
        }

        private const ulong LowFrequencyClockHertz = 32768;
        private const ulong NanosecondsPerSecond = 1000000000;
        private const uint CounterMask = 0xFFFFFF;
        private const uint TriggerOverflowValue = 0xFFFFF0;
        private const int MaximumCompareChannels = 4;
        private const uint TickBit = 1u << 0;
        private const uint OverflowBit = 1u << 1;
        private const int CompareBitShift = 16;
        private const ulong NoEvent = ulong.MaxValue;

        private readonly int compareChannelCount;
        private readonly uint[] compareValues;
        private readonly uint enableMask;
        private readonly Action alarmHandler;

        private bool running;
        private bool insideAlarm;
        private ulong anchorTime;
        private ulong baseTick;
        private uint baseCount;
        private ulong processedTicks;
        private ulong prescaler;
        private uint interruptEnabled;
        private uint eventRoutingEnabled;
        private uint eventsPending;

        private enum Registers
        {
            Start = 0x000,
            Stop = 0x004,
            Clear = 0x008,
            TriggerOverflow = 0x00C,
            Tick = 0x100,
            Overflow = 0x104,
            Compare = 0x140,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            EventEnable = 0x340,
            EventEnableSet = 0x344,
            EventEnableClear = 0x348,
            Counter = 0x504,
            Prescaler = 0x508,
            CompareValue = 0x540,
        }
    }
}
