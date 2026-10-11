// The nRF52832's shared CCM and AAR block: Bluetooth LE packet encryption (AES-CCM) and resolvable private address resolution.

using System;
using System.Security.Cryptography;
using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Logging;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    // CCM and AAR occupy the same 4 KB and the same registers; the ENABLE value picks which one the tasks drive.
    public sealed class NRF52832_CCM_AAR : BasicDoubleWordPeripheral, IKnownSize, INRFEventProvider
    {
        public NRF52832_CCM_AAR(IMachine machine) : base(machine)
        {
            IRQ = new GPIO();
            DefineRegisters();
        }

        public override void Reset()
        {
            base.Reset();
            Array.Clear(events, 0, events.Length);
            interruptEnable = 0;
            status = 0;
            IRQ.Unset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            // CCM TASKS_KSGEN and AAR TASKS_START share this offset.
            Registers.TaskKeystreamOrStart.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) KeystreamOrStartTask(); }, name: "TASKS_KSGEN_START")
                .WithReservedBits(1, 31);

            Registers.TaskCrypt.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, value) => { if(value) CryptTask(); }, name: "TASKS_CRYPT")
                .WithReservedBits(1, 31);

            // Every operation finishes at once, so STOP has nothing to stop.
            Registers.TaskStop.Define(this)
                .WithFlag(0, FieldMode.Write, name: "TASKS_STOP")
                .WithReservedBits(1, 31);

            for(var index = 0; index < EventCount; index++)
            {
                var event_index = index;
                RegistersCollection.DefineRegister(EventsOffset + 4 * index)
                    .WithFlag(0, valueProviderCallback: _ => events[event_index],
                        writeCallback: (_, value) => { events[event_index] = value; UpdateInterrupts(); }, name: "EVENTS")
                    .WithReservedBits(1, 31);
            }

            Registers.Shorts.Define(this)
                .WithFlag(0, out shortKeystreamToCrypt, name: "ENDKSGEN_CRYPT")
                .WithReservedBits(1, 31);

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable |= (uint)value; UpdateInterrupts(); }, name: "INTENSET")
                .WithReservedBits(EventCount, 32 - EventCount);

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, EventCount, valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable &= ~(uint)value; UpdateInterrupts(); }, name: "INTENCLR")
                .WithReservedBits(EventCount, 32 - EventCount);

            // CCM MICSTATUS and AAR STATUS: whichever ran last wrote it.
            Registers.Status.Define(this)
                .WithValueField(0, 4, FieldMode.Read, valueProviderCallback: _ => status, name: "MICSTATUS_STATUS")
                .WithReservedBits(4, 28);

            Registers.Enable.Define(this)
                .WithEnumField(0, 2, out enable, name: "ENABLE")
                .WithReservedBits(2, 30);

            // CCM MODE and AAR NIRK share this offset, and both reset to 1.
            Registers.ModeOrIrkCount.Define(this, 0x1)
                .WithValueField(0, 32, out modeOrIrkCount, name: "MODE_NIRK");

            Registers.ConfigurationOrIrkPointer.Define(this)
                .WithValueField(0, 32, out configurationOrIrkPointer, name: "CNFPTR_IRKPTR");

            Registers.InputPointer.Define(this)
                .WithValueField(0, 32, out inputPointer, name: "INPTR");

            Registers.OutputOrAddressPointer.Define(this)
                .WithValueField(0, 32, out outputOrAddressPointer, name: "OUTPTR_ADDRPTR");

            Registers.ScratchPointer.Define(this)
                .WithValueField(0, 32, name: "SCRATCHPTR");
        }

        // ====================================================================
        // Tasks
        // ====================================================================

        private void KeystreamOrStartTask()
        {
            switch(enable.Value)
            {
                case EnableMode.Ccm:
                    // The keystream is derived during CRYPT, so KSGEN only has to report completion.
                    SetEvent(Event.EndKeystreamOrEnd);
                    if(shortKeystreamToCrypt.Value)
                    {
                        CryptTask();
                    }
                    break;
                case EnableMode.Aar:
                    ResolveAddress();
                    break;
                default:
                    this.Log(LogLevel.Warning, "Ignored KSGEN/START while neither CCM nor AAR is enabled");
                    break;
            }
        }

        private void CryptTask()
        {
            if(enable.Value != EnableMode.Ccm)
            {
                this.Log(LogLevel.Warning, "Ignored CRYPT while the CCM is not enabled");
                return;
            }

            var decrypting = (modeOrIrkCount.Value & ModeDecryptBit) != 0;
            var extended_length = (modeOrIrkCount.Value & ModeExtendedLengthBit) != 0;
            if(decrypting)
            {
                Decrypt(extended_length);
            }
            else
            {
                Encrypt(extended_length);
            }
            SetEvent(Event.EndCryptOrResolved);
        }

        // ====================================================================
        // CCM: Bluetooth LE AES-CCM
        // ====================================================================

        // Packets in RAM are a header byte, a length byte, an RFU byte, then the payload.
        private void Encrypt(bool extended_length)
        {
            var header = ReadPacketHeader(inputPointer.Value, extended_length, out var payload_length);
            var payload = sysbus.ReadBytes(inputPointer.Value + PacketHeaderSize, payload_length);
            ReadConfiguration(out var key, out var nonce);

            // An empty packet is sent unencrypted and without a MIC.
            var output = payload_length == 0 ? payload : CcmEncrypt(key, nonce, header[0], payload);
            header[1] = (byte)output.Length;
            sysbus.WriteBytes(header, outputOrAddressPointer.Value);
            sysbus.WriteBytes(output, outputOrAddressPointer.Value + PacketHeaderSize);
        }

        private void Decrypt(bool extended_length)
        {
            var header = ReadPacketHeader(inputPointer.Value, extended_length, out var packet_length);
            var packet = sysbus.ReadBytes(inputPointer.Value + PacketHeaderSize, packet_length);
            ReadConfiguration(out var key, out var nonce);

            byte[] plaintext;
            var mic_passed = true;
            if(packet_length == 0)
            {
                plaintext = packet;
            }
            else if(packet_length < MicSize)
            {
                this.Log(LogLevel.Warning, "Decrypted a {0}-byte packet, too short to hold a MIC", packet_length);
                plaintext = new byte[0];
                mic_passed = false;
            }
            else
            {
                plaintext = CcmDecrypt(key, nonce, header[0], packet, out mic_passed);
            }

            header[1] = (byte)plaintext.Length;
            sysbus.WriteBytes(header, outputOrAddressPointer.Value);
            sysbus.WriteBytes(plaintext, outputOrAddressPointer.Value + PacketHeaderSize);
            status = mic_passed ? 1u : 0u;
        }

        private byte[] ReadPacketHeader(ulong address, bool extended_length, out int length)
        {
            var header = sysbus.ReadBytes(address, (int)PacketHeaderSize);
            length = extended_length ? header[1] : header[1] & DefaultLengthMask;
            return header;
        }

        // The CCM data structure: a 16-byte key, an 8-byte packet counter (39 bits used), a direction byte and an 8-byte IV.
        private void ReadConfiguration(out byte[] key, out byte[] nonce)
        {
            var configuration = sysbus.ReadBytes(configurationOrIrkPointer.Value, ConfigurationSize);
            key = new byte[KeySize];
            Array.Copy(configuration, 0, key, 0, KeySize);

            // The nonce is the 39-bit counter little-endian, the direction bit on top, then the IV.
            nonce = new byte[NonceSize];
            Array.Copy(configuration, KeySize, nonce, 0, 5);
            nonce[4] = (byte)((nonce[4] & 0x7F) | ((configuration[DirectionOffset] & 0x1) << 7));
            Array.Copy(configuration, IvOffset, nonce, 5, IvSize);
        }

        private static byte[] CcmEncrypt(byte[] key, byte[] nonce, byte header, byte[] plaintext)
        {
            using(var aes = CreateBlockCipher(key))
            {
                var tag = CcmTag(aes, nonce, header, plaintext);
                var output = new byte[plaintext.Length + MicSize];
                CcmCounterMode(aes, nonce, plaintext, output);
                var first_keystream = CounterBlockKeystream(aes, nonce, 0);
                for(var index = 0; index < MicSize; index++)
                {
                    output[plaintext.Length + index] = (byte)(tag[index] ^ first_keystream[index]);
                }
                return output;
            }
        }

        private static byte[] CcmDecrypt(byte[] key, byte[] nonce, byte header, byte[] packet, out bool mic_passed)
        {
            using(var aes = CreateBlockCipher(key))
            {
                var ciphertext = new byte[packet.Length - MicSize];
                Array.Copy(packet, ciphertext, ciphertext.Length);
                var plaintext = new byte[ciphertext.Length];
                CcmCounterMode(aes, nonce, ciphertext, plaintext);

                var tag = CcmTag(aes, nonce, header, plaintext);
                var first_keystream = CounterBlockKeystream(aes, nonce, 0);
                mic_passed = true;
                for(var index = 0; index < MicSize; index++)
                {
                    mic_passed &= packet[ciphertext.Length + index] == (byte)(tag[index] ^ first_keystream[index]);
                }
                return plaintext;
            }
        }

        // The CBC-MAC over B0, the one-byte header with NESN, SN and MD masked off, and the payload.
        private static byte[] CcmTag(ICryptoTransform aes, byte[] nonce, byte header, byte[] plaintext)
        {
            var block = new byte[BlockSize];
            block[0] = CcmAuthenticationFlags;
            Array.Copy(nonce, 0, block, 1, NonceSize);
            block[14] = (byte)(plaintext.Length >> 8);
            block[15] = (byte)plaintext.Length;
            var mac = EncryptBlock(aes, block);

            block = new byte[BlockSize];
            block[0] = 0x00;
            block[1] = 0x01;
            block[2] = (byte)(header & HeaderAuthenticationMask);
            mac = EncryptBlock(aes, Xor(mac, block));

            for(var offset = 0; offset < plaintext.Length; offset += BlockSize)
            {
                block = new byte[BlockSize];
                Array.Copy(plaintext, offset, block, 0, Math.Min(BlockSize, plaintext.Length - offset));
                mac = EncryptBlock(aes, Xor(mac, block));
            }
            return mac;
        }

        // Counter blocks from 1 upward encrypt the payload; counter block 0 masks the MIC.
        private static void CcmCounterMode(ICryptoTransform aes, byte[] nonce, byte[] input, byte[] output)
        {
            for(var offset = 0; offset < input.Length; offset += BlockSize)
            {
                var keystream = CounterBlockKeystream(aes, nonce, offset / BlockSize + 1);
                for(var index = 0; index < BlockSize && offset + index < input.Length; index++)
                {
                    output[offset + index] = (byte)(input[offset + index] ^ keystream[index]);
                }
            }
        }

        private static byte[] CounterBlockKeystream(ICryptoTransform aes, byte[] nonce, int counter)
        {
            var block = new byte[BlockSize];
            block[0] = CcmCounterFlags;
            Array.Copy(nonce, 0, block, 1, NonceSize);
            block[14] = (byte)(counter >> 8);
            block[15] = (byte)counter;
            return EncryptBlock(aes, block);
        }

        // ====================================================================
        // AAR: resolvable private address resolution
        // ====================================================================

        // ADDRPTR points at a packet; the 6-byte address follows its 3-byte header, least significant byte first.
        private void ResolveAddress()
        {
            var address = sysbus.ReadBytes(outputOrAddressPointer.Value + PacketHeaderSize, AddressSize);
            var irk_count = (int)Math.Min(modeOrIrkCount.Value & IrkCountMask, MaximumIrkCount);
            var resolved = false;

            for(var index = 0; index < irk_count && !resolved; index++)
            {
                var irk = sysbus.ReadBytes(configurationOrIrkPointer.Value + (ulong)(KeySize * index), KeySize);
                if(AddressMatchesIrk(irk, address))
                {
                    status = (uint)index;
                    resolved = true;
                }
            }

            SetEvent(resolved ? Event.EndCryptOrResolved : Event.ErrorOrNotResolved);
            SetEvent(Event.EndKeystreamOrEnd);
        }

        // ah(k, r) = e(k, 0^104 || prand) mod 2^24; the IRK is stored most significant byte first, as AES takes it.
        private static bool AddressMatchesIrk(byte[] irk, byte[] address)
        {
            var block = new byte[BlockSize];
            block[13] = address[5];
            block[14] = address[4];
            block[15] = address[3];
            byte[] hash;
            using(var aes = CreateBlockCipher(irk))
            {
                hash = EncryptBlock(aes, block);
            }
            return hash[15] == address[0] && hash[14] == address[1] && hash[13] == address[2];
        }

        // ====================================================================
        // AES block primitive
        // ====================================================================

        private static ICryptoTransform CreateBlockCipher(byte[] key)
        {
            using(var aes = Aes.Create())
            {
                aes.Mode = CipherMode.ECB;
                aes.Padding = PaddingMode.None;
                aes.Key = key;
                return aes.CreateEncryptor();
            }
        }

        private static byte[] EncryptBlock(ICryptoTransform aes, byte[] block)
        {
            var output = new byte[BlockSize];
            aes.TransformBlock(block, 0, BlockSize, output, 0);
            return output;
        }

        private static byte[] Xor(byte[] left, byte[] right)
        {
            var output = new byte[left.Length];
            for(var index = 0; index < left.Length; index++)
            {
                output[index] = (byte)(left[index] ^ right[index]);
            }
            return output;
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

        private const int EventCount = 3;
        private const int EventsOffset = 0x100;
        private const ulong ModeDecryptBit = 1UL << 0;
        private const ulong ModeExtendedLengthBit = 1UL << 24;
        private const ulong IrkCountMask = 0x1F;
        private const ulong MaximumIrkCount = 16;
        private const ulong PacketHeaderSize = 3;
        private const int DefaultLengthMask = 0x1F;
        private const int ConfigurationSize = 33;
        private const int KeySize = 16;
        private const int DirectionOffset = 24;
        private const int IvOffset = 25;
        private const int IvSize = 8;
        private const int NonceSize = 13;
        private const int MicSize = 4;
        private const int BlockSize = 16;
        private const int AddressSize = 6;
        private const byte CcmAuthenticationFlags = 0x49;
        private const byte CcmCounterFlags = 0x01;
        private const byte HeaderAuthenticationMask = 0xE3;

        private readonly bool[] events = new bool[EventCount];

        private uint interruptEnable;
        private uint status;

        private IFlagRegisterField shortKeystreamToCrypt;
        private IEnumRegisterField<EnableMode> enable;
        private IValueRegisterField modeOrIrkCount;
        private IValueRegisterField configurationOrIrkPointer;
        private IValueRegisterField inputPointer;
        private IValueRegisterField outputOrAddressPointer;

        private enum EnableMode
        {
            Disabled = 0,
            Ccm = 2,
            Aar = 3,
        }

        // Each event slot means one thing for the CCM and another for the AAR.
        private enum Event
        {
            EndKeystreamOrEnd = 0,
            EndCryptOrResolved = 1,
            ErrorOrNotResolved = 2,
        }

        private enum Registers
        {
            TaskKeystreamOrStart = 0x000,
            TaskCrypt = 0x004,
            TaskStop = 0x008,
            Shorts = 0x200,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            Status = 0x400,
            Enable = 0x500,
            ModeOrIrkCount = 0x504,
            ConfigurationOrIrkPointer = 0x508,
            InputPointer = 0x50C,
            OutputOrAddressPointer = 0x510,
            ScratchPointer = 0x514,
        }
    }
}
