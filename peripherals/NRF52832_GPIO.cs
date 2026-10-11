// The nRF52832's GPIO port P0: each pin carries a real electrical level built from the chip's driver, outside devices, internal and board pulls, with SENSE, LATCH and DETECT.

using System;
using System.Text;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;

namespace Antmicro.Renode.Peripherals.GPIOPort
{
    public sealed class NRF52832_GPIO : BaseGPIOPort, IProvidesRegisterCollection<DoubleWordRegisterCollection>, IDoubleWordPeripheral, IKnownSize
    {
        /// Bits set in boardPullUps or boardPullDowns mark pins that have a resistor to VCC or ground on the board.
        public NRF52832_GPIO(IMachine machine, uint boardPullUps = 0, uint boardPullDowns = 0) : base(machine, NumberOfPins)
        {
            pins = new PinState[NumberOfPins];
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                pins[pin] = new PinState();
                if((boardPullUps & (1u << pin)) != 0)
                {
                    pins[pin].board_pull = true;
                }
                else if((boardPullDowns & (1u << pin)) != 0)
                {
                    pins[pin].board_pull = false;
                }
            }

            RegistersCollection = new DoubleWordRegisterCollection(this);
            DefineRegisters();
            ResetPinConfiguration();
            machine.MachineReset += _ => AnnounceAllLevels();
        }

        /// Tells every connected part the level of its pin, since parts reset after the port forget them.
        public void AnnounceAllLevels()
        {
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                foreach(var endpoint in Connections[pin].Endpoints)
                {
                    endpoint.Receiver.OnGPIO(endpoint.Number, pins[pin].level);
                }
            }
        }

        public long Size => 0x1000;

        public DoubleWordRegisterCollection RegistersCollection { get; }

        /// Fires when the port's DETECT signal rises, which wakes the chip from System OFF.
        public event Action Detect;

        /// Fires with the pin number and its new level whenever a pin's level changes.
        public event Action<int, bool> PinLevelChanged;

        public override void Reset()
        {
            RegistersCollection.Reset();
            ResetPinConfiguration();
        }

        public uint ReadDoubleWord(long offset)
        {
            return RegistersCollection.Read(offset);
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            RegistersCollection.Write(offset, value);
        }

        /// A connected device drives the pin to the given level until ReleaseExternalDrive is called.
        public override void OnGPIO(int number, bool value)
        {
            if(!CheckPinNumber(number))
            {
                return;
            }
            pins[number].externally_driven = true;
            pins[number].external_level = value;
            UpdatePin(number);
        }

        // ====================================================================
        // Monitor helpers
        // ====================================================================

        /// The pin's electrical level; a floating pin reads low.
        public bool PinLevel(int pin)
        {
            return CheckPinNumber(pin) && pins[pin].level;
        }

        /// Fits a board resistor to VCC (high) or ground (low) on the pin.
        public void SetBoardPull(int pin, bool high)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].board_pull = high;
            UpdatePin(pin);
        }

        /// Removes the board resistor from the pin.
        public void ClearBoardPull(int pin)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].board_pull = null;
            UpdatePin(pin);
        }

        /// Drives the pin from outside the chip, as a connected device would.
        public void DriveExternally(int pin, bool level)
        {
            OnGPIO(pin, level);
        }

        /// Stops driving the pin from outside, so it falls back to the chip or the pulls.
        public void ReleaseExternalDrive(int pin)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].externally_driven = false;
            UpdatePin(pin);
        }

        /// One line saying what sets the pin's level, for debugging from the monitor.
        public string DescribePin(int pin)
        {
            if(!CheckPinNumber(pin))
            {
                return "";
            }
            var state = pins[pin];
            var description = new StringBuilder();
            description.AppendFormat("P0.{0:D2} level={1} source={2}", pin, state.level ? 1 : 0, LevelSource(pin));
            description.AppendFormat(" dir={0} out={1} input={2} pull={3} drive={4} sense={5}",
                state.is_output ? "output" : "input", state.output_latch ? 1 : 0,
                state.input_disconnected ? "disconnected" : "connected", state.pull, state.drive, state.sense);
            description.AppendFormat(" board={0} gpiote={1} latch={2}",
                state.board_pull == null ? "none" : (state.board_pull.Value ? "up" : "down"), state.gpiote_control, state.latched ? 1 : 0);
            return description.ToString();
        }

        // ====================================================================
        // Control from GPIOTE
        // ====================================================================

        /// A GPIOTE channel in task mode drives the pin, overriding DIR and OUT.
        public void SetGpioteOutput(int pin, bool level)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].gpiote_control = GpioteControl.Output;
            pins[pin].gpiote_level = level;
            UpdatePin(pin);
        }

        /// A GPIOTE channel in event mode makes the pin an input, overriding DIR.
        public void SetGpioteInput(int pin)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].gpiote_control = GpioteControl.Input;
            UpdatePin(pin);
        }

        /// Hands the pin back to the GPIO registers.
        public void ReleaseGpioteControl(int pin)
        {
            if(!CheckPinNumber(pin))
            {
                return;
            }
            pins[pin].gpiote_control = GpioteControl.None;
            UpdatePin(pin);
        }

        /// The level a GPIOTE task is driving on the pin.
        public bool GpioteOutputLevel(int pin)
        {
            return CheckPinNumber(pin) && pins[pin].gpiote_level;
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            Registers.Out.Define(this)
                .WithValueField(0, 32, name: "OUT",
                    valueProviderCallback: _ => OutputBits(),
                    writeCallback: (_, value) => WriteOutputBits(_ => (uint)value));

            Registers.OutSet.Define(this)
                .WithValueField(0, 32, name: "OUTSET",
                    valueProviderCallback: _ => OutputBits(),
                    writeCallback: (_, value) => WriteOutputBits(old => old | (uint)value));

            Registers.OutClear.Define(this)
                .WithValueField(0, 32, name: "OUTCLR",
                    valueProviderCallback: _ => OutputBits(),
                    writeCallback: (_, value) => WriteOutputBits(old => old & ~(uint)value));

            Registers.In.Define(this)
                .WithValueField(0, 32, FieldMode.Read, name: "IN",
                    valueProviderCallback: _ => InputBits());

            Registers.Direction.Define(this)
                .WithValueField(0, 32, name: "DIR",
                    valueProviderCallback: _ => DirectionBits(),
                    writeCallback: (_, value) => WriteDirectionBits(_ => (uint)value));

            Registers.DirectionSet.Define(this)
                .WithValueField(0, 32, name: "DIRSET",
                    valueProviderCallback: _ => DirectionBits(),
                    writeCallback: (_, value) => WriteDirectionBits(old => old | (uint)value));

            Registers.DirectionClear.Define(this)
                .WithValueField(0, 32, name: "DIRCLR",
                    valueProviderCallback: _ => DirectionBits(),
                    writeCallback: (_, value) => WriteDirectionBits(old => old & ~(uint)value));

            Registers.Latch.Define(this)
                .WithValueField(0, 32, name: "LATCH",
                    valueProviderCallback: _ => LatchBits(),
                    writeCallback: (_, value) => ClearLatchBits((uint)value));

            Registers.DetectMode.Define(this)
                .WithFlag(0, name: "DETECTMODE",
                    valueProviderCallback: _ => latched_detect_mode,
                    writeCallback: (_, value) =>
                    {
                        latched_detect_mode = value;
                        UpdateDetect();
                    })
                .WithReservedBits(1, 31);

            Registers.PinConfiguration.DefineMany(this, NumberOfPins, (register, pin) =>
            {
                register
                    .WithFlag(0, name: "DIR",
                        valueProviderCallback: _ => pins[pin].is_output,
                        writeCallback: (_, value) => pins[pin].is_output = value)
                    .WithFlag(1, name: "INPUT",
                        valueProviderCallback: _ => pins[pin].input_disconnected,
                        writeCallback: (_, value) => pins[pin].input_disconnected = value)
                    .WithEnumField<DoubleWordRegister, PullMode>(2, 2, name: "PULL",
                        valueProviderCallback: _ => pins[pin].pull,
                        writeCallback: (_, value) => pins[pin].pull = value)
                    .WithReservedBits(4, 4)
                    .WithEnumField<DoubleWordRegister, DriveMode>(8, 3, name: "DRIVE",
                        valueProviderCallback: _ => pins[pin].drive,
                        writeCallback: (_, value) => pins[pin].drive = value)
                    .WithReservedBits(11, 5)
                    .WithEnumField<DoubleWordRegister, SenseMode>(16, 2, name: "SENSE",
                        valueProviderCallback: _ => pins[pin].sense,
                        writeCallback: (_, value) => pins[pin].sense = value)
                    .WithReservedBits(18, 14)
                    .WithWriteCallback((_, __) => UpdatePin(pin));
            }, stepInBytes: 4, resetValue: PinConfigurationResetValue);
        }

        private uint OutputBits()
        {
            return CollectBits(pin => pins[pin].output_latch);
        }

        private uint InputBits()
        {
            return CollectBits(pin => !pins[pin].input_disconnected && pins[pin].level);
        }

        private uint DirectionBits()
        {
            return CollectBits(pin => pins[pin].is_output);
        }

        private uint LatchBits()
        {
            return CollectBits(pin => pins[pin].latched);
        }

        private uint CollectBits(Func<int, bool> bit_is_set)
        {
            var bits = 0u;
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                if(bit_is_set(pin))
                {
                    bits |= 1u << pin;
                }
            }
            return bits;
        }

        private void WriteOutputBits(Func<uint, uint> new_bits_from_old)
        {
            var bits = new_bits_from_old(OutputBits());
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                pins[pin].output_latch = (bits & (1u << pin)) != 0;
            }
            UpdateAllPins();
        }

        private void WriteDirectionBits(Func<uint, uint> new_bits_from_old)
        {
            var bits = new_bits_from_old(DirectionBits());
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                pins[pin].is_output = (bits & (1u << pin)) != 0;
            }
            UpdateAllPins();
        }

        // ====================================================================
        // Pin levels
        // ====================================================================

        private void ResetPinConfiguration()
        {
            foreach(var state in pins)
            {
                state.output_latch = false;
                state.is_output = false;
                state.input_disconnected = true;
                state.pull = PullMode.Disabled;
                state.drive = DriveMode.S0S1;
                state.sense = SenseMode.Disabled;
                state.gpiote_control = GpioteControl.None;
                state.gpiote_level = false;
                state.latched = false;
            }
            latched_detect_mode = false;
            detect_signal = false;

            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                pins[pin].level = ComputeLevel(pin);
                Connections[pin].Set(pins[pin].level);
            }
            UpdateDetect();
        }

        private void UpdateAllPins()
        {
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                UpdatePin(pin);
            }
        }

        private void UpdatePin(int pin)
        {
            var new_level = ComputeLevel(pin);
            var changed = new_level != pins[pin].level;
            pins[pin].level = new_level;
            Connections[pin].Set(new_level);
            if(changed)
            {
                this.NoisyLog("P0.{0:D2} is now {1} ({2})", pin, new_level ? "high" : "low", LevelSource(pin));
                PinLevelChanged?.Invoke(pin, new_level);
            }
            UpdateDetect();
        }

        private bool ComputeLevel(int pin)
        {
            var state = pins[pin];
            if(ChipDrives(state, out var driven_level))
            {
                return driven_level;
            }
            if(state.externally_driven)
            {
                return state.external_level;
            }
            if(state.pull == PullMode.PullUp)
            {
                return true;
            }
            if(state.pull == PullMode.PullDown)
            {
                return false;
            }
            if(state.board_pull != null)
            {
                return state.board_pull.Value;
            }
            // A floating pin has no defined level; it reads low.
            return false;
        }

        private string LevelSource(int pin)
        {
            var state = pins[pin];
            if(ChipDrives(state, out var _))
            {
                return state.gpiote_control == GpioteControl.Output ? "gpiote task" : "chip output";
            }
            if(state.externally_driven)
            {
                return "external drive";
            }
            if(state.pull != PullMode.Disabled)
            {
                return "internal pull";
            }
            if(state.board_pull != null)
            {
                return "board pull";
            }
            return "floating";
        }

        // Says whether the chip's own driver is on, and at what level; open-drain drives turn it off for one of the two levels.
        private bool ChipDrives(PinState state, out bool driven_level)
        {
            if(state.gpiote_control == GpioteControl.Output)
            {
                driven_level = state.gpiote_level;
            }
            else if(state.gpiote_control == GpioteControl.None && state.is_output)
            {
                driven_level = state.output_latch;
            }
            else
            {
                driven_level = false;
                return false;
            }

            if(driven_level)
            {
                return state.drive != DriveMode.S0D1 && state.drive != DriveMode.H0D1;
            }
            return state.drive != DriveMode.D0S1 && state.drive != DriveMode.D0H1;
        }

        // ====================================================================
        // SENSE, LATCH and DETECT
        // ====================================================================

        // Sensing looks through the input buffer, so a pin with a disconnected buffer never meets its condition.
        private bool SenseConditionMet(int pin)
        {
            var state = pins[pin];
            if(state.sense == SenseMode.Disabled || state.input_disconnected)
            {
                return false;
            }
            return state.sense == SenseMode.High ? state.level : !state.level;
        }

        private void UpdateDetect()
        {
            var any_pin_detects = false;
            var any_pin_latched = false;
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                if(SenseConditionMet(pin))
                {
                    any_pin_detects = true;
                    pins[pin].latched = true;
                }
                any_pin_latched |= pins[pin].latched;
            }
            SetDetectSignal(latched_detect_mode ? any_pin_latched : any_pin_detects);
        }

        private void ClearLatchBits(uint bits)
        {
            for(var pin = 0; pin < NumberOfPins; pin++)
            {
                if((bits & (1u << pin)) != 0)
                {
                    pins[pin].latched = false;
                }
            }
            // A clear that leaves latched pins behind gives LDETECT a fresh rising edge.
            if(latched_detect_mode)
            {
                detect_signal = false;
            }
            UpdateDetect();
        }

        private void SetDetectSignal(bool value)
        {
            if(value == detect_signal)
            {
                return;
            }
            detect_signal = value;
            if(value)
            {
                this.NoisyLog("DETECT rises");
                Detect?.Invoke();
            }
        }

        // ====================================================================
        // State
        // ====================================================================

        private readonly PinState[] pins;
        private bool latched_detect_mode;
        private bool detect_signal;

        private const int NumberOfPins = 32;
        private const uint PinConfigurationResetValue = 0x2;

        private sealed class PinState
        {
            public bool output_latch;
            public bool is_output;
            public bool input_disconnected;
            public PullMode pull;
            public DriveMode drive;
            public SenseMode sense;
            public bool externally_driven;
            public bool external_level;
            public bool? board_pull;
            public GpioteControl gpiote_control;
            public bool gpiote_level;
            public bool latched;
            public bool level;
        }

        private enum GpioteControl
        {
            None,
            Input,
            Output,
        }

        private enum PullMode
        {
            Disabled = 0,
            PullDown = 1,
            PullUp = 3,
        }

        private enum DriveMode
        {
            S0S1 = 0,
            H0S1 = 1,
            S0H1 = 2,
            H0H1 = 3,
            D0S1 = 4,
            D0H1 = 5,
            S0D1 = 6,
            H0D1 = 7,
        }

        private enum SenseMode
        {
            Disabled = 0,
            High = 2,
            Low = 3,
        }

        private enum Registers
        {
            Out = 0x504,
            OutSet = 0x508,
            OutClear = 0x50C,
            In = 0x510,
            Direction = 0x514,
            DirectionSet = 0x518,
            DirectionClear = 0x51C,
            Latch = 0x520,
            DetectMode = 0x524,
            PinConfiguration = 0x700,
        }
    }
}
