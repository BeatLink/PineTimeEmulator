// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's pulse width modulator: plays EasyDMA sequences in virtual time and reports each channel's duty cycle.

using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // Timing is coarse: steps shorter than a millisecond are merged, and PWMPERIODEND fires once per merged chunk.
    public sealed class NRF52832_PWM : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_PWM(IMachine machine) : base(machine)
        {
            IRQ = new GPIO();
            Output0 = new GPIO();
            Output1 = new GPIO();
            Output2 = new GPIO();
            Output3 = new GPIO();
            outputs = new[] { Output0, Output1, Output2, Output3 };
            stepTimer = new LimitTimer(machine.ClockSource, BaseClockFrequency, this, "step_timer", limit: 1, workMode: WorkMode.OneShot, eventEnabled: true);
            stepTimer.LimitReached += OnStepTimer;
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            stepTimer.Reset();
            pendingStepAction = null;
            Array.Clear(events, 0, events.Length);
            interruptEnable = 0;
            running = false;
            activeSequence = 0;
            stepIndex = 0;
            stepCount = 0;
            loopsRemaining = 0;
            waveformCounterTop = 0;
            SetDutyCycles(0, 0, 0, 0);
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        // Each output is high while its channel's duty cycle is above one half.
        public GPIO Output0 { get; }
        public GPIO Output1 { get; }
        public GPIO Output2 { get; }
        public GPIO Output3 { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Monitor interface
        // ====================================================================

        // The fraction of each period, 0 to 1, that the channel's output is high; 0 while stopped.
        public double Channel0DutyCycle => dutyCycles[0];
        public double Channel1DutyCycle => dutyCycles[1];
        public double Channel2DutyCycle => dutyCycles[2];
        public double Channel3DutyCycle => dutyCycles[3];

        public double GetDutyCycle(int channel)
        {
            if(channel < 0 || channel >= ChannelCount)
            {
                this.Log(LogLevel.Error, "There is no PWM channel {0}", channel);
                return 0;
            }
            return dutyCycles[channel];
        }

        // The GPIO pin a channel drives, or -1 when it is disconnected.
        public int GetOutputPin(int channel)
        {
            if(channel < 0 || channel >= ChannelCount)
            {
                this.Log(LogLevel.Error, "There is no PWM channel {0}", channel);
                return -1;
            }
            return outputDisconnected[channel].Value ? -1 : (int)outputPin[channel].Value;
        }

        public bool IsRunning => running;

        // The current PWM period in seconds.
        public double PeriodSeconds => (double)PeriodTicks() / BaseClockFrequency;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask(Registers.TaskStop, "TASKS_STOP", Stop);
            DefineTask(Registers.TaskSequenceStart0, "TASKS_SEQSTART[0]", () => StartSequenceTask(0));
            DefineTask(Registers.TaskSequenceStart1, "TASKS_SEQSTART[1]", () => StartSequenceTask(1));
            DefineTask(Registers.TaskNextStep, "TASKS_NEXTSTEP", NextStep);

            // Event n lives at 0x100 + 4n and matches bit n of INTEN; event 0 does not exist.
            for(var index = FirstEvent; index < EventCount; index++)
            {
                var event_index = index;
                RegistersCollection.DefineRegister(EventsOffset + 4 * index)
                    .WithFlag(0, valueProviderCallback: _ => events[event_index],
                        writeCallback: (_, value) => { events[event_index] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

            Registers.Shorts.Define(this)
                .WithFlag(0, out shortSequenceEnd0Stop, name: "SEQEND0_STOP")
                .WithFlag(1, out shortSequenceEnd1Stop, name: "SEQEND1_STOP")
                .WithFlag(2, out shortLoopsDoneSequenceStart0, name: "LOOPSDONE_SEQSTART0")
                .WithFlag(3, out shortLoopsDoneSequenceStart1, name: "LOOPSDONE_SEQSTART1")
                .WithFlag(4, out shortLoopsDoneStop, name: "LOOPSDONE_STOP")
                .WithReservedBits(5, 27);

            Registers.InterruptEnable.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable = (uint)value & EventMask; UpdateInterrupts(); }, name: "INTEN");

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable |= (uint)value & EventMask; UpdateInterrupts(); }, name: "INTENSET");

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable &= ~(uint)value; UpdateInterrupts(); }, name: "INTENCLR");

            Registers.Enable.Define(this)
                .WithFlag(0, out enabled, writeCallback: (_, value) => { if(!value) Halt(); }, name: "ENABLE")
                .WithReservedBits(1, 31);

            Registers.Mode.Define(this)
                .WithFlag(0, out upAndDown, name: "UPDOWN")
                .WithReservedBits(1, 31);

            Registers.CounterTop.Define(this, 0x3FF)
                .WithValueField(0, 15, out counterTop, name: "COUNTERTOP")
                .WithReservedBits(15, 17);

            Registers.Prescaler.Define(this)
                .WithValueField(0, 3, out prescaler, name: "PRESCALER")
                .WithReservedBits(3, 29);

            Registers.Decoder.Define(this)
                .WithEnumField(0, 2, out decoderLoad, name: "LOAD")
                .WithReservedBits(2, 6)
                .WithFlag(8, out decoderNextStep, name: "MODE")
                .WithReservedBits(9, 23);

            Registers.Loop.Define(this)
                .WithValueField(0, 16, out loopCount, name: "CNT")
                .WithReservedBits(16, 16);

            for(var sequence = 0; sequence < SequenceCount; sequence++)
            {
                var sequence_offset = SequencesOffset + SequenceStride * sequence;

                RegistersCollection.DefineRegister(sequence_offset + 0x0)
                    .WithValueField(0, 32, out sequencePointer[sequence], name: "PTR");

                RegistersCollection.DefineRegister(sequence_offset + 0x4)
                    .WithValueField(0, 15, out sequenceCount[sequence], name: "CNT")
                    .WithReservedBits(15, 17);

                RegistersCollection.DefineRegister(sequence_offset + 0x8, 0x1)
                    .WithValueField(0, 24, out sequenceRefresh[sequence], name: "REFRESH")
                    .WithReservedBits(24, 8);

                RegistersCollection.DefineRegister(sequence_offset + 0xC)
                    .WithValueField(0, 24, out sequenceEndDelay[sequence], name: "ENDDELAY")
                    .WithReservedBits(24, 8);
            }

            for(var channel = 0; channel < ChannelCount; channel++)
            {
                RegistersCollection.DefineRegister(OutputSelectOffset + 4 * channel, 0xFFFFFFFF)
                    .WithValueField(0, 5, out outputPin[channel], name: "PIN")
                    .WithReservedBits(5, 26)
                    .WithFlag(31, out outputDisconnected[channel], name: "CONNECT");
            }
        }

        private void DefineTask(Registers register, string name, Action task)
        {
            register.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) task(); }, name: name)
                .WithReservedBits(1, 31);
        }

        // ====================================================================
        // Tasks
        // ====================================================================

        private void StartSequenceTask(int sequence)
        {
            if(!enabled.Value)
            {
                this.Log(LogLevel.Warning, "Ignored SEQSTART[{0}] while the PWM is disabled", sequence);
                return;
            }

            // The loop counter is loaded each time playback is started.
            running = true;
            loopsRemaining = (uint)loopCount.Value;
            BeginSequence(sequence);
        }

        private void Stop()
        {
            Halt();
            SetEvent(Event.Stopped);
        }

        private void NextStep()
        {
            if(!running || !decoderNextStep.Value || stepIndex >= stepCount)
            {
                return;
            }

            stepIndex++;
            if(stepIndex < stepCount)
            {
                ApplyStep(stepIndex);
                if(stepIndex == stepCount - 1)
                {
                    SetEvent(SequenceEndEvent(activeSequence));
                }
                return;
            }

            // Past the last value, the end delay runs and then the sequence finishes.
            var delay = sequenceEndDelay[activeSequence].Value * PeriodTicks();
            if(delay == 0)
            {
                FinishSequence();
            }
            else
            {
                ScheduleStep(delay, FinishSequence);
            }
        }

        // Stops pulse generation without raising an event.
        private void Halt()
        {
            stepTimer.Enabled = false;
            pendingStepAction = null;
            running = false;
            SetDutyCycles(0, 0, 0, 0);
        }

        // ====================================================================
        // Sequence playback
        // ====================================================================

        private void BeginSequence(int sequence)
        {
            activeSequence = sequence;
            stepIndex = 0;
            stepCount = (int)(sequenceCount[sequence].Value / (ulong)ValuesPerStep());
            SetEvent(sequence == 0 ? Event.SequenceStarted0 : Event.SequenceStarted1);
            if(!running)
            {
                return;
            }

            if(stepCount == 0)
            {
                this.Log(LogLevel.Warning, "Sequence {0} holds no complete step; it lasts one period", sequence);
                SetEvent(SequenceEndEvent(sequence));
                if(running)
                {
                    ScheduleStep(PeriodTicks(), FinishSequence);
                }
                return;
            }

            if(decoderNextStep.Value)
            {
                ApplyStep(0);
                if(stepCount == 1)
                {
                    SetEvent(SequenceEndEvent(sequence));
                }
                return;
            }

            PlaySteps();
        }

        // Applies the next step and schedules the one after; SEQEND fires when the last value is applied.
        private void PlaySteps()
        {
            if(!running)
            {
                return;
            }

            ApplyStep(stepIndex);
            var ticks = StepTicks();

            if(stepIndex == stepCount - 1)
            {
                SetEvent(SequenceEndEvent(activeSequence));
                if(running)
                {
                    ScheduleStep(ticks + sequenceEndDelay[activeSequence].Value * PeriodTicks(), FinishSequence);
                }
                return;
            }

            stepIndex++;
            while(ticks < MinimumChunkTicks && stepIndex < stepCount - 1)
            {
                ticks += StepTicks();
                stepIndex++;
            }
            ScheduleStep(ticks, PlaySteps);
        }

        // Runs after a sequence's last value and end delay have played: moves on to the next sequence or loop.
        private void FinishSequence()
        {
            if(!running || loopsRemaining == 0)
            {
                // Without looping, the last value keeps playing until STOP.
                return;
            }

            if(activeSequence == 0)
            {
                BeginSequence(1);
                return;
            }

            loopsRemaining--;
            if(loopsRemaining > 0)
            {
                BeginSequence(0);
                return;
            }
            SetEvent(Event.LoopsDone);
        }

        private void ApplyStep(int step)
        {
            var values_per_step = ValuesPerStep();
            var address = sequencePointer[activeSequence].Value + (ulong)(2 * step * values_per_step);
            var values = new ushort[values_per_step];
            for(var index = 0; index < values_per_step; index++)
            {
                values[index] = sysbus.ReadWord(address + (ulong)(2 * index));
            }

            var top = (ulong)counterTop.Value;
            switch(decoderLoad.Value)
            {
                case DecoderLoad.Common:
                    var common = DutyCycle(values[0], top);
                    SetDutyCycles(common, common, common, common);
                    break;
                case DecoderLoad.Grouped:
                    SetDutyCycles(DutyCycle(values[0], top), DutyCycle(values[0], top), DutyCycle(values[1], top), DutyCycle(values[1], top));
                    break;
                case DecoderLoad.Individual:
                    SetDutyCycles(DutyCycle(values[0], top), DutyCycle(values[1], top), DutyCycle(values[2], top), DutyCycle(values[3], top));
                    break;
                case DecoderLoad.Waveform:
                    // The fourth value replaces COUNTERTOP for this step, and channel 3 has no output.
                    waveformCounterTop = (ulong)(values[3] & CompareMask);
                    top = waveformCounterTop;
                    SetDutyCycles(DutyCycle(values[0], top), DutyCycle(values[1], top), DutyCycle(values[2], top), 0);
                    break;
            }
        }

        // With bit 15 set the output starts high and falls at the compare value; clear, it starts low and rises.
        private static double DutyCycle(ushort value, ulong top)
        {
            if(top == 0)
            {
                return 0;
            }
            var compare = Math.Min((ulong)(value & CompareMask), top);
            var fraction = (double)compare / top;
            return (value & PolarityBit) != 0 ? fraction : 1 - fraction;
        }

        private void SetDutyCycles(double channel0, double channel1, double channel2, double channel3)
        {
            dutyCycles[0] = channel0;
            dutyCycles[1] = channel1;
            dutyCycles[2] = channel2;
            dutyCycles[3] = channel3;
            for(var channel = 0; channel < ChannelCount; channel++)
            {
                outputs[channel].Set(dutyCycles[channel] > 0.5);
            }
        }

        // ====================================================================
        // Timing
        // ====================================================================

        // One period in 16 MHz ticks: COUNTERTOP prescaled, twice over when counting up and down.
        private ulong PeriodTicks()
        {
            var top = decoderLoad.Value == DecoderLoad.Waveform && waveformCounterTop != 0 ? waveformCounterTop : (ulong)counterTop.Value;
            var ticks = Math.Max(1UL, top) << (int)prescaler.Value;
            return upAndDown.Value ? 2 * ticks : ticks;
        }

        // Each value plays for REFRESH + 1 periods.
        private ulong StepTicks()
        {
            return (sequenceRefresh[activeSequence].Value + 1) * PeriodTicks();
        }

        private int ValuesPerStep()
        {
            switch(decoderLoad.Value)
            {
                case DecoderLoad.Common: return 1;
                case DecoderLoad.Grouped: return 2;
                default: return 4;
            }
        }

        private void ScheduleStep(ulong ticks, Action action)
        {
            stepTimer.Enabled = false;
            stepTimer.Limit = Math.Max(1UL, ticks);
            stepTimer.ResetValue();
            pendingStepAction = action;
            stepTimer.Enabled = true;
        }

        private void OnStepTimer()
        {
            stepTimer.Enabled = false;
            var action = pendingStepAction;
            pendingStepAction = null;
            if(!running)
            {
                return;
            }
            SetEvent(Event.PeriodEnd);
            action?.Invoke();
        }

        // ====================================================================
        // Events, shorts and interrupts
        // ====================================================================

        private static Event SequenceEndEvent(int sequence)
        {
            return sequence == 0 ? Event.SequenceEnd0 : Event.SequenceEnd1;
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
                case Event.SequenceEnd0:
                    if(shortSequenceEnd0Stop.Value)
                    {
                        Stop();
                    }
                    break;
                case Event.SequenceEnd1:
                    if(shortSequenceEnd1Stop.Value)
                    {
                        Stop();
                    }
                    break;
                case Event.LoopsDone:
                    if(shortLoopsDoneStop.Value)
                    {
                        Stop();
                    }
                    else if(shortLoopsDoneSequenceStart0.Value)
                    {
                        StartSequenceTask(0);
                    }
                    else if(shortLoopsDoneSequenceStart1.Value)
                    {
                        StartSequenceTask(1);
                    }
                    break;
            }
        }

        private void UpdateInterrupts()
        {
            var pending = false;
            for(var index = FirstEvent; index < EventCount; index++)
            {
                pending |= events[index] && (interruptEnable & (1u << index)) != 0;
            }
            IRQ.Set(pending);
        }

        private const int ChannelCount = 4;
        private const int SequenceCount = 2;
        private const int FirstEvent = 1;
        private const int EventCount = 8;
        private const uint EventMask = 0xFE;
        private const int EventsOffset = 0x100;
        private const int SequencesOffset = 0x520;
        private const int SequenceStride = 0x20;
        private const int OutputSelectOffset = 0x560;
        private const int CompareMask = 0x7FFF;
        private const int PolarityBit = 0x8000;
        private const ulong BaseClockFrequency = 16000000;
        private const ulong MinimumChunkTicks = BaseClockFrequency / 1000;

        private readonly GPIO[] outputs;
        private readonly double[] dutyCycles = new double[ChannelCount];
        private readonly bool[] events = new bool[EventCount];
        private readonly LimitTimer stepTimer;

        private readonly IValueRegisterField[] sequencePointer = new IValueRegisterField[SequenceCount];
        private readonly IValueRegisterField[] sequenceCount = new IValueRegisterField[SequenceCount];
        private readonly IValueRegisterField[] sequenceRefresh = new IValueRegisterField[SequenceCount];
        private readonly IValueRegisterField[] sequenceEndDelay = new IValueRegisterField[SequenceCount];
        private readonly IValueRegisterField[] outputPin = new IValueRegisterField[ChannelCount];
        private readonly IFlagRegisterField[] outputDisconnected = new IFlagRegisterField[ChannelCount];

        private Action pendingStepAction;
        private uint interruptEnable;
        private bool running;
        private int activeSequence;
        private int stepIndex;
        private int stepCount;
        private uint loopsRemaining;
        private ulong waveformCounterTop;

        private IFlagRegisterField shortSequenceEnd0Stop;
        private IFlagRegisterField shortSequenceEnd1Stop;
        private IFlagRegisterField shortLoopsDoneSequenceStart0;
        private IFlagRegisterField shortLoopsDoneSequenceStart1;
        private IFlagRegisterField shortLoopsDoneStop;
        private IFlagRegisterField enabled;
        private IFlagRegisterField upAndDown;
        private IValueRegisterField counterTop;
        private IValueRegisterField prescaler;
        private IEnumRegisterField<DecoderLoad> decoderLoad;
        private IFlagRegisterField decoderNextStep;
        private IValueRegisterField loopCount;

        private enum DecoderLoad
        {
            Common = 0,
            Grouped = 1,
            Individual = 2,
            Waveform = 3,
        }

        private enum Event
        {
            Stopped = 1,
            SequenceStarted0 = 2,
            SequenceStarted1 = 3,
            SequenceEnd0 = 4,
            SequenceEnd1 = 5,
            PeriodEnd = 6,
            LoopsDone = 7,
        }

        private enum Registers
        {
            TaskStop = 0x004,
            TaskSequenceStart0 = 0x008,
            TaskSequenceStart1 = 0x00C,
            TaskNextStep = 0x010,
            Shorts = 0x200,
            InterruptEnable = 0x300,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Enable = 0x500,
            Mode = 0x504,
            CounterTop = 0x508,
            Prescaler = 0x50C,
            Decoder = 0x510,
            Loop = 0x514,
        }
    }
}
