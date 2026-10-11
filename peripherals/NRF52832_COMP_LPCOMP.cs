// The nRF52832's shared comparator and low-power comparator: register storage, a READY event on START, and a result set from the monitor.

using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // COMP (ENABLE = 2) and LPCOMP (ENABLE = 1) share one 4 KB block and one register set.
    public sealed class NRF52832_COMP_LPCOMP : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_COMP_LPCOMP(IMachine machine) : base(machine)
        {
            IRQ = new GPIO();
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            Array.Clear(events, 0, events.Length);
            interruptEnable = 0;
            started = false;
            inputAbove = false;
            result = false;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Monitor interface
        // ====================================================================

        // Sets whether the input is above the reference; while started, a change raises UP or DOWN and CROSS.
        public void SetInputAbove(bool above)
        {
            var changed = above != inputAbove;
            inputAbove = above;
            if(!started || !changed)
            {
                return;
            }
            result = above;
            SetEvent(above ? Event.Up : Event.Down);
            SetEvent(Event.Cross);
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask(Registers.TaskStart, "TASKS_START", Start);
            DefineTask(Registers.TaskStop, "TASKS_STOP", () => started = false);
            DefineTask(Registers.TaskSample, "TASKS_SAMPLE", () => { if(started) result = inputAbove; });

            for(var index = 0; index < EventCount; index++)
            {
                var event_index = index;
                RegistersCollection.DefineRegister(EventsOffset + 4 * index)
                    .WithFlag(0, valueProviderCallback: _ => events[event_index],
                        writeCallback: (_, value) => { events[event_index] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

            Registers.Shorts.Define(this)
                .WithFlag(0, out shortReadySample, name: "READY_SAMPLE")
                .WithFlag(1, out shortReadyStop, name: "READY_STOP")
                .WithFlag(2, out shortDownStop, name: "DOWN_STOP")
                .WithFlag(3, out shortUpStop, name: "UP_STOP")
                .WithFlag(4, out shortCrossStop, name: "CROSS_STOP")
                .WithReservedBits(5, 27);

            // INTEN exists on COMP only; INTENSET and INTENCLR serve both.
            Registers.InterruptEnable.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable = (uint)value; UpdateInterrupts(); }, name: "INTEN")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable |= (uint)value; UpdateInterrupts(); }, name: "INTENSET")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable &= ~(uint)value; UpdateInterrupts(); }, name: "INTENCLR")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.Result.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => result, name: "RESULT")
                .WithReservedBits(1, 31);

            Registers.Enable.Define(this)
                .WithValueField(0, 2, writeCallback: (_, value) => { if(value == 0) started = false; }, name: "ENABLE")
                .WithReservedBits(2, 30);

            Registers.InputSelect.Define(this)
                .WithValueField(0, 3, name: "PSEL")
                .WithReservedBits(3, 29);

            Registers.ReferenceSelect.Define(this, 0x4)
                .WithValueField(0, 4, name: "REFSEL")
                .WithReservedBits(4, 28);

            Registers.ExternalReferenceSelect.Define(this)
                .WithValueField(0, 3, name: "EXTREFSEL")
                .WithReservedBits(3, 29);

            Registers.AnalogDetect.Define(this)
                .WithValueField(0, 2, name: "ANADETECT")
                .WithReservedBits(2, 30);

            Registers.Threshold.Define(this)
                .WithValueField(0, 6, name: "THDOWN")
                .WithReservedBits(6, 2)
                .WithValueField(8, 6, name: "THUP")
                .WithReservedBits(14, 18);

            Registers.Mode.Define(this)
                .WithValueField(0, 2, name: "SP")
                .WithReservedBits(2, 6)
                .WithFlag(8, name: "MAIN")
                .WithReservedBits(9, 23);

            Registers.Hysteresis.Define(this)
                .WithFlag(0, name: "HYST")
                .WithReservedBits(1, 31);

            Registers.CurrentSource.Define(this)
                .WithValueField(0, 2, name: "ISOURCE")
                .WithReservedBits(2, 30);

            // An undocumented trim register; SystemInit copies the reference ladder calibration from FICR here (errata 12).
            Registers.ReferenceLadderTrim.Define(this)
                .WithValueField(0, 32, name: "REFLADDER_TRIM");
        }

        private void DefineTask(Registers register, string name, Action task)
        {
            register.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) task(); }, name: name)
                .WithReservedBits(1, 31);
        }

        // ====================================================================
        // Comparing
        // ====================================================================

        private void Start()
        {
            started = true;
            result = inputAbove;
            SetEvent(Event.Ready);
        }

        private void SetEvent(Event @event)
        {
            var index = (int)@event;
            events[index] = true;
            EventTriggered?.Invoke((uint)(EventsOffset + 4 * index));
            UpdateInterrupts();
            ApplyShorts(@event);
        }

        private void ApplyShorts(Event @event)
        {
            switch(@event)
            {
                case Event.Ready:
                    if(shortReadySample.Value)
                    {
                        result = inputAbove;
                    }
                    if(shortReadyStop.Value)
                    {
                        started = false;
                    }
                    break;
                case Event.Down:
                    if(shortDownStop.Value)
                    {
                        started = false;
                    }
                    break;
                case Event.Up:
                    if(shortUpStop.Value)
                    {
                        started = false;
                    }
                    break;
                case Event.Cross:
                    if(shortCrossStop.Value)
                    {
                        started = false;
                    }
                    break;
            }
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

        private const int EventCount = 4;
        private const int EventsOffset = 0x100;

        private readonly bool[] events = new bool[EventCount];

        private uint interruptEnable;
        private bool started;
        private bool inputAbove;
        private bool result;

        private IFlagRegisterField shortReadySample;
        private IFlagRegisterField shortReadyStop;
        private IFlagRegisterField shortDownStop;
        private IFlagRegisterField shortUpStop;
        private IFlagRegisterField shortCrossStop;

        private enum Event
        {
            Ready = 0,
            Down = 1,
            Up = 2,
            Cross = 3,
        }

        private enum Registers
        {
            TaskStart = 0x000,
            TaskStop = 0x004,
            TaskSample = 0x008,
            Shorts = 0x200,
            InterruptEnable = 0x300,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Result = 0x400,
            Enable = 0x500,
            InputSelect = 0x504,
            ReferenceSelect = 0x508,
            ExternalReferenceSelect = 0x50C,
            AnalogDetect = 0x520,
            Threshold = 0x530,
            Mode = 0x534,
            Hysteresis = 0x538,
            CurrentSource = 0x53C,
            ReferenceLadderTrim = 0x540,
        }
    }
}
