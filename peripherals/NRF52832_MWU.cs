// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832's memory watch unit: its region, enable and event registers read back, but no memory access is watched.

using System;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class NRF52832_MWU : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_MWU(IMachine machine) : base(machine)
        {
            IRQ = new GPIO();
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            Array.Clear(events, 0, events.Length);
            interruptEnable = 0;
            nonMaskableInterruptEnable = 0;
            regionEnable = 0;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        // The MWU interrupt; the SoftDevice may route regions to the NMI instead, which this model never raises.
        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Monitor interface
        // ====================================================================

        // Raises a watch event by hand: bits 0 to 7 are REGION[0..3].WA/RA, bits 24 to 27 are PREGION[0..1].WA/RA.
        public void TriggerEvent(int bit)
        {
            if(((1u << bit) & EventMask) == 0)
            {
                this.Log(LogLevel.Error, "Bit {0} is not an MWU event", bit);
                return;
            }
            events[bit] = true;
            EventTriggered?.Invoke((uint)(EventsOffset + 4 * bit));
            UpdateInterrupts();
        }

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            // Event n lives at 0x100 + 4n and matches bit n of INTEN, NMIEN and REGIONEN.
            for(var bit = 0; bit < 32; bit++)
            {
                if(((1u << bit) & EventMask) == 0)
                {
                    continue;
                }
                var event_bit = bit;
                RegistersCollection.DefineRegister(EventsOffset + 4 * bit)
                    .WithFlag(0, valueProviderCallback: _ => events[event_bit],
                        writeCallback: (_, value) => { events[event_bit] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

            DefineEnableTrio(Registers.InterruptEnable, () => interruptEnable, value => interruptEnable = value, "INTEN");
            DefineEnableTrio(Registers.NonMaskableInterruptEnable, () => nonMaskableInterruptEnable, value => nonMaskableInterruptEnable = value, "NMIEN");
            DefineEnableTrio(Registers.RegionEnable, () => regionEnable, value => regionEnable = value, "REGIONEN");

            // The per-subregion access status bits are cleared by writing 1.
            for(var region = 0; region < PeripheralRegionCount; region++)
            {
                RegistersCollection.DefineRegister(PeripheralRegionStatusOffset + 8 * region)
                    .WithValueField(0, 32, FieldMode.Read | FieldMode.WriteOneToClear, name: "SUBSTATWA");
                RegistersCollection.DefineRegister(PeripheralRegionStatusOffset + 8 * region + 4)
                    .WithValueField(0, 32, FieldMode.Read | FieldMode.WriteOneToClear, name: "SUBSTATRA");
            }

            for(var region = 0; region < RegionCount; region++)
            {
                RegistersCollection.DefineRegister(RegionsOffset + 16 * region)
                    .WithValueField(0, 32, name: "START");
                RegistersCollection.DefineRegister(RegionsOffset + 16 * region + 4)
                    .WithValueField(0, 32, name: "END");
            }

            // PREGION START and END are reserved and read as zero; SUBS picks the watched subregions.
            for(var region = 0; region < PeripheralRegionCount; region++)
            {
                RegistersCollection.DefineRegister(PeripheralRegionsOffset + 16 * region)
                    .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => 0, name: "START");
                RegistersCollection.DefineRegister(PeripheralRegionsOffset + 16 * region + 4)
                    .WithValueField(0, 32, FieldMode.Read, valueProviderCallback: _ => 0, name: "END");
                RegistersCollection.DefineRegister(PeripheralRegionsOffset + 16 * region + 8)
                    .WithValueField(0, 32, name: "SUBS");
            }
        }

        // Defines a plain enable register and its SET and CLR companions at the next two words.
        private void DefineEnableTrio(Registers register, Func<uint> read, Action<uint> write, string name)
        {
            var offset = (long)register;
            RegistersCollection.DefineRegister(offset)
                .WithValueField(0, 32, valueProviderCallback: _ => read(),
                    writeCallback: (_, value) => { write((uint)value & EventMask); UpdateInterrupts(); }, name: name);
            RegistersCollection.DefineRegister(offset + 4)
                .WithValueField(0, 32, valueProviderCallback: _ => read(),
                    writeCallback: (_, value) => { write(read() | ((uint)value & EventMask)); UpdateInterrupts(); }, name: name + "SET");
            RegistersCollection.DefineRegister(offset + 8)
                .WithValueField(0, 32, valueProviderCallback: _ => read(),
                    writeCallback: (_, value) => { write(read() & ~(uint)value); UpdateInterrupts(); }, name: name + "CLR");
        }

        // ====================================================================
        // Interrupts
        // ====================================================================

        private void UpdateInterrupts()
        {
            var pending = false;
            for(var bit = 0; bit < 32; bit++)
            {
                pending |= events[bit] && (interruptEnable & (1u << bit)) != 0;
            }
            IRQ.Set(pending);
        }

        private const uint EventMask = 0x0F0000FF;
        private const int EventsOffset = 0x100;
        private const int RegionCount = 4;
        private const int PeripheralRegionCount = 2;
        private const int PeripheralRegionStatusOffset = 0x400;
        private const int RegionsOffset = 0x600;
        private const int PeripheralRegionsOffset = 0x6C0;

        private readonly bool[] events = new bool[32];

        private uint interruptEnable;
        private uint nonMaskableInterruptEnable;
        private uint regionEnable;

        private enum Registers
        {
            InterruptEnable = 0x300,
            NonMaskableInterruptEnable = 0x320,
            RegionEnable = 0x510,
        }
    }
}
