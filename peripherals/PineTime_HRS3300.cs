// SPDX-License-Identifier: GPL-3.0-or-later
// The PineTime's Tianyi HRS3300 heart rate sensor: a synthetic pulse on the HRS channel and steady ambient light on the ALS channel.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.I2C;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Sensors
{
    public class PineTime_HRS3300 : II2CPeripheral
    {
        public PineTime_HRS3300(IMachine machine)
        {
            this.machine = machine;
            registers = new byte[RegisterCount];
            heartRate = DefaultHeartRate;
            ambientLight = DefaultAmbientLight;
            Reset();
        }

        // ====================================================================
        // Public interface
        // ====================================================================

        // The steady level of the pulse signal, before gain.
        public int PulseBaseline { get; set; } = 9000;

        // The height of each beat above and below the baseline, before gain; InfiniTime loses fast pulses above about 15.
        public int PulseAmplitude { get; set; } = 10;

        public void Reset()
        {
            lock(stateLock)
            {
                Array.Clear(registers, 0, registers.Length);
                registers[(int)Registers.Id] = IdValue;
                registers[(int)Registers.Enable] = 0x68;
                registers[(int)Registers.LedDriver] = 0x68;
                registers[(int)Registers.Resolution] = 0x66;
                registers[(int)Registers.Gain] = 0x10;
                registerPointer = 0;
                phaseAtChange = 0.0;
                timeOfChange = Now;
                latchedSampleIndex = ulong.MaxValue;
            }
        }

        // Sets the pulse in beats per minute; zero gives a flat signal, as with no finger on the sensor.
        public void SetHeartRate(int beatsPerMinute)
        {
            lock(stateLock)
            {
                // Keep the waveform continuous when the rate changes.
                var now = Now;
                phaseAtChange = PhaseAt(now);
                timeOfChange = now;
                heartRate = Math.Max(0, beatsPerMinute);
            }
        }

        public void SetAmbientLight(int level)
        {
            lock(stateLock)
            {
                ambientLight = Math.Max(0, Math.Min(MaximumAmbientLight, level));
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
                registerPointer = data[0];
                for(var index = 1; index < data.Length; index++)
                {
                    WriteRegister(registerPointer, data[index]);
                    registerPointer = (byte)(registerPointer + 1);
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
                    registerPointer = (byte)(registerPointer + 1);
                }
            }
            return result;
        }

        public void FinishTransmission()
        {
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void WriteRegister(byte address, byte value)
        {
            switch((Registers)address)
            {
                case Registers.Id:
                case Registers.AlsMiddle:
                case Registers.HrsMiddle:
                case Registers.HrsHigh:
                case Registers.AlsHigh:
                case Registers.AlsLow:
                case Registers.HrsLow:
                    this.Log(LogLevel.Warning, "Ignored a write of 0x{0:X2} to read-only register 0x{1:X2}", value, address);
                    return;
                default:
                    registers[address] = value;
                    return;
            }
        }

        // The data bits are spread over six registers the way the datasheet lays them out.
        private byte ReadRegister(byte address)
        {
            switch((Registers)address)
            {
                case Registers.HrsMiddle:
                    return (byte)(CurrentSample().Hrs >> 8);
                case Registers.HrsHigh:
                    return (byte)((CurrentSample().Hrs >> 4) & 0x0F);
                case Registers.HrsLow:
                {
                    var hrs = CurrentSample().Hrs;
                    return (byte)((hrs & 0x0F) | ((hrs >> 12) & 0x30));
                }
                case Registers.AlsMiddle:
                    return (byte)(CurrentSample().Als >> 3);
                case Registers.AlsHigh:
                    return (byte)((CurrentSample().Als >> 11) & 0x3F);
                case Registers.AlsLow:
                    return (byte)(CurrentSample().Als & 0x07);
                default:
                    return registers[address];
            }
        }

        // ====================================================================
        // Sampling
        // ====================================================================

        private bool Enabled => (registers[(int)Registers.Enable] & EnableBit) != 0;

        // One conversion lasts as long as the wait time chosen in the enable register, so reads within it agree.
        private double ConversionMilliseconds
        {
            get
            {
                var waitTimes = new double[] { 800, 400, 200, 100, 75, 50, 12.5, 0 };
                return Math.Max(MinimumConversionMilliseconds, waitTimes[(registers[(int)Registers.Enable] >> 4) & 0x07]);
            }
        }

        private Sample CurrentSample()
        {
            if(!Enabled)
            {
                return new Sample(0, 0);
            }

            var now = Now;
            var period = ConversionMilliseconds;
            var sampleIndex = (ulong)(now.TotalMilliseconds / period);
            if(sampleIndex != latchedSampleIndex)
            {
                latchedSampleIndex = sampleIndex;
                var sampleTime = TimeInterval.FromSeconds(sampleIndex * period / 1000.0);
                latchedSample = new Sample(PulseAt(sampleTime), ambientLight);
            }
            return latchedSample;
        }

        // A beat is a smooth rise and fall with a small second hump, like a real pulse wave.
        private int PulseAt(TimeInterval time)
        {
            var phase = 2.0 * Math.PI * PhaseAt(time);
            var shape = heartRate == 0 ? 0.0 : Math.Sin(phase) + 0.1 * Math.Sin(2.0 * phase + Math.PI / 4.0);
            var value = (PulseBaseline + PulseAmplitude * shape) * GainFactor;
            var limit = (1 << HrsBits) - 1;
            return (int)Math.Max(0, Math.Min(limit, Math.Round(value)));
        }

        // The number of beats since the last rate change, plus the beats before it.
        private double PhaseAt(TimeInterval time)
        {
            var seconds = time.TotalSeconds - timeOfChange.TotalSeconds;
            return phaseAtChange + Math.Max(0.0, seconds) * heartRate / 60.0;
        }

        // The gain field picks 1x, 2x, 4x, 8x or 64x.
        private int GainFactor
        {
            get
            {
                var gains = new int[] { 1, 2, 4, 8, 64, 64, 64, 64 };
                return gains[(registers[(int)Registers.Gain] >> 2) & 0x07];
            }
        }

        private int HrsBits => Math.Min(18, 8 + (registers[(int)Registers.Resolution] & 0x0F));

        private TimeInterval Now => machine.ElapsedVirtualTime.TimeElapsed;

        private struct Sample
        {
            public Sample(int hrs, int als)
            {
                Hrs = hrs;
                Als = als;
            }

            public int Hrs { get; }
            public int Als { get; }
        }

        // ====================================================================
        // State
        // ====================================================================

        private readonly IMachine machine;
        private readonly byte[] registers;
        private readonly object stateLock = new object();

        private byte registerPointer;
        private int heartRate;
        private int ambientLight;
        private double phaseAtChange;
        private TimeInterval timeOfChange;
        private ulong latchedSampleIndex;
        private Sample latchedSample;

        // ====================================================================
        // Constants
        // ====================================================================

        private const int RegisterCount = 256;
        private const byte IdValue = 0x21;
        private const byte EnableBit = 0x80;
        private const int DefaultHeartRate = 70;
        private const int DefaultAmbientLight = 40;
        private const int MaximumAmbientLight = (1 << 17) - 1;
        private const double MinimumConversionMilliseconds = 5;

        private enum Registers : byte
        {
            Id = 0x00,
            Enable = 0x01,
            AlsMiddle = 0x08,
            HrsMiddle = 0x09,
            HrsHigh = 0x0A,
            LedDriver = 0x0C,
            AlsHigh = 0x0D,
            AlsLow = 0x0E,
            HrsLow = 0x0F,
            Resolution = 0x16,
            Gain = 0x17,
        }
    }
}
