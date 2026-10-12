// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's successive approximation ADC: converts the voltages set from the monitor and writes 16-bit results to RAM.

using System;
using System.Collections.Generic;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Miscellaneous;
using Antmicro.Renode.Peripherals.Timers;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Analog
{
    public sealed class NRF52832_SAADC : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_SAADC(IMachine machine, double supplyVoltage = 3.3) : base(machine)
        {
            IRQ = new GPIO();
            defaultSupplyVoltage = supplyVoltage;
            this.supplyVoltage = supplyVoltage;
            sampleTimer = new LimitTimer(machine.ClockSource, SampleTimerFrequency, this, "sample_timer", limit: 2047, workMode: WorkMode.Periodic, eventEnabled: true);
            sampleTimer.LimitReached += TakeSample;
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            sampleTimer.Reset();
            Array.Clear(events, 0, events.Length);
            Array.Clear(inputVoltages, 0, inputVoltages.Length);
            supplyVoltage = defaultSupplyVoltage;
            interruptEnable = 0;
            bufferActive = false;
            bufferPointer = 0;
            bufferMaximumCount = 0;
            resultAmount = 0;
            oversampleProgress = 0;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Monitor interface
        // ====================================================================

        // Sets the voltage on one analog input: 0 to 7 are AIN0 to AIN7, and 8 is the VDD supply.
        public void SetInputVoltage(int analogInput, double volts)
        {
            if(analogInput == SupplyInputIndex)
            {
                supplyVoltage = volts;
                return;
            }
            if(analogInput < 0 || analogInput >= AnalogInputCount)
            {
                this.Log(LogLevel.Error, "There is no analog input {0}; use 0 to 7, or 8 for VDD", analogInput);
                return;
            }
            inputVoltages[analogInput] = volts;
        }

        public double GetInputVoltage(int analogInput)
        {
            if(analogInput == SupplyInputIndex)
            {
                return supplyVoltage;
            }
            if(analogInput < 0 || analogInput >= AnalogInputCount)
            {
                this.Log(LogLevel.Error, "There is no analog input {0}; use 0 to 7, or 8 for VDD", analogInput);
                return 0;
            }
            return inputVoltages[analogInput];
        }

        // Sets the VDD supply, which is both an input and the VDD/4 reference.
        public void SetSupplyVoltage(double volts)
        {
            supplyVoltage = volts;
        }

        public double SupplyVoltage => supplyVoltage;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask(Registers.TaskStart, "TASKS_START", Start);
            DefineTask(Registers.TaskSample, "TASKS_SAMPLE", SampleTask);
            DefineTask(Registers.TaskStop, "TASKS_STOP", Stop);
            DefineTask(Registers.TaskCalibrateOffset, "TASKS_CALIBRATEOFFSET", () => SetEvent(Event.CalibrateDone));

            for(var index = 0; index < EventCount; index++)
            {
                var event_index = index;
                RegistersCollection.DefineRegister(EventsOffset + 4 * index)
                    .WithFlag(0, valueProviderCallback: _ => events[event_index],
                        writeCallback: (_, value) => { events[event_index] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

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

            // Conversions finish at once, so the converter is never busy.
            Registers.Status.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => false, name: "STATUS")
                .WithReservedBits(1, 31);

            Registers.Enable.Define(this)
                .WithFlag(0, out enabled, writeCallback: (_, value) => { if(!value) sampleTimer.Enabled = false; }, name: "ENABLE")
                .WithReservedBits(1, 31);

            for(var channel = 0; channel < ChannelCount; channel++)
            {
                var channel_offset = ChannelsOffset + ChannelStride * channel;

                RegistersCollection.DefineRegister(channel_offset + 0x0)
                    .WithValueField(0, 5, out positiveInput[channel], name: "PSELP")
                    .WithReservedBits(5, 27);

                RegistersCollection.DefineRegister(channel_offset + 0x4)
                    .WithValueField(0, 5, out negativeInput[channel], name: "PSELN")
                    .WithReservedBits(5, 27);

                RegistersCollection.DefineRegister(channel_offset + 0x8, 0x00020000)
                    .WithValueField(0, 2, name: "RESP")
                    .WithReservedBits(2, 2)
                    .WithValueField(4, 2, name: "RESN")
                    .WithReservedBits(6, 2)
                    .WithValueField(8, 3, out gain[channel], name: "GAIN")
                    .WithReservedBits(11, 1)
                    .WithFlag(12, out referenceIsQuarterSupply[channel], name: "REFSEL")
                    .WithReservedBits(13, 3)
                    .WithValueField(16, 3, name: "TACQ")
                    .WithReservedBits(19, 1)
                    .WithFlag(20, out differential[channel], name: "MODE")
                    .WithReservedBits(21, 3)
                    .WithFlag(24, out burst[channel], name: "BURST")
                    .WithReservedBits(25, 7);

                RegistersCollection.DefineRegister(channel_offset + 0xC, 0x7FFF8000)
                    .WithValueField(0, 16, out limitLow[channel], name: "LOW")
                    .WithValueField(16, 16, out limitHigh[channel], name: "HIGH");
            }

            Registers.Resolution.Define(this, 0x1)
                .WithValueField(0, 3, out resolution, name: "VAL")
                .WithReservedBits(3, 29);

            Registers.Oversample.Define(this)
                .WithValueField(0, 4, out oversample, name: "OVERSAMPLE")
                .WithReservedBits(4, 28);

            Registers.SampleRate.Define(this)
                .WithValueField(0, 11, out sampleRateCompare, name: "CC")
                .WithReservedBits(11, 1)
                .WithFlag(12, out sampleRateFromTimer, name: "MODE")
                .WithReservedBits(13, 19);

            Registers.ResultPointer.Define(this)
                .WithValueField(0, 32, out resultPointer, name: "PTR");

            Registers.ResultMaximumCount.Define(this)
                .WithValueField(0, 15, out resultMaximumCount, name: "MAXCNT")
                .WithReservedBits(15, 17);

            Registers.ResultAmount.Define(this)
                .WithValueField(0, 15, FieldMode.Read, valueProviderCallback: _ => resultAmount, name: "AMOUNT")
                .WithReservedBits(15, 17);
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

        private void Start()
        {
            if(!enabled.Value)
            {
                this.Log(LogLevel.Warning, "Ignored START while the SAADC is disabled");
                return;
            }

            // The pointer and count are latched here, so firmware may queue the next buffer straight away.
            bufferPointer = resultPointer.Value;
            bufferMaximumCount = (uint)resultMaximumCount.Value;
            resultAmount = 0;
            bufferActive = true;
            SetEvent(Event.Started);
        }

        private void SampleTask()
        {
            if(!enabled.Value)
            {
                this.Log(LogLevel.Warning, "Ignored SAMPLE while the SAADC is disabled");
                return;
            }

            TakeSample();

            // In timer mode one SAMPLE task starts continuous sampling at 16 MHz / CC.
            if(sampleRateFromTimer.Value && !sampleTimer.Enabled)
            {
                var compare = Math.Max(SampleRateCompareMinimum, sampleRateCompare.Value);
                sampleTimer.Limit = compare;
                sampleTimer.ResetValue();
                sampleTimer.Enabled = true;
            }
        }

        private void Stop()
        {
            sampleTimer.Enabled = false;
            bufferActive = false;
            oversampleProgress = 0;
            SetEvent(Event.Stopped);
        }

        // ====================================================================
        // Conversion
        // ====================================================================

        private void TakeSample()
        {
            var active_channels = new List<int>();
            for(var channel = 0; channel < ChannelCount; channel++)
            {
                if(positiveInput[channel].Value != (ulong)InputSelect.NotConnected)
                {
                    active_channels.Add(channel);
                }
            }
            if(active_channels.Count == 0)
            {
                this.Log(LogLevel.Warning, "Sampled with no channel enabled");
                return;
            }

            var samples_per_result = oversample.Value <= MaximumOversample ? 1 << (int)oversample.Value : 1;
            if(samples_per_result > 1 && active_channels.Count > 1)
            {
                this.Log(LogLevel.Warning, "Oversampling is only defined for one channel; it is ignored");
                samples_per_result = 1;
            }

            // Without burst, an oversampled result needs one SAMPLE task per sample.
            SetEvent(Event.Done);
            if(samples_per_result > 1 && !burst[active_channels[0]].Value)
            {
                oversampleProgress++;
                if(oversampleProgress < samples_per_result)
                {
                    return;
                }
            }
            oversampleProgress = 0;

            foreach(var channel in active_channels)
            {
                var result = Convert(channel);
                StoreResult(result);
                CheckLimits(channel, result);
            }
        }

        // RESULT = (V(P) - V(N)) * GAIN / REFERENCE * 2^(RESOLUTION - m), where m is 1 for differential and 0 for single-ended.
        private short Convert(int channel)
        {
            var is_differential = differential[channel].Value;
            var positive_volts = InputSelectVoltage(positiveInput[channel].Value);
            var negative_volts = is_differential ? InputSelectVoltage(negativeInput[channel].Value) : 0.0;
            var gain_factor = GainFactors[gain[channel].Value];
            var reference_volts = referenceIsQuarterSupply[channel].Value ? supplyVoltage / 4 : InternalReferenceVolts;
            var resolution_bits = ResolutionBits();
            var scale_bits = is_differential ? resolution_bits - 1 : resolution_bits;

            var ideal = (positive_volts - negative_volts) * gain_factor / reference_volts * Math.Pow(2, scale_bits);
            var code = (long)Math.Floor(ideal);

            // Single-ended inputs cannot go below ground; differential results are signed.
            var minimum = is_differential ? -(1L << scale_bits) : 0L;
            var maximum = (1L << scale_bits) - 1;
            code = Math.Max(minimum, Math.Min(maximum, code));
            return (short)code;
        }

        private double InputSelectVoltage(ulong input_select)
        {
            if(input_select == (ulong)InputSelect.NotConnected)
            {
                return 0.0;
            }
            if(input_select == (ulong)InputSelect.Supply)
            {
                return supplyVoltage;
            }
            if(input_select <= AnalogInputCount)
            {
                return inputVoltages[input_select - 1];
            }
            this.Log(LogLevel.Warning, "Input select {0} is not a valid input; it reads as 0 V", input_select);
            return 0.0;
        }

        private int ResolutionBits()
        {
            switch(resolution.Value)
            {
                case 0: return 8;
                case 1: return 10;
                case 2: return 12;
                case 3: return 14;
                default:
                    this.Log(LogLevel.Warning, "Resolution {0} is not valid; using 10 bits", resolution.Value);
                    return 10;
            }
        }

        // ====================================================================
        // EasyDMA results
        // ====================================================================

        // EasyDMA reaches only data RAM, so a pointer anywhere else (flash included) writes nothing.
        private void WriteDataRam(byte[] bytes, ulong address)
        {
            if(address < DataRamStart || address + (ulong)bytes.Length > DataRamEnd)
            {
                this.Log(LogLevel.Warning, "EasyDMA write of {0} bytes to 0x{1:X} is outside data RAM, dropped", bytes.Length, address);
                return;
            }
            sysbus.WriteBytes(bytes, address);
        }

        private const ulong DataRamStart = 0x20000000;
        private const ulong DataRamEnd = 0x20040000;

        private void StoreResult(short result)
        {
            if(!bufferActive)
            {
                this.Log(LogLevel.Noisy, "Dropped result {0} because no buffer is started", result);
                return;
            }

            var address = bufferPointer + 2UL * resultAmount;
            WriteDataRam(BitConverter.GetBytes(result), address);
            resultAmount++;
            SetEvent(Event.ResultDone);

            if(resultAmount >= bufferMaximumCount)
            {
                bufferActive = false;
                SetEvent(Event.End);
            }
        }

        private void CheckLimits(int channel, short result)
        {
            var high = unchecked((short)limitHigh[channel].Value);
            var low = unchecked((short)limitLow[channel].Value);
            if(result > high)
            {
                SetEvent(Event.Channel0LimitHigh + 2 * channel);
            }
            if(result < low)
            {
                SetEvent(Event.Channel0LimitLow + 2 * channel);
            }
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

        private const int ChannelCount = 8;
        private const int AnalogInputCount = 8;
        private const int SupplyInputIndex = 8;
        private const int EventCount = 22;
        private const int EventsOffset = 0x100;
        private const int ChannelsOffset = 0x510;
        private const int ChannelStride = 0x10;
        private const int MaximumOversample = 8;
        private const ulong SampleTimerFrequency = 16000000;
        private const ulong SampleRateCompareMinimum = 80;
        private const double InternalReferenceVolts = 0.6;

        private static readonly double[] GainFactors = { 1.0 / 6, 1.0 / 5, 1.0 / 4, 1.0 / 3, 1.0 / 2, 1.0, 2.0, 4.0 };

        private readonly double defaultSupplyVoltage;
        private readonly double[] inputVoltages = new double[AnalogInputCount];
        private readonly bool[] events = new bool[EventCount];
        private readonly LimitTimer sampleTimer;

        private readonly IValueRegisterField[] positiveInput = new IValueRegisterField[ChannelCount];
        private readonly IValueRegisterField[] negativeInput = new IValueRegisterField[ChannelCount];
        private readonly IValueRegisterField[] gain = new IValueRegisterField[ChannelCount];
        private readonly IFlagRegisterField[] referenceIsQuarterSupply = new IFlagRegisterField[ChannelCount];
        private readonly IFlagRegisterField[] differential = new IFlagRegisterField[ChannelCount];
        private readonly IFlagRegisterField[] burst = new IFlagRegisterField[ChannelCount];
        private readonly IValueRegisterField[] limitLow = new IValueRegisterField[ChannelCount];
        private readonly IValueRegisterField[] limitHigh = new IValueRegisterField[ChannelCount];

        private double supplyVoltage;
        private uint interruptEnable;
        private bool bufferActive;
        private ulong bufferPointer;
        private uint bufferMaximumCount;
        private uint resultAmount;
        private int oversampleProgress;

        private IFlagRegisterField enabled;
        private IValueRegisterField resolution;
        private IValueRegisterField oversample;
        private IValueRegisterField sampleRateCompare;
        private IFlagRegisterField sampleRateFromTimer;
        private IValueRegisterField resultPointer;
        private IValueRegisterField resultMaximumCount;

        private enum InputSelect
        {
            NotConnected = 0,
            Supply = 9,
        }

        private enum Event
        {
            Started = 0,
            End = 1,
            Done = 2,
            ResultDone = 3,
            CalibrateDone = 4,
            Stopped = 5,
            Channel0LimitHigh = 6,
            Channel0LimitLow = 7,
        }

        private enum Registers
        {
            TaskStart = 0x000,
            TaskSample = 0x004,
            TaskStop = 0x008,
            TaskCalibrateOffset = 0x00C,
            InterruptEnable = 0x300,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Status = 0x400,
            Enable = 0x500,
            Resolution = 0x5F0,
            Oversample = 0x5F4,
            SampleRate = 0x5F8,
            ResultPointer = 0x62C,
            ResultMaximumCount = 0x630,
            ResultAmount = 0x634,
        }
    }
}
