// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's flash controller: it is always ready, erases pages of flash and UICR to 0xFF, and guards writes.

using System.Linq;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.Memory;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class NRF52832_NVMC : BasicDoubleWordPeripheral, IKnownSize
    {
        public NRF52832_NVMC(IMachine machine, MappedMemory flash, MappedMemory uicr) : base(machine)
        {
            this.flash = flash;
            this.uicr = uicr;
            DefineRegisters();
        }

        public long Size => 0x1000;

        // The CPU writes flash only while WEN is set, and then can only clear bits; any other write is ignored.
        // Data accesses to flash go through the bus so these hooks see them; instruction fetches stay direct.
        public void GuardFlashWrites()
        {
            if(!flashWritesGuarded)
            {
                flashWritesGuarded = true;
                machine.MachineReset += _ => RouteFlashThroughBus();
                machine.SystemBus.SetHookBeforePeripheralWrite<byte>(flash, (value, offset) => (byte)GuardedWrite(flash.ReadByte(offset), value));
                machine.SystemBus.SetHookBeforePeripheralWrite<ushort>(flash, (value, offset) => (ushort)GuardedWrite(flash.ReadWord(offset), value));
                machine.SystemBus.SetHookBeforePeripheralWrite<uint>(flash, (value, offset) => GuardedWrite(flash.ReadDoubleWord(offset), value));
            }
            RouteFlashThroughBus();
        }

        // A CPU reset forgets which pages go through the bus, so this runs again after every one.
        private void RouteFlashThroughBus()
        {
            var cpu = machine.SystemBus.GetCPUs().OfType<TranslationCPU>().First();
            for(ulong page = 0; page < (ulong)flash.Size; page += PageSize)
            {
                cpu.SetPageAccessViaIo(page);
                cpu.FlushTlbPage(page);
            }
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            Registers.Ready.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => true, name: "READY")
                .WithReservedBits(1, 31);

            Registers.ReadyNext.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ => true, name: "READYNEXT")
                .WithReservedBits(1, 31);

            Registers.Config.Define(this)
                .WithEnumField(0, 2, out writeMode, name: "WEN")
                .WithReservedBits(2, 30);

            Registers.ErasePage.Define(this)
                .WithValueField(0, 32, FieldMode.Write, writeCallback: (_, address) => ErasePage(address), name: "ERASEPAGE");

            Registers.EraseAll.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, erase) => { if(erase) EraseAll(); }, name: "ERASEALL")
                .WithReservedBits(1, 31);

            Registers.ErasePartial.Define(this)
                .WithValueField(0, 32, FieldMode.Write, writeCallback: (_, address) => ErasePage(address), name: "ERASEPCR0");

            Registers.EraseUicr.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, erase) => { if(erase) EraseUicr(); }, name: "ERASEUICR")
                .WithReservedBits(1, 31);

            Registers.ErasePagePartialConfig.Define(this, 0xA)
                .WithValueField(0, 7, name: "DURATION")
                .WithReservedBits(7, 25);

            Registers.InstructionCacheConfig.Define(this)
                .WithFlag(0, name: "CACHEEN")
                .WithReservedBits(1, 7)
                .WithFlag(8, name: "CACHEPROFEN")
                .WithReservedBits(9, 23);

            Registers.InstructionCacheHits.Define(this)
                .WithValueField(0, 32, name: "HITS");

            Registers.InstructionCacheMisses.Define(this)
                .WithValueField(0, 32, name: "MISSES");
        }

        // ====================================================================
        // Writing
        // ====================================================================

        // Loading images from the monitor is not a CPU write, so it always lands.
        private uint GuardedWrite(uint stored, uint written)
        {
            if(!machine.SystemBus.TryGetCurrentCPU(out var _))
            {
                return written;
            }
            if(writeMode.Value != WriteMode.WriteEnabled)
            {
                this.Log(LogLevel.Noisy, "Ignored a flash write of 0x{0:X} without write enabled", written);
                return stored;
            }
            return stored & written;
        }

        // ====================================================================
        // Erasing
        // ====================================================================

        private void ErasePage(ulong address)
        {
            if(writeMode.Value != WriteMode.EraseEnabled)
            {
                this.Log(LogLevel.Warning, "Ignored an erase of page 0x{0:X} without erase enabled", address);
                return;
            }

            var page_start = (long)address & ~((long)PageSize - 1);
            if(page_start >= flash.Size)
            {
                this.Log(LogLevel.Warning, "Ignored an erase of page 0x{0:X} outside flash", address);
                return;
            }

            flash.SetRange(page_start, (long)PageSize, ErasedByte);
        }

        private void EraseAll()
        {
            if(writeMode.Value != WriteMode.EraseEnabled)
            {
                return;
            }

            flash.SetRange(0, flash.Size, ErasedByte);
            EraseUicr();
        }

        private void EraseUicr()
        {
            if(writeMode.Value != WriteMode.EraseEnabled)
            {
                return;
            }

            uicr.SetRange(0, uicr.Size, ErasedByte);
        }

        private const ulong PageSize = 0x1000;
        private const byte ErasedByte = 0xFF;

        private readonly MappedMemory flash;
        private bool flashWritesGuarded;
        private readonly MappedMemory uicr;
        private IEnumRegisterField<WriteMode> writeMode;

        private enum WriteMode
        {
            ReadOnly = 0,
            WriteEnabled = 1,
            EraseEnabled = 2,
        }

        private enum Registers
        {
            Ready = 0x400,
            ReadyNext = 0x408,
            Config = 0x504,
            ErasePage = 0x508,
            EraseAll = 0x50C,
            ErasePartial = 0x510,
            EraseUicr = 0x514,
            ErasePagePartialConfig = 0x51C,
            InstructionCacheConfig = 0x540,
            InstructionCacheHits = 0x548,
            InstructionCacheMisses = 0x54C,
        }
    }
}
