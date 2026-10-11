// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's GPIO tasks and events: eight channels that watch or drive one pin each, plus the PORT event from the port's DETECT signal.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Miscellaneous;

namespace Antmicro.Renode.Peripherals.GPIOPort
{
    public sealed class NRF52832_GPIOTE : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_GPIOTE(IMachine machine, NRF52832_GPIO port) : base(machine)
        {
            this.port = port;
            IRQ = new GPIO();
            channels = new Channel[NumberOfChannels];
            for(var index = 0; index < NumberOfChannels; index++)
            {
                channels[index] = new Channel();
            }
            port.PinLevelChanged += OnPinLevelChanged;
            port.Detect += OnDetect;
            DefineRegisters();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        /// Tells PPI the offset of each event as it happens.
        public event Action<uint> EventTriggered;

        public override void Reset()
        {
            base.Reset();
            for(var index = 0; index < NumberOfChannels; index++)
            {
                ReleasePin(index);
                channels[index] = new Channel();
            }
            port_event_pending = false;
            port_interrupt_enabled = false;
            UpdateInterrupt();
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            // Tasks take effect only when 1 is written, which is what PPI writes.
            Registers.TasksOut.DefineMany(this, NumberOfChannels, (register, index) =>
            {
                register
                    .WithFlag(0, FieldMode.Write, name: "TASKS_OUT",
                        writeCallback: (_, value) => { if(value) RunOutTask(index); })
                    .WithReservedBits(1, 31);
            });

            Registers.TasksSet.DefineMany(this, NumberOfChannels, (register, index) =>
            {
                register
                    .WithFlag(0, FieldMode.Write, name: "TASKS_SET",
                        writeCallback: (_, value) => { if(value) DriveTaskPin(index, true); })
                    .WithReservedBits(1, 31);
            });

            Registers.TasksClear.DefineMany(this, NumberOfChannels, (register, index) =>
            {
                register
                    .WithFlag(0, FieldMode.Write, name: "TASKS_CLR",
                        writeCallback: (_, value) => { if(value) DriveTaskPin(index, false); })
                    .WithReservedBits(1, 31);
            });

            Registers.EventsIn.DefineMany(this, NumberOfChannels, (register, index) =>
            {
                register
                    .WithFlag(0, name: "EVENTS_IN",
                        valueProviderCallback: _ => channels[index].event_pending,
                        writeCallback: (_, value) => channels[index].event_pending = value)
                    .WithReservedBits(1, 31)
                    .WithWriteCallback((_, __) => UpdateInterrupt());
            });

            Registers.EventsPort.Define(this)
                .WithFlag(0, name: "EVENTS_PORT",
                    valueProviderCallback: _ => port_event_pending,
                    writeCallback: (_, value) => port_event_pending = value)
                .WithReservedBits(1, 31)
                .WithWriteCallback((_, __) => UpdateInterrupt());

            Registers.InterruptEnableSet.Define(this)
                .WithFlags(0, NumberOfChannels, name: "IN",
                    valueProviderCallback: (index, _) => channels[index].interrupt_enabled,
                    writeCallback: (index, _, value) => { if(value) channels[index].interrupt_enabled = true; })
                .WithReservedBits(8, 23)
                .WithFlag(31, name: "PORT",
                    valueProviderCallback: _ => port_interrupt_enabled,
                    writeCallback: (_, value) => { if(value) port_interrupt_enabled = true; })
                .WithWriteCallback((_, __) => UpdateInterrupt());

            Registers.InterruptEnableClear.Define(this)
                .WithFlags(0, NumberOfChannels, name: "IN",
                    valueProviderCallback: (index, _) => channels[index].interrupt_enabled,
                    writeCallback: (index, _, value) => { if(value) channels[index].interrupt_enabled = false; })
                .WithReservedBits(8, 23)
                .WithFlag(31, name: "PORT",
                    valueProviderCallback: _ => port_interrupt_enabled,
                    writeCallback: (_, value) => { if(value) port_interrupt_enabled = false; })
                .WithWriteCallback((_, __) => UpdateInterrupt());

            Registers.Configuration.DefineMany(this, NumberOfChannels, (register, index) =>
            {
                register
                    .WithEnumField<DoubleWordRegister, Mode>(0, 2, name: "MODE",
                        valueProviderCallback: _ => channels[index].mode,
                        writeCallback: (_, value) => channels[index].next_mode = value)
                    .WithReservedBits(2, 6)
                    .WithValueField(8, 5, name: "PSEL",
                        valueProviderCallback: _ => (ulong)channels[index].pin,
                        writeCallback: (_, value) => channels[index].next_pin = (int)value)
                    .WithReservedBits(13, 3)
                    .WithEnumField<DoubleWordRegister, Polarity>(16, 2, name: "POLARITY",
                        valueProviderCallback: _ => channels[index].polarity,
                        writeCallback: (_, value) => channels[index].polarity = value)
                    .WithReservedBits(18, 2)
                    .WithFlag(20, name: "OUTINIT",
                        valueProviderCallback: _ => channels[index].output_initial,
                        writeCallback: (_, value) => channels[index].output_initial = value)
                    .WithReservedBits(21, 11)
                    .WithWriteCallback((_, __) => ApplyConfiguration(index));
            });
        }

        // ====================================================================
        // Channels
        // ====================================================================

        private void ApplyConfiguration(int index)
        {
            var channel = channels[index];
            ReleasePin(index);
            channel.pin = channel.next_pin;

            // The mode takes effect only after the pin settles, so taking over a pin raises no event.
            switch(channel.next_mode)
            {
            case Mode.Task:
                port.SetGpioteOutput(channel.pin, channel.output_initial);
                break;
            case Mode.Event:
                port.SetGpioteInput(channel.pin);
                channel.last_level = port.PinLevel(channel.pin);
                break;
            default:
                break;
            }
            channel.mode = channel.next_mode;
        }

        // A channel that leaves a pin gives it back to the GPIO registers, unless another channel still claims it.
        private void ReleasePin(int index)
        {
            var channel = channels[index];
            if(channel.mode == Mode.Disabled)
            {
                return;
            }
            channel.mode = Mode.Disabled;
            port.ReleaseGpioteControl(channel.pin);
            foreach(var other in channels)
            {
                if(other.mode == Mode.Task && other.pin == channel.pin)
                {
                    port.SetGpioteOutput(other.pin, port.GpioteOutputLevel(other.pin));
                }
                else if(other.mode == Mode.Event && other.pin == channel.pin)
                {
                    port.SetGpioteInput(other.pin);
                }
            }
        }

        private void RunOutTask(int index)
        {
            var channel = channels[index];
            switch(channel.polarity)
            {
            case Polarity.LoToHi:
                DriveTaskPin(index, true);
                break;
            case Polarity.HiToLo:
                DriveTaskPin(index, false);
                break;
            case Polarity.Toggle:
                DriveTaskPin(index, !port.GpioteOutputLevel(channel.pin));
                break;
            default:
                break;
            }
        }

        private void DriveTaskPin(int index, bool level)
        {
            var channel = channels[index];
            if(channel.mode != Mode.Task)
            {
                this.Log(LogLevel.Debug, "Ignored a task on channel {0}, which is not in task mode", index);
                return;
            }
            port.SetGpioteOutput(channel.pin, level);
        }

        private void OnPinLevelChanged(int pin, bool level)
        {
            for(var index = 0; index < NumberOfChannels; index++)
            {
                var channel = channels[index];
                if(channel.mode != Mode.Event || channel.pin != pin)
                {
                    continue;
                }

                var was_high = channel.last_level;
                channel.last_level = level;
                var rising = !was_high && level;
                var falling = was_high && !level;
                var fires = (channel.polarity == Polarity.LoToHi && rising)
                    || (channel.polarity == Polarity.HiToLo && falling)
                    || (channel.polarity == Polarity.Toggle && (rising || falling));
                if(fires)
                {
                    channel.event_pending = true;
                    EventTriggered?.Invoke((uint)Registers.EventsIn + (uint)index * 4);
                    UpdateInterrupt();
                }
            }
        }

        private void OnDetect()
        {
            port_event_pending = true;
            EventTriggered?.Invoke((uint)Registers.EventsPort);
            UpdateInterrupt();
        }

        private void UpdateInterrupt()
        {
            var interrupt = port_event_pending && port_interrupt_enabled;
            foreach(var channel in channels)
            {
                interrupt |= channel.event_pending && channel.interrupt_enabled;
            }
            IRQ.Set(interrupt);
        }

        // ====================================================================
        // State
        // ====================================================================

        private readonly NRF52832_GPIO port;
        private readonly Channel[] channels;
        private bool port_event_pending;
        private bool port_interrupt_enabled;

        private const int NumberOfChannels = 8;

        private sealed class Channel
        {
            public Mode mode;
            public Mode next_mode;
            public int pin;
            public int next_pin;
            public Polarity polarity;
            public bool output_initial;
            public bool last_level;
            public bool event_pending;
            public bool interrupt_enabled;
        }

        private enum Mode
        {
            Disabled = 0,
            Event = 1,
            Task = 3,
        }

        private enum Polarity
        {
            None = 0,
            LoToHi = 1,
            HiToLo = 2,
            Toggle = 3,
        }

        private enum Registers
        {
            TasksOut = 0x000,
            TasksSet = 0x030,
            TasksClear = 0x060,
            EventsIn = 0x100,
            EventsPort = 0x17C,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Configuration = 0x510,
        }
    }
}
