// SPDX-License-Identifier: GPL-3.0-or-later
// Test-only device for the radio tests: it writes down every radio event with the exact virtual time it happened.

using System.Collections.Generic;

using Antmicro.Renode.Core;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.Miscellaneous;

namespace Antmicro.Renode.Peripherals.Mocks
{
    public class RadioEventRecorder : IDoubleWordPeripheral, IKnownSize
    {
        public RadioEventRecorder(IMachine machine, INRFEventProvider radio)
        {
            radio.EventTriggered += offset => events.Add($"{EventName(offset)}@{machine.ClockSource.CurrentValue.Ticks / 1000.0:0.###}");
        }

        public long Size => 0x4;

        // Events in the order they happened, each with its time in microseconds.
        public string Events => string.Join(" ", events);

        public void Clear()
        {
            events.Clear();
        }

        public uint ReadDoubleWord(long offset)
        {
            return 0;
        }

        public void WriteDoubleWord(long offset, uint value)
        {
        }

        public void Reset()
        {
            events.Clear();
        }

        private static string EventName(uint offset)
        {
            switch(offset)
            {
            case 0x100:
                return "READY";
            case 0x104:
                return "ADDRESS";
            case 0x108:
                return "PAYLOAD";
            case 0x10C:
                return "END";
            case 0x110:
                return "DISABLED";
            case 0x114:
                return "DEVMATCH";
            case 0x118:
                return "DEVMISS";
            case 0x11C:
                return "RSSIEND";
            case 0x128:
                return "BCMATCH";
            case 0x130:
                return "CRCOK";
            case 0x134:
                return "CRCERROR";
            case 0x154:
                return "TXREADY";
            case 0x158:
                return "RXREADY";
            case 0x16C:
                return "PHYEND";
            default:
                return $"0x{offset:X}";
            }
        }

        private readonly List<string> events = new List<string>();
    }
}
