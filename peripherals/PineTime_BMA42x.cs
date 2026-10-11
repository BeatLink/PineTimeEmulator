// SPDX-License-Identifier: GPL-3.0-or-later
// The PineTime's Bosch BMA421 or BMA425 accelerometer: config file upload, 12-bit acceleration, a step counter and two interrupt pins.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.I2C;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Sensors
{
    public class PineTime_BMA42x : II2CPeripheral
    {
        public PineTime_BMA42x(IMachine machine, string model = "BMA421")
        {
            this.machine = machine;
            switch(model.Trim().ToUpperInvariant())
            {
                case "BMA421":
                    chipId = Bma421ChipId;
                    break;
                case "BMA425":
                    chipId = Bma425ChipId;
                    break;
                default:
                    throw new ArgumentException($"Unknown model '{model}'; use BMA421 or BMA425");
            }

            INT1 = new GPIO();
            INT2 = new GPIO();
            registers = new byte[RegisterCount];
            configMemory = new byte[ConfigMemorySize];
            featureMemory = new byte[FeatureMemorySize];
            accelerationMilliG = new int[] { 0, 0, 1000 };
            temperatureCelsius = 23.0;
            Reset();
        }

        // ====================================================================
        // Public interface
        // ====================================================================

        public GPIO INT1 { get; }
        public GPIO INT2 { get; }

        public void Reset()
        {
            lock(stateLock)
            {
                SoftReset();
            }
        }

        // Sets the acceleration the sensor feels, in milli-g along its own axes.
        public void SetAcceleration(int x, int y, int z)
        {
            lock(stateLock)
            {
                accelerationMilliG[0] = x;
                accelerationMilliG[1] = y;
                accelerationMilliG[2] = z;
            }
        }

        public void SetSteps(uint steps)
        {
            lock(stateLock)
            {
                var before = stepCount;
                stepCount = steps;
                CheckStepWatermark(before);
            }
        }

        public void AddSteps(uint steps)
        {
            lock(stateLock)
            {
                var before = stepCount;
                stepCount += steps;
                CheckStepWatermark(before);
            }
        }

        public uint Steps
        {
            get
            {
                lock(stateLock)
                {
                    return stepCount;
                }
            }
        }

        public void SetTemperature(double celsius)
        {
            lock(stateLock)
            {
                temperatureCelsius = celsius;
            }
        }

        // Raises feature interrupts by their INT_STATUS_0 bits, such as 0x02 for the step counter or 0x40 for any motion.
        public void TriggerFeatureInterrupt(byte statusBits)
        {
            lock(stateLock)
            {
                RaiseFeatureInterrupt(statusBits);
            }
        }

        // ====================================================================
        // I2C
        // ====================================================================

        public void Write(byte[] data)
        {
            if(data.Length == 0)
            {
                return;
            }

            lock(stateLock)
            {
                SelectRegister(data[0]);
                for(var index = 1; index < data.Length; index++)
                {
                    WriteRegister(registerPointer, data[index]);
                    AdvancePointer();
                }
            }
        }

        public byte[] Read(int count = 1)
        {
            var result = new byte[count];
            lock(stateLock)
            {
                for(var index = 0; index < count; index++)
                {
                    result[index] = ReadRegister(registerPointer);
                    AdvancePointer();
                }
            }
            return result;
        }

        public void FinishTransmission()
        {
        }

        private void SelectRegister(byte address)
        {
            registerPointer = address;
            if(address == (byte)Registers.FeaturesIn)
            {
                featureOffset = FeatureWordAddress * 2;
            }
        }

        // The pointer moves on after each byte, except on the feature port, which steps through memory instead.
        private void AdvancePointer()
        {
            if(registerPointer == (byte)Registers.FeaturesIn)
            {
                featureOffset++;
            }
            else if(registerPointer != (byte)Registers.FifoData)
            {
                registerPointer = (byte)((registerPointer + 1) & 0x7F);
            }
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private byte ReadRegister(byte address)
        {
            switch((Registers)address)
            {
                case Registers.ChipId:
                    return chipId;
                case Registers.Status:
                    return (byte)(CommandReady | (AccelerometerEnabled ? DataReady : 0));
                case Registers.AccelerationXLow:
                case Registers.AccelerationXHigh:
                case Registers.AccelerationYLow:
                case Registers.AccelerationYHigh:
                case Registers.AccelerationZLow:
                case Registers.AccelerationZHigh:
                    return AccelerationByte(address - (int)Registers.AccelerationXLow);
                case Registers.SensorTime0:
                    return (byte)SensorTimeTicks;
                case Registers.SensorTime1:
                    return (byte)(SensorTimeTicks >> 8);
                case Registers.SensorTime2:
                    return (byte)(SensorTimeTicks >> 16);
                case Registers.Event:
                {
                    var value = registers[address];
                    registers[address] = 0;
                    return value;
                }
                case Registers.InterruptStatus0:
                case Registers.InterruptStatus1:
                {
                    var value = registers[address];
                    registers[address] = 0;
                    UpdateInterruptPins();
                    return value;
                }
                case Registers.StepCount0:
                    return (byte)stepCount;
                case Registers.StepCount1:
                    return (byte)(stepCount >> 8);
                case Registers.StepCount2:
                    return (byte)(stepCount >> 16);
                case Registers.StepCount3:
                    return (byte)(stepCount >> 24);
                case Registers.Temperature:
                    return (byte)(sbyte)Math.Max(-127, Math.Min(127, Math.Round(temperatureCelsius - 23.0)));
                case Registers.FifoLength0:
                case Registers.FifoLength1:
                case Registers.FifoData:
                    return 0;
                case Registers.FeaturesIn:
                    return FeaturePortRead();
                default:
                    return registers[address];
            }
        }

        private void WriteRegister(byte address, byte value)
        {
            switch((Registers)address)
            {
                case Registers.Command:
                    RunCommand(value);
                    return;
                case Registers.InitControl:
                    registers[address] = value;
                    if(value == 0)
                    {
                        initialised = false;
                        registers[(int)Registers.InternalStatus] = InternalStatusNotInitialised;
                    }
                    else if((value & 0x01) != 0)
                    {
                        FinishConfigLoad();
                    }
                    return;
                case Registers.FeaturesIn:
                    FeaturePortWrite(value);
                    return;
                case Registers.ChipId:
                case Registers.Status:
                case Registers.InternalStatus:
                    this.Log(LogLevel.Warning, "Ignored a write of 0x{0:X2} to read-only register 0x{1:X2}", value, address);
                    return;
                case Registers.InterruptLatch:
                case Registers.Int1IoControl:
                case Registers.Int2IoControl:
                case Registers.Int1Map:
                case Registers.Int2Map:
                case Registers.InterruptMapData:
                    registers[address] = value;
                    UpdateInterruptPins();
                    return;
                default:
                    if(address >= (int)Registers.AccelerationXLow && address <= (int)Registers.InternalStatus)
                    {
                        this.Log(LogLevel.Warning, "Ignored a write of 0x{0:X2} to read-only register 0x{1:X2}", value, address);
                        return;
                    }
                    registers[address] = value;
                    return;
            }
        }

        private void RunCommand(byte command)
        {
            switch(command)
            {
                case SoftResetCommand:
                    this.Log(LogLevel.Debug, "Soft reset");
                    SoftReset();
                    return;
                case FifoFlushCommand:
                    return;
                default:
                    this.Log(LogLevel.Warning, "Ignored unknown command 0x{0:X2}", command);
                    return;
            }
        }

        private void SoftReset()
        {
            Array.Clear(registers, 0, registers.Length);
            Array.Clear(configMemory, 0, configMemory.Length);
            Array.Clear(featureMemory, 0, featureMemory.Length);
            registerPointer = 0;
            featureOffset = 0;
            initialised = false;
            configBytesLoaded = 0;
            stepCount = 0;

            registers[(int)Registers.Event] = PowerOnDetected;
            registers[(int)Registers.InternalStatus] = InternalStatusNotInitialised;
            registers[(int)Registers.AccelerationConfig] = 0xA8;
            registers[(int)Registers.AccelerationRange] = 0x01;
            registers[(int)Registers.FifoConfig1] = 0x10;
            registers[(int)Registers.InitControl] = 0x90;
            registers[(int)Registers.PowerConfig] = 0x03;

            INT1.Set(false);
            INT2.Set(false);
        }

        // ====================================================================
        // Config memory and feature area
        // ====================================================================

        // The word address set through registers 0x5B (low four bits) and 0x5C (the rest).
        private int FeatureWordAddress => (registers[(int)Registers.FeatureAddressHigh] << 4) | (registers[(int)Registers.FeatureAddressLow] & 0x0F);

        private void FeaturePortWrite(byte value)
        {
            if(initialised)
            {
                if(featureOffset < featureMemory.Length)
                {
                    featureMemory[featureOffset] = value;
                    if(featureOffset == StepCounterControlOffset)
                    {
                        ApplyStepCounterControl();
                    }
                }
                return;
            }

            if(featureOffset < configMemory.Length)
            {
                configMemory[featureOffset] = value;
                configBytesLoaded++;
            }
            else
            {
                this.Log(LogLevel.Warning, "Ignored a config byte beyond the end of config memory at 0x{0:X}", featureOffset);
            }
        }

        private byte FeaturePortRead()
        {
            var memory = initialised ? featureMemory : configMemory;
            return featureOffset < memory.Length ? memory[featureOffset] : (byte)0;
        }

        // Starting the loaded config makes the feature area appear at word address zero.
        private void FinishConfigLoad()
        {
            if(configBytesLoaded == 0)
            {
                this.Log(LogLevel.Warning, "Config load started with no config file written");
                registers[(int)Registers.InternalStatus] = InternalStatusInitialisationError;
                return;
            }

            initialised = true;
            registers[(int)Registers.InternalStatus] = InternalStatusInitialised;
            registers[(int)Registers.FeatureAddressLow] = 0;
            registers[(int)Registers.FeatureAddressHigh] = 0;
            Array.Clear(featureMemory, 0, featureMemory.Length);
            this.Log(LogLevel.Debug, "Config file of {0} bytes accepted", configBytesLoaded);
        }

        // The step counter's reset bit clears the count and then clears itself.
        private void ApplyStepCounterControl()
        {
            if((featureMemory[StepCounterControlOffset] & StepCounterResetBit) != 0)
            {
                stepCount = 0;
                featureMemory[StepCounterControlOffset] &= unchecked((byte)~StepCounterResetBit);
            }
        }

        // ====================================================================
        // Measurements
        // ====================================================================

        private bool AccelerometerEnabled => (registers[(int)Registers.PowerControl] & AccelerometerEnableBit) != 0;

        private byte AccelerationByte(int index)
        {
            var countsPerG = 1024 >> (registers[(int)Registers.AccelerationRange] & 0x03);
            var counts = (int)Math.Round(accelerationMilliG[index / 2] * countsPerG / 1000.0);
            counts = Math.Max(-2048, Math.Min(2047, counts));
            var leftJustified = (ushort)(short)(counts << 4);
            return (byte)((index % 2) == 0 ? leftJustified : leftJustified >> 8);
        }

        // The 24-bit sensor time counts in steps of 39.0625 microseconds.
        private uint SensorTimeTicks => (uint)(machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds / 39.0625) & 0xFFFFFF;

        // ====================================================================
        // Interrupts
        // ====================================================================

        private void CheckStepWatermark(uint before)
        {
            var watermark = ((featureMemory[StepCounterControlOffset - 1] | (featureMemory[StepCounterControlOffset] << 8)) & 0x3FF) * 20u;
            if(watermark != 0 && stepCount / watermark != before / watermark)
            {
                RaiseFeatureInterrupt(StepCounterInterrupt);
            }
        }

        private void RaiseFeatureInterrupt(byte statusBits)
        {
            registers[(int)Registers.InterruptStatus0] |= statusBits;
            var latched = (registers[(int)Registers.InterruptLatch] & 0x01) != 0;
            UpdateInterruptPins();
            if(!latched)
            {
                // Without latching, the status and the pins only hold for a moment.
                machine.ScheduleAction(TimeInterval.FromMilliseconds(UnlatchedPulseMilliseconds), _ =>
                {
                    lock(stateLock)
                    {
                        registers[(int)Registers.InterruptStatus0] &= unchecked((byte)~statusBits);
                        UpdateInterruptPins();
                    }
                }, "BMA42x interrupt pulse end");
            }
        }

        private void UpdateInterruptPins()
        {
            var status = registers[(int)Registers.InterruptStatus0];
            SetPin(INT1, (status & registers[(int)Registers.Int1Map]) != 0, registers[(int)Registers.Int1IoControl]);
            SetPin(INT2, (status & registers[(int)Registers.Int2Map]) != 0, registers[(int)Registers.Int2IoControl]);
        }

        // Drives a pin at its configured active level, or leaves it low while its output is off.
        private static void SetPin(GPIO pin, bool active, byte ioControl)
        {
            var outputEnabled = (ioControl & 0x08) != 0;
            var activeHigh = (ioControl & 0x02) != 0;
            pin.Set(outputEnabled && (active == activeHigh));
        }

        // ====================================================================
        // State
        // ====================================================================

        private readonly IMachine machine;
        private readonly byte chipId;
        private readonly byte[] registers;
        private readonly byte[] configMemory;
        private readonly byte[] featureMemory;
        private readonly int[] accelerationMilliG;
        private readonly object stateLock = new object();

        private byte registerPointer;
        private int featureOffset;
        private bool initialised;
        private int configBytesLoaded;
        private uint stepCount;
        private double temperatureCelsius;

        // ====================================================================
        // Constants
        // ====================================================================

        private const byte Bma421ChipId = 0x11;
        private const byte Bma425ChipId = 0x13;

        private const int RegisterCount = 128;
        private const int ConfigMemorySize = 8192;
        private const int FeatureMemorySize = 256;

        private const byte SoftResetCommand = 0xB6;
        private const byte FifoFlushCommand = 0xB0;

        private const byte CommandReady = 0x10;
        private const byte DataReady = 0x80;
        private const byte PowerOnDetected = 0x01;
        private const byte AccelerometerEnableBit = 0x04;

        private const byte InternalStatusNotInitialised = 0x00;
        private const byte InternalStatusInitialised = 0x01;
        private const byte InternalStatusInitialisationError = 0x02;

        private const int StepCounterControlOffset = 0x3B;
        private const byte StepCounterResetBit = 0x04;
        private const byte StepCounterInterrupt = 0x02;
        private const ulong UnlatchedPulseMilliseconds = 1;

        private enum Registers : byte
        {
            ChipId = 0x00,
            Error = 0x02,
            Status = 0x03,
            AccelerationXLow = 0x12,
            AccelerationXHigh = 0x13,
            AccelerationYLow = 0x14,
            AccelerationYHigh = 0x15,
            AccelerationZLow = 0x16,
            AccelerationZHigh = 0x17,
            SensorTime0 = 0x18,
            SensorTime1 = 0x19,
            SensorTime2 = 0x1A,
            Event = 0x1B,
            InterruptStatus0 = 0x1C,
            InterruptStatus1 = 0x1D,
            StepCount0 = 0x1E,
            StepCount1 = 0x1F,
            StepCount2 = 0x20,
            StepCount3 = 0x21,
            Temperature = 0x22,
            FifoLength0 = 0x24,
            FifoLength1 = 0x25,
            FifoData = 0x26,
            ActivityType = 0x27,
            InternalStatus = 0x2A,
            AccelerationConfig = 0x40,
            AccelerationRange = 0x41,
            FifoConfig1 = 0x49,
            Int1IoControl = 0x53,
            Int2IoControl = 0x54,
            InterruptLatch = 0x55,
            Int1Map = 0x56,
            Int2Map = 0x57,
            InterruptMapData = 0x58,
            InitControl = 0x59,
            FeatureAddressLow = 0x5B,
            FeatureAddressHigh = 0x5C,
            FeaturesIn = 0x5E,
            InternalError = 0x5F,
            NvConfig = 0x70,
            PowerConfig = 0x7C,
            PowerControl = 0x7D,
            Command = 0x7E,
        }
    }
}
