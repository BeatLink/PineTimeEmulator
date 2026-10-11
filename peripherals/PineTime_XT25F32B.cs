// The PineTime's 4 MB XTX XT25F32B SPI NOR flash, with its erase and program timing.

using System;
using System.Collections.Generic;
using System.IO;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Memory;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.SPI
{
    public sealed class PineTime_XT25F32B : ISPIPeripheral
    {
        public PineTime_XT25F32B(IMachine machine, MappedMemory memory)
        {
            this.machine = machine;
            this.memory = memory;
        }

        public bool DeepPoweredDown { get; private set; }

        public bool WriteEnabled { get; private set; }

        public bool Busy => machine.ElapsedVirtualTime.TimeElapsed < busyUntil;

        public void Reset()
        {
            command.Clear();
            DeepPoweredDown = false;
            WriteEnabled = false;
            busyUntil = TimeInterval.Empty;
        }

        // ====================================================================
        // Saving and loading the contents
        // ====================================================================

        /// Loads a raw image of the flash, such as a dump from a real watch.
        public void LoadImage(string path)
        {
            var image = File.ReadAllBytes(path);
            memory.WriteBytes(0, image, 0, Math.Min(image.Length, (int)memory.Size));
        }

        public void SaveImage(string path)
        {
            var image = new byte[memory.Size];
            memory.ReadBytes(0, image.Length, image, 0);
            File.WriteAllBytes(path, image);
        }

        // ====================================================================
        // SPI
        // ====================================================================

        public byte Transmit(byte data)
        {
            command.Add(data);
            var opcode = command[0];
            var position = command.Count - 1;

            if(DeepPoweredDown && opcode != ReleaseFromDeepPowerDown)
            {
                return 0xFF;
            }

            switch(opcode)
            {
                case ReadData:
                    return position < 4 ? (byte)0xFF : ReadByte(AddressPlus(position - 4));
                case FastRead:
                    return position < 5 ? (byte)0xFF : ReadByte(AddressPlus(position - 5));
                case ReadStatus:
                    return StatusLow();
                case ReadStatusHigh:
                    return statusHigh;
                case ReadSecurityRegister:
                    return securityRegister;
                case ReadJedecId:
                    return position >= 1 && position <= 3 ? JedecId[position - 1] : (byte)0xFF;
                case ReadManufacturerDeviceId:
                    return position >= 4 ? ManufacturerDeviceId[(position - 4) % 2] : (byte)0xFF;
                case ReleaseFromDeepPowerDown:
                    return position >= 4 ? JedecId[2] : (byte)0xFF;
                case ReadUniqueId:
                    return position >= 5 ? UniqueId[(position - 5) % UniqueId.Length] : (byte)0xFF;
                case PageProgram:
                    if(position >= 4)
                    {
                        pageProgramBytes.Add(data);
                    }
                    return 0xFF;
                default:
                    return 0xFF;
            }
        }

        // Most commands take effect when the chip select goes high at the end of the command.
        public void FinishTransmission()
        {
            if(command.Count == 0)
            {
                return;
            }

            var opcode = command[0];
            if(DeepPoweredDown)
            {
                if(opcode == ReleaseFromDeepPowerDown)
                {
                    DeepPoweredDown = false;
                }
                command.Clear();
                return;
            }

            switch(opcode)
            {
                case WriteEnable:
                    WriteEnabled = true;
                    break;
                case WriteDisable:
                    WriteEnabled = false;
                    break;
                case DeepPowerDown:
                    DeepPoweredDown = true;
                    break;
                case PageProgram:
                    ProgramPage();
                    break;
                case SectorErase:
                    Erase(SectorSize, SectorEraseTime);
                    break;
                case BlockErase32K:
                    Erase(Block32KSize, Block32KEraseTime);
                    break;
                case BlockErase64K:
                    Erase(Block64KSize, Block64KEraseTime);
                    break;
                case ChipErase:
                case ChipEraseAlternative:
                    if(command.Count == 1)
                    {
                        EraseRange(0, memory.Size, ChipEraseTime);
                    }
                    break;
                case WriteStatus:
                    if(WriteEnabled && !Busy && command.Count >= 2)
                    {
                        statusLowWritable = (byte)(command[1] & 0xFC);
                        if(command.Count >= 3)
                        {
                            statusHigh = command[2];
                        }
                        WriteEnabled = false;
                    }
                    break;
                case ResetEnable:
                    resetEnabled = true;
                    command.Clear();
                    return;
                case ResetDevice:
                    if(resetEnabled)
                    {
                        WriteEnabled = false;
                    }
                    break;
            }

            resetEnabled = false;
            pageProgramBytes.Clear();
            command.Clear();
        }

        // ====================================================================
        // Reading, programming and erasing
        // ====================================================================

        private long Address => ((long)command[1] << 16) | ((long)command[2] << 8) | command[3];

        private long AddressPlus(int offset)
        {
            return (Address + offset) % memory.Size;
        }

        private byte ReadByte(long address)
        {
            return memory.ReadByte(address);
        }

        // Programming only clears bits, and an address past the end of the page wraps to its start.
        private void ProgramPage()
        {
            if(!WriteEnabled || Busy || command.Count < 5)
            {
                return;
            }

            var page_start = Address & ~(PageSize - 1);
            var offset_in_page = Address & (PageSize - 1);
            foreach(var value in pageProgramBytes)
            {
                var address = page_start + offset_in_page;
                memory.WriteByte(address, (byte)(memory.ReadByte(address) & value));
                offset_in_page = (offset_in_page + 1) % PageSize;
            }

            securityRegister = 0;
            StartBusy(PageProgramTime);
        }

        private void Erase(long size, TimeInterval duration)
        {
            if(command.Count < 4)
            {
                return;
            }

            EraseRange(Address & ~(size - 1), size, duration);
        }

        private void EraseRange(long start, long size, TimeInterval duration)
        {
            if(!WriteEnabled || Busy)
            {
                return;
            }

            memory.SetRange(start, size, ErasedByte);
            securityRegister = 0;
            StartBusy(duration);
        }

        private void StartBusy(TimeInterval duration)
        {
            WriteEnabled = false;
            busyUntil = machine.ElapsedVirtualTime.TimeElapsed + duration;
        }

        private byte StatusLow()
        {
            var status = statusLowWritable;
            if(Busy)
            {
                status |= StatusWriteInProgress;
            }
            if(WriteEnabled)
            {
                status |= StatusWriteEnableLatch;
            }
            return status;
        }

        // ====================================================================
        // Commands
        // ====================================================================

        private const byte WriteStatus = 0x01;
        private const byte PageProgram = 0x02;
        private const byte ReadData = 0x03;
        private const byte WriteDisable = 0x04;
        private const byte ReadStatus = 0x05;
        private const byte WriteEnable = 0x06;
        private const byte FastRead = 0x0B;
        private const byte SectorErase = 0x20;
        private const byte ReadSecurityRegister = 0x2B;
        private const byte ReadStatusHigh = 0x35;
        private const byte ReadUniqueId = 0x4B;
        private const byte BlockErase32K = 0x52;
        private const byte ChipErase = 0x60;
        private const byte ResetEnable = 0x66;
        private const byte ReadManufacturerDeviceId = 0x90;
        private const byte ResetDevice = 0x99;
        private const byte ReadJedecId = 0x9F;
        private const byte ReleaseFromDeepPowerDown = 0xAB;
        private const byte DeepPowerDown = 0xB9;
        private const byte ChipEraseAlternative = 0xC7;
        private const byte BlockErase64K = 0xD8;

        private const byte StatusWriteInProgress = 0x01;
        private const byte StatusWriteEnableLatch = 0x02;
        private const byte ErasedByte = 0xFF;

        private const long PageSize = 256;
        private const long SectorSize = 4 * 1024;
        private const long Block32KSize = 32 * 1024;
        private const long Block64KSize = 64 * 1024;

        // Typical times from the XT25F32B datasheet.
        private static readonly TimeInterval PageProgramTime = TimeInterval.FromMicroseconds(500);
        private static readonly TimeInterval SectorEraseTime = TimeInterval.FromMilliseconds(45);
        private static readonly TimeInterval Block32KEraseTime = TimeInterval.FromMilliseconds(150);
        private static readonly TimeInterval Block64KEraseTime = TimeInterval.FromMilliseconds(250);
        private static readonly TimeInterval ChipEraseTime = TimeInterval.FromSeconds(10);

        private static readonly byte[] JedecId = { 0x0B, 0x40, 0x16 };
        private static readonly byte[] ManufacturerDeviceId = { 0x0B, 0x15 };
        private static readonly byte[] UniqueId = { 0x50, 0x49, 0x4E, 0x45, 0x54, 0x49, 0x4D, 0x45, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01 };

        private readonly IMachine machine;
        private readonly MappedMemory memory;
        private readonly List<byte> command = new List<byte>();
        private readonly List<byte> pageProgramBytes = new List<byte>();
        private TimeInterval busyUntil;
        private byte statusLowWritable;
        private byte statusHigh;
        private byte securityRegister;
        private bool resetEnabled;
    }
}
