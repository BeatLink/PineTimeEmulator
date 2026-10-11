// Test-only bus devices that record what the serial interface sends them.

using System.Collections.Generic;
using System.Linq;

using Antmicro.Renode.Peripherals.I2C;
using Antmicro.Renode.Peripherals.SPI;

namespace Antmicro.Renode.Peripherals.Mocks
{
    // ====================================================================
    // SPI
    // ====================================================================

    // Answers every byte with its bitwise inverse and remembers what it received.
    public class SerialTestSpiDevice : ISPIPeripheral
    {
        public byte Transmit(byte data)
        {
            received.Add(data);
            return (byte)~data;
        }

        public void FinishTransmission()
        {
        }

        public void Reset()
        {
            received.Clear();
        }

        public string Received => string.Join(" ", received.Select(value => value.ToString("X2")));

        private readonly List<byte> received = new List<byte>();
    }

    // ====================================================================
    // I2C
    // ====================================================================

    // A 256-byte register file: the first written byte picks a register and each access moves to the next one.
    public class SerialTestI2cDevice : II2CPeripheral
    {
        public void Write(byte[] data)
        {
            transactionLog.Add("W " + string.Join(" ", data.Select(value => value.ToString("X2"))));
            if(data.Length == 0)
            {
                return;
            }
            registerPointer = data[0];
            foreach(var value in data.Skip(1))
            {
                registers[registerPointer++] = value;
            }
        }

        public byte[] Read(int count = 1)
        {
            transactionLog.Add("R " + count);
            var result = new byte[count];
            for(var index = 0; index < count; index++)
            {
                result[index] = registers[registerPointer++];
            }
            return result;
        }

        public void FinishTransmission()
        {
            transactionLog.Add("F");
        }

        public void Reset()
        {
            transactionLog.Clear();
        }

        public void SetRegister(int register, int value)
        {
            registers[(byte)register] = (byte)value;
        }

        public int GetRegister(int register)
        {
            return registers[(byte)register];
        }

        public string TransactionLog => string.Join(" | ", transactionLog);

        public void ClearLog()
        {
            transactionLog.Clear();
        }

        private readonly byte[] registers = new byte[256];
        private readonly List<string> transactionLog = new List<string>();
        private byte registerPointer;
    }
}
