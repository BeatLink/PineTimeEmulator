// The nRF52832's quadrature decoder: register storage with reset values; no rotation is ever reported.

using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class NRF52832_QDEC : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_QDEC(IMachine machine) : base(machine)
        {
            IRQ = new GPIO();
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            Array.Clear(events, 0, events.Length);
            interruptEnable = 0;
            accumulatorRead = 0;
            doubleTransitionsRead = 0;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            // The accumulators stay at zero, so reading and clearing them copies zero.
            DefineTask(Registers.TaskStart, "TASKS_START", () => { });
            DefineTask(Registers.TaskStop, "TASKS_STOP", () => SetEvent(Event.Stopped));
            DefineTask(Registers.TaskReadClearAccumulator, "TASKS_READCLRACC", () => accumulatorRead = 0);
            DefineTask(Registers.TaskReadClearAccumulatorAlias, "TASKS_RDCLRACC", () => accumulatorRead = 0);
            DefineTask(Registers.TaskReadClearDoubleTransitions, "TASKS_RDCLRDBL", () => doubleTransitionsRead = 0);

            for(var index = 0; index < EventCount; index++)
            {
                var event_index = index;
                RegistersCollection.DefineRegister(EventsOffset + 4 * index)
                    .WithFlag(0, valueProviderCallback: _ => events[event_index],
                        writeCallback: (_, value) => { events[event_index] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

            Registers.Shorts.Define(this)
                .WithValueField(0, 7, name: "SHORTS")
                .WithReservedBits(7, 25);

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable |= (uint)value; UpdateInterrupts(); }, name: "INTENSET")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable &= ~(uint)value; UpdateInterrupts(); }, name: "INTENCLR")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.Enable.Define(this)
                .WithFlag(0, name: "ENABLE")
                .WithReservedBits(1, 31);

            Registers.LedPolarity.Define(this)
                .WithFlag(0, name: "LEDPOL")
                .WithReservedBits(1, 31);

            Registers.SamplePeriod.Define(this)
                .WithValueField(0, 4, name: "SAMPLEPER")
                .WithReservedBits(4, 28);

            Registers.Sample.Define(this)
                .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => 0, name: "SAMPLE");

            Registers.ReportPeriod.Define(this)
                .WithValueField(0, 4, name: "REPORTPER")
                .WithReservedBits(4, 28);

            Registers.Accumulator.Define(this)
                .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => 0, name: "ACC");

            Registers.AccumulatorRead.Define(this)
                .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => accumulatorRead, name: "ACCREAD");

            DefinePinSelect(Registers.LedPinSelect, "LED");
            DefinePinSelect(Registers.APinSelect, "A");
            DefinePinSelect(Registers.BPinSelect, "B");

            Registers.DebounceFilterEnable.Define(this)
                .WithFlag(0, name: "DBFEN")
                .WithReservedBits(1, 31);

            Registers.LedPreTime.Define(this, 0x10)
                .WithValueField(0, 9, name: "LEDPRE")
                .WithReservedBits(9, 23);

            Registers.DoubleTransitions.Define(this)
                .WithValueField(0, 4, FieldMode.Read, valueProviderCallback: _ => 0, name: "ACCDBL")
                .WithReservedBits(4, 28);

            Registers.DoubleTransitionsRead.Define(this)
                .WithValueField(0, 4, FieldMode.Read, valueProviderCallback: _ => doubleTransitionsRead, name: "ACCDBLREAD")
                .WithReservedBits(4, 28);
        }

        private void DefineTask(Registers register, string name, Action task)
        {
            register.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) task(); }, name: name)
                .WithReservedBits(1, 31);
        }

        private void DefinePinSelect(Registers register, string name)
        {
            register.Define(this, 0xFFFFFFFF)
                .WithValueField(0, 5, name: "PIN")
                .WithReservedBits(5, 26)
                .WithFlag(31, name: "CONNECT");
        }

        // ====================================================================
        // Events and interrupts
        // ====================================================================

        private void SetEvent(Event @event)
        {
            var index = (int)@event;
            events[index] = true;
            EventTriggered?.Invoke((uint)(EventsOffset + 4 * index));
            UpdateInterrupts();
        }

        private void UpdateInterrupts()
        {
            var pending = false;
            for(var index = 0; index < EventCount; index++)
            {
                pending |= events[index] && (interruptEnable & (1u << index)) != 0;
            }
            IRQ.Set(pending);
        }

        private const int EventCount = 5;
        private const int EventsOffset = 0x100;

        private readonly bool[] events = new bool[EventCount];

        private uint interruptEnable;
        private uint accumulatorRead;
        private uint doubleTransitionsRead;

        private enum Event
        {
            SampleReady = 0,
            ReportReady = 1,
            AccumulatorOverflow = 2,
            DoubleTransitionsReady = 3,
            Stopped = 4,
        }

        private enum Registers
        {
            TaskStart = 0x000,
            TaskStop = 0x004,
            TaskReadClearAccumulator = 0x008,
            TaskReadClearAccumulatorAlias = 0x00C,
            TaskReadClearDoubleTransitions = 0x010,
            Shorts = 0x200,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Enable = 0x500,
            LedPolarity = 0x504,
            SamplePeriod = 0x508,
            Sample = 0x50C,
            ReportPeriod = 0x510,
            Accumulator = 0x514,
            AccumulatorRead = 0x518,
            LedPinSelect = 0x51C,
            APinSelect = 0x520,
            BPinSelect = 0x524,
            DebounceFilterEnable = 0x528,
            LedPreTime = 0x540,
            DoubleTransitions = 0x544,
            DoubleTransitionsRead = 0x548,
        }
    }
}
