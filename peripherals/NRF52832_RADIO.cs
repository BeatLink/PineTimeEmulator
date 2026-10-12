// SPDX-License-Identifier: GPL-3.0-or-later
// The nRF52832 radio: ramp-up, packets sent and received bit by bit in virtual time, shortcuts, TIFS turnaround, address and device matching, on Renode's wireless medium.

using System;
using System.Collections.Generic;
using System.Linq;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Exceptions;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.CPU;
using Antmicro.Renode.Peripherals.Miscellaneous;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Wireless
{
    public sealed class NRF52832_RADIO : BasicDoubleWordPeripheral, IRadio, IKnownSize, INRFEventProvider
    {
        // With nrf52840Registers the radio also has the nRF52840's TXREADY, RXREADY and PHYEND events and their shortcuts, for Zephyr's controller on the bridge.
        public NRF52832_RADIO(IMachine machine, bool nrf52840Registers = false) : base(machine)
        {
            hasNrf52840Registers = nrf52840Registers;
            eventMask = nrf52840Registers ? Nrf52832EventMask | Nrf52840ExtraEventMask : Nrf52832EventMask;
            shortcutMask = nrf52840Registers ? Nrf52832ShortcutMask | Nrf52840ExtraShortcutMask : Nrf52832ShortcutMask;
            alarmHandler = OnAlarm;
            IRQ = new GPIO();
            DefineRegisters();
            Reset();
        }

        public long Size => 0x1000;

        public GPIO IRQ { get; }

        public event Action<IRadio, byte[]> FrameSent;

        public event Action<uint> EventTriggered;

        // The Bluetooth LE channel index the FREQUENCY register points at; other frequencies get numbers no BLE channel uses.
        public int Channel
        {
            get
            {
                var megahertz = (frequencyMap.Value ? LowBandStart : NormalBandStart) + (int)frequency.Value;
                switch(megahertz)
                {
                case 2402:
                    return 37;
                case 2426:
                    return 38;
                case 2480:
                    return 39;
                }
                if(megahertz % 2 == 0 && megahertz >= 2404 && megahertz <= 2424)
                {
                    return (megahertz - 2404) / 2;
                }
                if(megahertz % 2 == 0 && megahertz >= 2428 && megahertz <= 2478)
                {
                    return (megahertz - 2428) / 2 + 11;
                }
                return NonBluetoothChannelBase + megahertz;
            }

            set
            {
                throw new RecoverableException("The channel follows the FREQUENCY register and cannot be set directly");
            }
        }

        public override void Reset()
        {
            base.Reset();
            scheduled.Clear();
            undocumentedRegisters.Clear();
            state = RadioState.Disabled;
            powered = true;
            shortcuts = 0;
            interruptEnabled = 0;
            eventsPending = 0;
            currentPacket = null;
            receivePointer = 0;
            bitCounterStart = null;
            crcMatched = false;
            receivedAddressIndex = 0;
            receivedCrc = 0;
            deviceAddressIndex = 0;
            lastAirEnd = 0;
            turnaroundAfterEnd = false;
            lastTransmission = null;
            whiteningSeed = 0x40;
            DisarmAlarm();
            IRQ.Unset();
        }

        // ====================================================================
        // Air timing
        // ====================================================================

        private ulong Now()
        {
            return machine.ClockSource.CurrentValue.Ticks;
        }

        private RadioMode Mode => (RadioMode)mode.Value;

        private ulong BitNanoseconds
        {
            get
            {
                switch(Mode)
                {
                case RadioMode.Nrf2Mbit:
                case RadioMode.Ble2Mbit:
                    return 500;
                case RadioMode.Nrf250Kbit:
                    return 4000;
                default:
                    return 1000;
                }
            }
        }

        private bool IsTwoMegabit => Mode == RadioMode.Nrf2Mbit || Mode == RadioMode.Ble2Mbit;

        // Bluetooth LE 2M always sends a two-byte preamble; otherwise PLEN chooses.
        private ulong PreambleBits => (Mode == RadioMode.Ble2Mbit || preambleLength.Value) ? 16UL : 8UL;

        private int AddressBytes => (int)Math.Min(4, Math.Max(1, baseAddressLength.Value)) + 1;

        private ulong HeaderAirBits => s0Length.Value * 8 + lengthFieldBits.Value + s1Bits.Value;

        private ulong CrcBits => crcLength.Value * 8;

        // Fast ramp-up takes 40 us, the default 140 us, for either direction.
        private ulong RampUpNanoseconds => fastRampUp.Value ? 40000UL : 140000UL;

        // Measured pipeline delays, as Apache NimBLE's nRF52 driver lists them: the air lags the TX events and the RX events lag the air.
        private ulong TransmitChainNanoseconds => IsTwoMegabit ? 3000UL : 4000UL;

        private ulong ReceiveChainNanoseconds => IsTwoMegabit ? 2000UL : 6000UL;

        private ulong TransmitDisableNanoseconds => IsTwoMegabit ? 4000UL : 6000UL;

        // ====================================================================
        // Virtual time scheduling
        // ====================================================================

        // Queues work for an exact moment in virtual time; one alarm on the machine clock wakes the model for the earliest.
        private void Schedule(ulong time, Work kind, Action<ulong> run)
        {
            scheduled.Add(new ScheduledWork(time, nextOrder++, kind, run));
            if(!processing)
            {
                ArmAlarm();
            }
        }

        private void Cancel(Work kinds)
        {
            scheduled.RemoveAll(work => (work.Kind & kinds) != 0);
            if(!processing)
            {
                ArmAlarm();
            }
        }

        // Runs every piece of work due by the given moment in time order, each at its own timestamp.
        private void RunDueWork(ulong now)
        {
            if(processing)
            {
                return;
            }
            processing = true;
            try
            {
                while(true)
                {
                    ScheduledWork next = null;
                    foreach(var work in scheduled)
                    {
                        if(work.Time <= now && (next == null || work.Time < next.Time || (work.Time == next.Time && work.Order < next.Order)))
                        {
                            next = work;
                        }
                    }
                    if(next == null)
                    {
                        break;
                    }
                    scheduled.Remove(next);
                    next.Run(next.Time);
                }
            }
            finally
            {
                processing = false;
            }
            ArmAlarm();
        }

        // Brings virtual time up to the instruction touching the registers and catches up on due work, so polling sees events exactly when they happen.
        private ulong CatchUp()
        {
            if(!processing)
            {
                SynchronizeCpuTime();
            }
            var now = Now();
            RunDueWork(now);
            return now;
        }

        private void OnAlarm()
        {
            RunDueWork(Math.Max(Now(), alarmTime));
        }

        private void ArmAlarm()
        {
            if(scheduled.Count == 0)
            {
                DisarmAlarm();
                return;
            }
            var earliest = scheduled.Min(work => work.Time);
            var now = Now();
            alarmTime = earliest;
            var delay = earliest > now ? earliest - now : 1;
            machine.ClockSource.ExchangeClockEntryWith(alarmHandler,
                entry => entry.With(period: delay, value: 0, enabled: true),
                () => new ClockEntry(delay, NanosecondsPerSecond, alarmHandler, this, "alarm", enabled: true, direction: Direction.Ascending, workMode: WorkMode.OneShot));
            RequestReturnOnCurrentCpu();
        }

        private void DisarmAlarm()
        {
            machine.ClockSource.ExchangeClockEntryWith(alarmHandler,
                entry => entry.With(enabled: false),
                () => new ClockEntry(1, NanosecondsPerSecond, alarmHandler, this, "alarm", enabled: false, direction: Direction.Ascending, workMode: WorkMode.OneShot));
        }

        private void SynchronizeCpuTime()
        {
            if(machine.SystemBus.TryGetCurrentCPU(out var cpu) && cpu.OnPossessedThread)
            {
                cpu.SyncTime();
            }
        }

        // Makes the running CPU stop at the new alarm instead of the end of its time slice.
        private void RequestReturnOnCurrentCpu()
        {
            if(EmulationManager.Instance.CurrentEmulation.TryGetExecutionContext(out var _, out var cpu))
            {
                (cpu as IControllableCPU)?.RequestReturn();
            }
        }

        // ====================================================================
        // Tasks and state changes
        // ====================================================================

        private void RunTask(Action<ulong> task)
        {
            var now = CatchUp();
            if(!powered)
            {
                return;
            }
            var wasProcessing = processing;
            processing = true;
            try
            {
                task(now);
            }
            finally
            {
                processing = wasProcessing;
            }
            RunDueWork(now);
        }

        // TXEN and RXEN ramp up from DISABLED or from either idle state.
        private bool CanRampUp => state == RadioState.Disabled || state == RadioState.TxIdle || state == RadioState.RxIdle;

        private void TransmitEnable(ulong time)
        {
            if(!CanRampUp)
            {
                this.Log(LogLevel.Debug, "Ignored TXEN in state {0}", state);
                return;
            }
            var readyTime = time + RampUpNanoseconds;
            // After END_DISABLE and DISABLED_TXEN the first bit goes on air exactly TIFS after the last packet ended.
            if(turnaroundAfterEnd)
            {
                var onAir = lastAirEnd + interFrameSpacing.Value * 1000;
                readyTime = Math.Max(readyTime, onAir > TransmitChainNanoseconds ? onAir - TransmitChainNanoseconds : 0);
            }
            turnaroundAfterEnd = false;
            state = RadioState.TxRampUp;
            Schedule(readyTime, Work.RampUp, ReadyReached);
        }

        private void ReceiveEnable(ulong time)
        {
            turnaroundAfterEnd = false;
            if(!CanRampUp)
            {
                this.Log(LogLevel.Debug, "Ignored RXEN in state {0}", state);
                return;
            }
            state = RadioState.RxRampUp;
            Schedule(time + RampUpNanoseconds, Work.RampUp, ReadyReached);
        }

        private void ReadyReached(ulong time)
        {
            var transmitting = state == RadioState.TxRampUp;
            state = transmitting ? RadioState.TxIdle : RadioState.RxIdle;
            RaiseEvent(RadioEvent.Ready);
            if(hasNrf52840Registers)
            {
                RaiseEvent(transmitting ? RadioEvent.TxReady : RadioEvent.RxReady);
            }
            if(HasShortcut(Shortcut.ReadyStart) || HasShortcut(transmitting ? Shortcut.TxReadyStart : Shortcut.RxReadyStart))
            {
                Start(time);
            }
        }

        private void Start(ulong time)
        {
            switch(state)
            {
            case RadioState.TxIdle:
                Transmit(time);
                break;
            case RadioState.RxIdle:
                // The receiver listens for one packet into the buffer PACKETPTR names now.
                state = RadioState.Rx;
                receivePointer = packetPointer.Value;
                break;
            default:
                this.Log(LogLevel.Debug, "Ignored START in state {0}", state);
                break;
            }
        }

        private void Stop(ulong time)
        {
            if(state != RadioState.Tx && state != RadioState.Rx)
            {
                return;
            }
            Cancel(Work.Packet | Work.BitCounter);
            currentPacket = null;
            bitCounterStart = null;
            state = state == RadioState.Tx ? RadioState.TxIdle : RadioState.RxIdle;
        }

        private void Disable(ulong time)
        {
            DisableFrom(time, false);
        }

        private void DisableFrom(ulong time, bool afterEnd)
        {
            if(state == RadioState.TxDisable || state == RadioState.RxDisable)
            {
                return;
            }
            Cancel(Work.RampUp | Work.Packet | Work.BitCounter);
            currentPacket = null;
            bitCounterStart = null;
            turnaroundAfterEnd = afterEnd;
            switch(state)
            {
            case RadioState.TxRampUp:
            case RadioState.TxIdle:
            case RadioState.Tx:
                state = RadioState.TxDisable;
                Schedule(time + TransmitDisableNanoseconds, Work.Disabling, DisabledReached);
                break;
            case RadioState.RxRampUp:
            case RadioState.RxIdle:
            case RadioState.Rx:
                // The receiver turns off at once.
                state = RadioState.RxDisable;
                DisabledReached(time);
                break;
            default:
                // DISABLE while already disabled still raises DISABLED.
                DisabledReached(time);
                break;
            }
        }

        private void DisabledReached(ulong time)
        {
            state = RadioState.Disabled;
            RaiseEvent(RadioEvent.Disabled);
            if(HasShortcut(Shortcut.DisabledRssiStop))
            {
                RssiStop(time);
            }
            if(HasShortcut(Shortcut.DisabledTxEnable))
            {
                TransmitEnable(time);
            }
            else if(HasShortcut(Shortcut.DisabledRxEnable))
            {
                ReceiveEnable(time);
            }
            turnaroundAfterEnd = false;
        }

        // One RSSI sample takes a quarter of a microsecond.
        private void RssiStart(ulong time)
        {
            Cancel(Work.Rssi);
            Schedule(time + RssiSampleNanoseconds, Work.Rssi, _ => RaiseEvent(RadioEvent.RssiEnd));
        }

        private void RssiStop(ulong time)
        {
            Cancel(Work.Rssi);
        }

        // The bit counter counts bits on air from the moment it starts, normally the ADDRESS event.
        private void BitCounterStart(ulong time)
        {
            bitCounterStart = time;
            ScheduleBitCounterMatch(time);
        }

        private void BitCounterStop(ulong time)
        {
            bitCounterStart = null;
            Cancel(Work.BitCounter);
        }

        private void ScheduleBitCounterMatch(ulong now)
        {
            Cancel(Work.BitCounter);
            if(!bitCounterStart.HasValue)
            {
                return;
            }
            var matchTime = bitCounterStart.Value + bitCountCompare.Value * currentBitNanoseconds;
            if(matchTime < now || (currentPacket != null && matchTime > currentPacket.EndTime))
            {
                return;
            }
            Schedule(matchTime, Work.BitCounter, _ => RaiseEvent(RadioEvent.BitCounterMatch));
        }

        private bool HasShortcut(Shortcut shortcut)
        {
            return (shortcuts & (uint)shortcut) != 0;
        }

        // ====================================================================
        // Transmitting
        // ====================================================================

        // START in TXIDLE reads the packet from RAM and puts it on the medium at once; its events follow at their bit times.
        private void Transmit(ulong time)
        {
            var header = ReadRamHeader(packetPointer.Value);
            var payloadLength = (int)Math.Min(header.Length + staticLength.Value, maximumLength.Value);
            var payload = sysbus.ReadBytes(packetPointer.Value + (ulong)RamHeaderBytes, payloadLength);
            var airHeader = PackAirHeader(header);
            var address = LogicalAddress((int)transmitAddress.Value);

            var frameLength = address.Length + Math.Max(airHeader.Length, RamHeaderBytes) + Math.Max(payloadLength, (int)maximumLength.Value);
            var frame = new byte[frameLength];
            Array.Copy(address, 0, frame, 0, address.Length);
            Array.Copy(airHeader, 0, frame, address.Length, airHeader.Length);
            Array.Copy(payload, 0, frame, address.Length + airHeader.Length, payloadLength);

            var bitTime = BitNanoseconds;
            var addressTime = time + (PreambleBits + (ulong)address.Length * 8) * bitTime;
            var packet = new PacketTimeline(true, addressTime, bitTime, HeaderAirBits, payloadLength, CrcBits);
            currentPacket = packet;
            currentBitNanoseconds = bitTime;
            state = RadioState.Tx;

            lastTransmission = new Transmission
            {
                Frame = frame,
                AirStart = time + TransmitChainNanoseconds,
                BitNanoseconds = bitTime,
                PreambleBits = PreambleBits,
                Mode = Mode,
                Crc = ComputeCrc(address, airHeader, payload),
                Whitening = WhiteningKey,
            };

            Schedule(packet.AddressTime, Work.Packet, AddressReached);
            Schedule(packet.PayloadTime, Work.Packet, _ => RaiseEvent(RadioEvent.Payload));
            Schedule(packet.EndTime, Work.Packet, EndReached);

            this.Log(LogLevel.Debug, "Transmitting at {0} ns on channel {1}: {2}", time, Channel, BitConverter.ToString(frame, 0, Math.Min(frame.Length, address.Length + airHeader.Length + payloadLength)));
            FrameSent?.Invoke(this, frame);
        }

        // ====================================================================
        // Receiving
        // ====================================================================

        // The medium hands over a frame as its sender starts it; the receiver decides when the access address has come in.
        // Other radio models, such as Renode's nRF52840 one, raise ADDRESS as they start, so their frames count as having their address already.
        public void ReceiveFrame(byte[] frame, IRadio sender)
        {
            var now = Now();
            var arrival = new Arrival { Frame = frame, Channel = sender?.Channel ?? Channel };
            var addressEnd = now;
            var known = (sender as NRF52832_RADIO)?.lastTransmission;
            if(known != null && known.Frame.SequenceEqual(frame))
            {
                arrival.Sender = known;
                var airStart = known.AirStart;
                // The medium delivers at the sender's next sync point, so the true start lies a little off from now.
                if(Math.Max(airStart, now) - Math.Min(airStart, now) > MaximumDeliveryDelayNanoseconds)
                {
                    airStart = now;
                }
                addressEnd = airStart + (known.PreambleBits + (ulong)AddressBytes * 8) * known.BitNanoseconds;
            }
            Schedule(Math.Max(addressEnd, now), Work.Arrival, time => AddressArrives(arrival, time));
            if(!processing)
            {
                RunDueWork(now);
            }
        }

        // The receiver takes a frame only if it is listening, on its channel, at its rate, when the whole access address has arrived.
        private void AddressArrives(Arrival arrival, ulong time)
        {
            TryReceive(arrival, time);
        }

        private bool TryReceive(Arrival arrival, ulong time)
        {
            if(state != RadioState.Rx || currentPacket != null)
            {
                this.Log(LogLevel.Debug, "Not listening for a frame on channel {0} at {1} ns, state {2}", arrival.Channel, time, state);
                return false;
            }
            if(arrival.Channel != Channel)
            {
                this.Log(LogLevel.Debug, "Lost a frame on channel {0}: now tuned to {1}", arrival.Channel, Channel);
                return false;
            }
            if(arrival.Sender != null && arrival.Sender.Mode != Mode)
            {
                this.Log(LogLevel.Debug, "Lost a frame sent in mode {0}: receiving in mode {1}", arrival.Sender.Mode, Mode);
                return false;
            }
            var frame = arrival.Frame;
            var addressLength = AddressBytes;
            var matched = -1;
            for(var index = 0; index < LogicalAddressCount && matched < 0 && frame.Length >= addressLength; index++)
            {
                if((receiveAddresses.Value & (1UL << index)) != 0 && LogicalAddress(index).SequenceEqual(frame.Take(addressLength)))
                {
                    matched = index;
                }
            }
            if(matched < 0)
            {
                this.Log(LogLevel.Debug, "Lost a frame on channel {0}: no receive address matches", arrival.Channel);
                return false;
            }

            var airHeaderLength = AirHeaderBytes;
            var airHeader = TakeBytes(frame, addressLength, airHeaderLength);
            var header = UnpackAirHeader(airHeader);
            var announcedLength = (int)(header.Length + staticLength.Value);
            var payloadLength = (int)Math.Min((ulong)announcedLength, maximumLength.Value);
            var payload = TakeBytes(frame, addressLength + airHeaderLength, payloadLength);
            var address = frame.Take(addressLength).ToArray();

            var crc = ComputeCrc(address, airHeader, payload);
            var crcGood = announcedLength == payloadLength;
            if(arrival.Sender != null)
            {
                crcGood &= crc == arrival.Sender.Crc && WhiteningKey == arrival.Sender.Whitening;
            }

            var bitTime = BitNanoseconds;
            var packet = new PacketTimeline(false, time + ReceiveChainNanoseconds, bitTime, HeaderAirBits, payloadLength, CrcBits);
            currentPacket = packet;
            currentBitNanoseconds = bitTime;

            Schedule(packet.AddressTime, Work.Packet, eventTime => ReceivedAddress(eventTime, matched, header, payload));
            if((deviceAddressConfiguration.Value & DeviceAddressEnableMask) != 0 && (ulong)payloadLength >= DeviceAddressBytes)
            {
                var deviceAddressTime = packet.AddressTime + (packet.HeaderBits + DeviceAddressBytes * 8) * bitTime;
                Schedule(deviceAddressTime, Work.Packet, _ => MatchDeviceAddress(header, payload));
            }
            Schedule(packet.PayloadTime, Work.Packet, _ => RaiseEvent(RadioEvent.Payload));
            Schedule(packet.EndTime, Work.Packet, eventTime =>
            {
                crcMatched = crcGood;
                receivedCrc = crc;
                EndReached(eventTime);
            });
            return true;
        }

        // The whole packet lands in RAM by the ADDRESS event, so CCM and AAR started from it through PPI see real data.
        private void ReceivedAddress(ulong time, int addressIndex, RamHeader header, byte[] payload)
        {
            WriteRamHeader(receivePointer, header);
            WriteDataRam(payload, receivePointer + (ulong)RamHeaderBytes);
            receivedAddressIndex = (uint)addressIndex;
            AddressReached(time);
        }

        private void AddressReached(ulong time)
        {
            RaiseEvent(RadioEvent.Address);
            if(HasShortcut(Shortcut.AddressRssiStart))
            {
                RssiStart(time);
            }
            if(HasShortcut(Shortcut.AddressBitCounterStart))
            {
                BitCounterStart(time);
            }
        }

        private void EndReached(ulong time)
        {
            var packet = currentPacket;
            currentPacket = null;
            bitCounterStart = null;
            Cancel(Work.BitCounter);
            if(packet.Transmitting)
            {
                lastAirEnd = time + TransmitChainNanoseconds;
                state = RadioState.TxIdle;
            }
            else
            {
                lastAirEnd = time - ReceiveChainNanoseconds;
                state = RadioState.RxIdle;
                RaiseEvent(crcMatched ? RadioEvent.CrcOk : RadioEvent.CrcError);
            }
            RaiseEvent(RadioEvent.End);
            // On the 1 and 2 Mbit modes PHYEND comes with END.
            if(hasNrf52840Registers)
            {
                RaiseEvent(RadioEvent.PhyEnd);
            }
            if(HasShortcut(Shortcut.EndDisable) || HasShortcut(Shortcut.PhyEndDisable))
            {
                DisableFrom(time, true);
            }
            else if(HasShortcut(Shortcut.EndStart) || HasShortcut(Shortcut.PhyEndStart))
            {
                Start(time);
            }
        }

        // Compares the advertiser address after the header with each enabled DAB, DAP and TxAdd.
        private void MatchDeviceAddress(RamHeader header, byte[] payload)
        {
            var low = BitConverter.ToUInt32(payload, 0);
            var high = (uint)BitConverter.ToUInt16(payload, 4);
            var transmitAddressType = (header.S0.Length > 0 ? (header.S0[0] >> 6) & 1 : 0) == 1;
            for(var index = 0; index < LogicalAddressCount; index++)
            {
                var enabled = (deviceAddressConfiguration.Value & (1UL << index)) != 0;
                var wantedType = (deviceAddressConfiguration.Value & (1UL << (index + 8))) != 0;
                if(enabled && wantedType == transmitAddressType && deviceAddressBase[index].Value == low && deviceAddressPrefix[index].Value == high)
                {
                    deviceAddressIndex = (uint)index;
                    RaiseEvent(RadioEvent.DeviceMatch);
                    return;
                }
            }
            RaiseEvent(RadioEvent.DeviceMiss);
        }

        // ====================================================================
        // Events and interrupt
        // ====================================================================

        private void RaiseEvent(RadioEvent radioEvent)
        {
            var index = (int)radioEvent;
            eventsPending |= 1u << index;
            var listeners = EventTriggered;
            listeners?.Invoke(EventsOffset + (uint)index * 4);
            // Renode's PPI runs its tasks at the next sync point, so the interrupt waits for that point too and the CPU never sees an event before its PPI work is done.
            if(listeners != null && (eventsPending & interruptEnabled) != 0)
            {
                if(!interruptWaitsForSync)
                {
                    interruptWaitsForSync = true;
                    machine.LocalTimeSource.ExecuteInNearestSyncedState(_ =>
                    {
                        interruptWaitsForSync = false;
                        UpdateInterrupt();
                    });
                }
                return;
            }
            UpdateInterrupt();
        }

        private void UpdateInterrupt()
        {
            IRQ.Set((eventsPending & interruptEnabled) != 0);
        }

        // ====================================================================
        // Packet format
        // ====================================================================

        // RAM holds S0, then LENGTH, then S1 in whole bytes; S1INCL keeps a byte for S1 even when it has no bits.
        private int LengthFieldBytes => (int)(lengthFieldBits.Value + 7) / 8;

        private int S1Bytes => s1Bits.Value > 0 ? (int)(s1Bits.Value + 7) / 8 : (s1Include.Value ? 1 : 0);

        private int RamHeaderBytes => (int)s0Length.Value + LengthFieldBytes + S1Bytes;

        // On the medium the header is S0, then LENGTH and S1 packed together from the least significant bit.
        private int AirHeaderBytes => (int)s0Length.Value + (int)(lengthFieldBits.Value + s1Bits.Value + 7) / 8;

        private RamHeader ReadRamHeader(ulong pointer)
        {
            var bytes = sysbus.ReadBytes(pointer, RamHeaderBytes);
            var s0Bytes = (int)s0Length.Value;
            return new RamHeader
            {
                S0 = bytes.Take(s0Bytes).ToArray(),
                Length = LittleEndianValue(bytes, s0Bytes, LengthFieldBytes) & BitMask(lengthFieldBits.Value),
                S1 = LittleEndianValue(bytes, s0Bytes + LengthFieldBytes, S1Bytes) & BitMask(s1Bits.Value),
            };
        }

        private void WriteRamHeader(ulong pointer, RamHeader header)
        {
            var bytes = new byte[RamHeaderBytes];
            Array.Copy(header.S0, bytes, Math.Min(header.S0.Length, (int)s0Length.Value));
            StoreLittleEndian(bytes, (int)s0Length.Value, LengthFieldBytes, header.Length);
            StoreLittleEndian(bytes, (int)s0Length.Value + LengthFieldBytes, S1Bytes, header.S1);
            WriteDataRam(bytes, pointer);
        }

        // EasyDMA reaches only data RAM, so a pointer anywhere else (flash included) writes nothing.
        private void WriteDataRam(byte[] bytes, ulong address)
        {
            if(address < DataRamStart || address + (ulong)bytes.Length > DataRamEnd)
            {
                this.Log(LogLevel.Warning, "EasyDMA write of {0} bytes to 0x{1:X} is outside data RAM, dropped", bytes.Length, address);
                return;
            }
            sysbus.WriteBytes(bytes, address);
        }

        private const ulong DataRamStart = 0x20000000;
        private const ulong DataRamEnd = 0x20040000;

        private byte[] PackAirHeader(RamHeader header)
        {
            var bytes = new byte[AirHeaderBytes];
            Array.Copy(header.S0, bytes, Math.Min(header.S0.Length, (int)s0Length.Value));
            var packed = header.Length | (header.S1 << (int)lengthFieldBits.Value);
            StoreLittleEndian(bytes, (int)s0Length.Value, bytes.Length - (int)s0Length.Value, packed);
            return bytes;
        }

        private RamHeader UnpackAirHeader(byte[] bytes)
        {
            var s0Bytes = (int)s0Length.Value;
            var packed = LittleEndianValue(bytes, s0Bytes, bytes.Length - s0Bytes);
            return new RamHeader
            {
                S0 = bytes.Take(s0Bytes).ToArray(),
                Length = packed & BitMask(lengthFieldBits.Value),
                S1 = (packed >> (int)lengthFieldBits.Value) & BitMask(s1Bits.Value),
            };
        }

        // A logical address as it goes on air: the top BALEN bytes of its base, least significant first, then its prefix byte.
        private byte[] LogicalAddress(int index)
        {
            var baseValue = index == 0 ? baseAddress0.Value : baseAddress1.Value;
            var prefixes = index < 4 ? prefix0.Value : prefix1.Value;
            var prefix = (byte)(prefixes >> (8 * (index % 4)));
            var baseBytes = AddressBytes - 1;
            var bytes = new byte[AddressBytes];
            for(var position = 0; position < baseBytes; position++)
            {
                bytes[position] = (byte)(baseValue >> (8 * (4 - baseBytes + position)));
            }
            bytes[baseBytes] = prefix;
            return bytes;
        }

        // The radio's CRC: a shift register of LEN bytes fed with each byte from its least significant bit.
        private uint ComputeCrc(byte[] address, byte[] airHeader, byte[] payload)
        {
            var bits = (int)CrcBits;
            if(bits == 0)
            {
                return 0;
            }
            var mask = (uint)BitMask((ulong)bits);
            var crc = (uint)crcInitial.Value & mask;
            var polynomial = (uint)crcPolynomial.Value & mask;
            var data = crcSkipAddress.Value ? airHeader.Concat(payload) : address.Concat(airHeader).Concat(payload);
            foreach(var value in data)
            {
                for(var bit = 0; bit < 8; bit++)
                {
                    var input = (uint)(value >> bit) & 1;
                    var top = (crc >> (bits - 1)) & 1;
                    crc = (crc << 1) & mask;
                    if(input != top)
                    {
                        crc ^= polynomial;
                    }
                }
            }
            return crc;
        }

        // Data whitening only has to agree between the two ends, so its seed stands in for the whitened bits.
        private int WhiteningKey => whiteningEnabled.Value ? (int)(whiteningSeed & 0x3F) : -1;

        private static byte[] TakeBytes(byte[] source, int start, int count)
        {
            var bytes = new byte[count];
            if(start < source.Length)
            {
                Array.Copy(source, start, bytes, 0, Math.Min(count, source.Length - start));
            }
            return bytes;
        }

        private static ulong LittleEndianValue(byte[] bytes, int start, int count)
        {
            ulong value = 0;
            for(var index = 0; index < count; index++)
            {
                value |= (ulong)bytes[start + index] << (8 * index);
            }
            return value;
        }

        private static void StoreLittleEndian(byte[] bytes, int start, int count, ulong value)
        {
            for(var index = 0; index < count; index++)
            {
                bytes[start + index] = (byte)(value >> (8 * index));
            }
        }

        private static ulong BitMask(ulong bits)
        {
            return bits >= 64 ? ulong.MaxValue : (1UL << (int)bits) - 1;
        }

        // ====================================================================
        // Registers
        // ====================================================================

        // Errata workarounds poke undocumented registers; they keep what is written so read-modify-write code works.
        public override uint ReadDoubleWord(long offset)
        {
            if(RegistersCollection.TryRead(offset, out var value))
            {
                return value;
            }
            undocumentedRegisters.TryGetValue(offset, out value);
            return value;
        }

        public override void WriteDoubleWord(long offset, uint value)
        {
            if(!RegistersCollection.TryWrite(offset, value))
            {
                undocumentedRegisters[offset] = value;
            }
        }

        private void DefineRegisters()
        {
            DefineTask(Registers.TransmitEnable, "TASKS_TXEN", TransmitEnable);
            DefineTask(Registers.ReceiveEnable, "TASKS_RXEN", ReceiveEnable);
            DefineTask(Registers.Start, "TASKS_START", Start);
            DefineTask(Registers.Stop, "TASKS_STOP", Stop);
            DefineTask(Registers.Disable, "TASKS_DISABLE", Disable);
            DefineTask(Registers.RssiStart, "TASKS_RSSISTART", RssiStart);
            DefineTask(Registers.RssiStop, "TASKS_RSSISTOP", RssiStop);
            DefineTask(Registers.BitCounterStart, "TASKS_BCSTART", BitCounterStart);
            DefineTask(Registers.BitCounterStop, "TASKS_BCSTOP", BitCounterStop);

            foreach(RadioEvent radioEvent in Enum.GetValues(typeof(RadioEvent)))
            {
                if((eventMask & (1u << (int)radioEvent)) == 0)
                {
                    continue;
                }
                var bit = 1u << (int)radioEvent;
                RegistersCollection.AddRegister(EventsOffset + (int)radioEvent * 4, new DoubleWordRegister(this)
                    .WithFlag(0, valueProviderCallback: _ =>
                    {
                        CatchUp();
                        return (eventsPending & bit) != 0;
                    }, writeCallback: (_, value) =>
                    {
                        CatchUp();
                        eventsPending = value ? eventsPending | bit : eventsPending & ~bit;
                        UpdateInterrupt();
                    }, name: $"EVENTS_{radioEvent}")
                    .WithReservedBits(1, 31));
            }

            Registers.Shortcuts.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => shortcuts, writeCallback: (_, value) =>
                {
                    CatchUp();
                    shortcuts = (uint)value & shortcutMask;
                }, name: "SHORTS");

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnabled, writeCallback: (_, value) =>
                {
                    interruptEnabled |= (uint)value & eventMask;
                    UpdateInterrupt();
                }, name: "INTENSET");

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, 32, valueProviderCallback: _ => interruptEnabled, writeCallback: (_, value) =>
                {
                    interruptEnabled &= ~((uint)value & eventMask);
                    UpdateInterrupt();
                }, name: "INTENCLR");

            Registers.CrcStatus.Define(this)
                .WithFlag(0, FieldMode.Read, valueProviderCallback: _ =>
                {
                    CatchUp();
                    return crcMatched;
                }, name: "CRCSTATUS")
                .WithReservedBits(1, 31);

            Registers.ReceiveMatch.Define(this)
                .WithValueField(0, 3, FieldMode.Read, valueProviderCallback: _ => receivedAddressIndex, name: "RXMATCH")
                .WithReservedBits(3, 29);

            Registers.ReceiveCrc.Define(this)
                .WithValueField(0, 24, FieldMode.Read, valueProviderCallback: _ =>
                {
                    CatchUp();
                    return receivedCrc;
                }, name: "RXCRC")
                .WithReservedBits(24, 8);

            Registers.DeviceAddressIndex.Define(this)
                .WithValueField(0, 3, FieldMode.Read, valueProviderCallback: _ => deviceAddressIndex, name: "DAI")
                .WithReservedBits(3, 29);

            Registers.PacketPointer.Define(this)
                .WithValueField(0, 32, out packetPointer, name: "PACKETPTR");

            Registers.Frequency.Define(this, 0x2)
                .WithValueField(0, 7, out frequency, name: "FREQUENCY")
                .WithReservedBits(7, 1)
                .WithFlag(8, out frequencyMap, name: "MAP")
                .WithReservedBits(9, 23);

            Registers.TransmitPower.Define(this)
                .WithValueField(0, 8, name: "TXPOWER")
                .WithReservedBits(8, 24);

            Registers.Mode.Define(this)
                .WithValueField(0, 4, out mode, name: "MODE")
                .WithReservedBits(4, 28);

            Registers.PacketConfiguration0.Define(this)
                .WithValueField(0, 4, out lengthFieldBits, name: "LFLEN")
                .WithReservedBits(4, 4)
                .WithValueField(8, 1, out s0Length, name: "S0LEN")
                .WithReservedBits(9, 7)
                .WithValueField(16, 4, out s1Bits, name: "S1LEN")
                .WithFlag(20, out s1Include, name: "S1INCL")
                .WithReservedBits(21, 3)
                .WithFlag(24, out preambleLength, name: "PLEN")
                .WithReservedBits(25, 7);

            Registers.PacketConfiguration1.Define(this)
                .WithValueField(0, 8, out maximumLength, name: "MAXLEN")
                .WithValueField(8, 8, out staticLength, name: "STATLEN")
                .WithValueField(16, 3, out baseAddressLength, name: "BALEN")
                .WithReservedBits(19, 5)
                .WithFlag(24, name: "ENDIAN")
                .WithFlag(25, out whiteningEnabled, name: "WHITEEN")
                .WithReservedBits(26, 6);

            Registers.BaseAddress0.Define(this)
                .WithValueField(0, 32, out baseAddress0, name: "BASE0");

            Registers.BaseAddress1.Define(this)
                .WithValueField(0, 32, out baseAddress1, name: "BASE1");

            Registers.Prefix0.Define(this)
                .WithValueField(0, 32, out prefix0, name: "PREFIX0");

            Registers.Prefix1.Define(this)
                .WithValueField(0, 32, out prefix1, name: "PREFIX1");

            Registers.TransmitAddress.Define(this)
                .WithValueField(0, 3, out transmitAddress, name: "TXADDRESS")
                .WithReservedBits(3, 29);

            Registers.ReceiveAddresses.Define(this)
                .WithValueField(0, 8, out receiveAddresses, name: "RXADDRESSES")
                .WithReservedBits(8, 24);

            Registers.CrcConfiguration.Define(this)
                .WithValueField(0, 2, out crcLength, name: "LEN")
                .WithReservedBits(2, 6)
                .WithFlag(8, out crcSkipAddress, name: "SKIPADDR")
                .WithReservedBits(9, 23);

            Registers.CrcPolynomial.Define(this)
                .WithValueField(0, 24, out crcPolynomial, name: "CRCPOLY")
                .WithReservedBits(24, 8);

            Registers.CrcInitial.Define(this)
                .WithValueField(0, 24, out crcInitial, name: "CRCINIT")
                .WithReservedBits(24, 8);

            Registers.InterFrameSpacing.Define(this)
                .WithValueField(0, 8, out interFrameSpacing, name: "TIFS")
                .WithReservedBits(8, 24);

            Registers.RssiSample.Define(this)
                .WithValueField(0, 7, FieldMode.Read, valueProviderCallback: _ => RssiSampleMagnitude, name: "RSSISAMPLE")
                .WithReservedBits(7, 25);

            Registers.State.Define(this)
                .WithValueField(0, 4, FieldMode.Read, valueProviderCallback: _ =>
                {
                    CatchUp();
                    return (ulong)state;
                }, name: "STATE")
                .WithReservedBits(4, 28);

            // Bit 6 of DATAWHITEIV always reads one.
            Registers.DataWhiteningInitial.Define(this)
                .WithValueField(0, 7, valueProviderCallback: _ => whiteningSeed | 0x40, writeCallback: (_, value) => whiteningSeed = (uint)value, name: "DATAWHITEIV")
                .WithReservedBits(7, 25);

            Registers.BitCountCompare.Define(this)
                .WithValueField(0, 32, out bitCountCompare, writeCallback: (_, __) => ScheduleBitCounterMatch(CatchUp()), name: "BCC");

            for(var index = 0; index < LogicalAddressCount; index++)
            {
                RegistersCollection.AddRegister((long)Registers.DeviceAddressBase + index * 4, new DoubleWordRegister(this)
                    .WithValueField(0, 32, out deviceAddressBase[index], name: $"DAB[{index}]"));
                RegistersCollection.AddRegister((long)Registers.DeviceAddressPrefix + index * 4, new DoubleWordRegister(this)
                    .WithValueField(0, 16, out deviceAddressPrefix[index], name: $"DAP[{index}]")
                    .WithReservedBits(16, 16));
            }

            Registers.DeviceAddressConfiguration.Define(this)
                .WithValueField(0, 16, out deviceAddressConfiguration, name: "DACNF")
                .WithReservedBits(16, 16);

            Registers.ModeConfiguration0.Define(this, 0x200)
                .WithFlag(0, out fastRampUp, name: "RU")
                .WithReservedBits(1, 7)
                .WithValueField(8, 2, name: "DTX")
                .WithReservedBits(10, 22);

            // Turning POWER off and on resets the radio, as firmware does at start-up.
            Registers.Power.Define(this, 0x1)
                .WithFlag(0, valueProviderCallback: _ => powered, writeCallback: (_, value) =>
                {
                    if(!value && powered)
                    {
                        Reset();
                        powered = false;
                    }
                    else if(value)
                    {
                        powered = true;
                    }
                }, name: "POWER")
                .WithReservedBits(1, 31);
        }

        private void DefineTask(Registers register, string name, Action<ulong> task)
        {
            register.Define(this)
                .WithFlag(0, FieldMode.Write, writeCallback: (_, trigger) => { if(trigger) RunTask(task); }, name: name)
                .WithReservedBits(1, 31);
        }

        private const ulong NanosecondsPerSecond = 1000000000;
        private const ulong RssiSampleNanoseconds = 250;
        // The receiver reports -40 dBm for every packet.
        private const ulong RssiSampleMagnitude = 40;
        private const ulong MaximumDeliveryDelayNanoseconds = 1000000;
        private const ulong DeviceAddressBytes = 6;
        private const ulong DeviceAddressEnableMask = 0xFF;
        private const int LogicalAddressCount = 8;
        private const int NormalBandStart = 2400;
        private const int LowBandStart = 2360;
        private const int NonBluetoothChannelBase = 1000;
        private const uint EventsOffset = 0x100;
        private const uint Nrf52832EventMask = 0x34FF;
        private const uint Nrf52840ExtraEventMask = (1u << 21) | (1u << 22) | (1u << 27);
        private const uint Nrf52832ShortcutMask = 0x17F;
        private const uint Nrf52840ExtraShortcutMask = (1u << 18) | (1u << 19) | (1u << 20) | (1u << 21);

        private readonly bool hasNrf52840Registers;
        private readonly uint eventMask;
        private readonly uint shortcutMask;
        private readonly Action alarmHandler;
        private readonly List<ScheduledWork> scheduled = new List<ScheduledWork>();
        private readonly Dictionary<long, uint> undocumentedRegisters = new Dictionary<long, uint>();
        private readonly IValueRegisterField[] deviceAddressBase = new IValueRegisterField[LogicalAddressCount];
        private readonly IValueRegisterField[] deviceAddressPrefix = new IValueRegisterField[LogicalAddressCount];

        private long nextOrder;
        private bool processing;
        private ulong alarmTime;
        private RadioState state;
        private bool powered;
        private uint shortcuts;
        private uint interruptEnabled;
        private uint eventsPending;
        private PacketTimeline currentPacket;
        private ulong currentBitNanoseconds = 1000;
        private ulong receivePointer;
        private ulong? bitCounterStart;
        private bool crcMatched;
        private uint receivedAddressIndex;
        private uint receivedCrc;
        private uint deviceAddressIndex;
        private ulong lastAirEnd;
        private bool turnaroundAfterEnd;
        private Transmission lastTransmission;
        private bool interruptWaitsForSync;
        private uint whiteningSeed;

        private IValueRegisterField packetPointer;
        private IValueRegisterField frequency;
        private IFlagRegisterField frequencyMap;
        private IValueRegisterField mode;
        private IValueRegisterField lengthFieldBits;
        private IValueRegisterField s0Length;
        private IValueRegisterField s1Bits;
        private IFlagRegisterField s1Include;
        private IFlagRegisterField preambleLength;
        private IValueRegisterField maximumLength;
        private IValueRegisterField staticLength;
        private IValueRegisterField baseAddressLength;
        private IFlagRegisterField whiteningEnabled;
        private IValueRegisterField baseAddress0;
        private IValueRegisterField baseAddress1;
        private IValueRegisterField prefix0;
        private IValueRegisterField prefix1;
        private IValueRegisterField transmitAddress;
        private IValueRegisterField receiveAddresses;
        private IValueRegisterField crcLength;
        private IFlagRegisterField crcSkipAddress;
        private IValueRegisterField crcPolynomial;
        private IValueRegisterField crcInitial;
        private IValueRegisterField interFrameSpacing;
        private IValueRegisterField bitCountCompare;
        private IValueRegisterField deviceAddressConfiguration;
        private IFlagRegisterField fastRampUp;

        // A piece of work waiting for its moment in virtual time.
        private sealed class ScheduledWork
        {
            public ScheduledWork(ulong time, long order, Work kind, Action<ulong> run)
            {
                Time = time;
                Order = order;
                Kind = kind;
                Run = run;
            }

            public ulong Time { get; }
            public long Order { get; }
            public Work Kind { get; }
            public Action<ulong> Run { get; }
        }

        // When each event of one packet happens, counted from its ADDRESS event.
        private sealed class PacketTimeline
        {
            public PacketTimeline(bool transmitting, ulong addressTime, ulong bitNanoseconds, ulong headerBits, int payloadBytes, ulong crcBits)
            {
                Transmitting = transmitting;
                AddressTime = addressTime;
                HeaderBits = headerBits;
                PayloadTime = addressTime + (headerBits + (ulong)payloadBytes * 8) * bitNanoseconds;
                EndTime = PayloadTime + crcBits * bitNanoseconds;
            }

            public bool Transmitting { get; }
            public ulong AddressTime { get; }
            public ulong HeaderBits { get; }
            public ulong PayloadTime { get; }
            public ulong EndTime { get; }
        }

        // What a sender of this model knows about its last packet, beyond the bytes the medium carries.
        private sealed class Transmission
        {
            public byte[] Frame;
            public ulong AirStart;
            public ulong BitNanoseconds;
            public ulong PreambleBits;
            public RadioMode Mode;
            public uint Crc;
            public int Whitening;
        }

        private sealed class Arrival
        {
            public byte[] Frame;
            public int Channel;
            public Transmission Sender;
        }

        private sealed class RamHeader
        {
            public byte[] S0;
            public ulong Length;
            public ulong S1;
        }

        [Flags]
        private enum Work
        {
            RampUp = 1,
            Packet = 2,
            Disabling = 4,
            Rssi = 8,
            BitCounter = 16,
            Arrival = 32,
        }

        [Flags]
        private enum Shortcut : uint
        {
            ReadyStart = 1 << 0,
            EndDisable = 1 << 1,
            DisabledTxEnable = 1 << 2,
            DisabledRxEnable = 1 << 3,
            AddressRssiStart = 1 << 4,
            EndStart = 1 << 5,
            AddressBitCounterStart = 1 << 6,
            DisabledRssiStop = 1 << 8,
            TxReadyStart = 1 << 18,
            RxReadyStart = 1 << 19,
            PhyEndDisable = 1 << 20,
            PhyEndStart = 1 << 21,
        }

        private enum RadioMode
        {
            Nrf1Mbit = 0,
            Nrf2Mbit = 1,
            Nrf250Kbit = 2,
            Ble1Mbit = 3,
            Ble2Mbit = 4,
        }

        private enum RadioState
        {
            Disabled = 0,
            RxRampUp = 1,
            RxIdle = 2,
            Rx = 3,
            RxDisable = 4,
            TxRampUp = 9,
            TxIdle = 10,
            Tx = 11,
            TxDisable = 12,
        }

        // Each event's number is its INTEN bit and its offset from 0x100 in words.
        private enum RadioEvent
        {
            Ready = 0,
            Address = 1,
            Payload = 2,
            End = 3,
            Disabled = 4,
            DeviceMatch = 5,
            DeviceMiss = 6,
            RssiEnd = 7,
            BitCounterMatch = 10,
            CrcOk = 12,
            CrcError = 13,
            TxReady = 21,
            RxReady = 22,
            PhyEnd = 27,
        }

        private enum Registers
        {
            TransmitEnable = 0x000,
            ReceiveEnable = 0x004,
            Start = 0x008,
            Stop = 0x00C,
            Disable = 0x010,
            RssiStart = 0x014,
            RssiStop = 0x018,
            BitCounterStart = 0x01C,
            BitCounterStop = 0x020,
            Shortcuts = 0x200,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            CrcStatus = 0x400,
            ReceiveMatch = 0x408,
            ReceiveCrc = 0x40C,
            DeviceAddressIndex = 0x410,
            PacketPointer = 0x504,
            Frequency = 0x508,
            TransmitPower = 0x50C,
            Mode = 0x510,
            PacketConfiguration0 = 0x514,
            PacketConfiguration1 = 0x518,
            BaseAddress0 = 0x51C,
            BaseAddress1 = 0x520,
            Prefix0 = 0x524,
            Prefix1 = 0x528,
            TransmitAddress = 0x52C,
            ReceiveAddresses = 0x530,
            CrcConfiguration = 0x534,
            CrcPolynomial = 0x538,
            CrcInitial = 0x53C,
            InterFrameSpacing = 0x544,
            RssiSample = 0x548,
            State = 0x550,
            DataWhiteningInitial = 0x554,
            BitCountCompare = 0x560,
            DeviceAddressBase = 0x600,
            DeviceAddressPrefix = 0x620,
            DeviceAddressConfiguration = 0x640,
            ModeConfiguration0 = 0x650,
            Power = 0xFFC,
        }
    }
}
