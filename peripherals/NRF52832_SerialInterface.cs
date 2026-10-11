// One nRF52832 shared serial block that acts as SPI, SPIM, TWI or TWIM depending on its ENABLE value.

using System;
using System.Collections.Generic;
using System.Linq;

using Antmicro.Renode.Core;
using Antmicro.Renode.Core.Structure;
using Antmicro.Renode.Core.Structure.Registers;
using Antmicro.Renode.Exceptions;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Bus;
using Antmicro.Renode.Peripherals.I2C;
using Antmicro.Renode.Peripherals.Miscellaneous;

namespace Antmicro.Renode.Peripherals.SPI
{
    public class NRF52832_SerialInterface : IDoubleWordPeripheral, IProvidesRegisterCollection<DoubleWordRegisterCollection>, IKnownSize, INRFEventProvider,
        IPeripheralContainer<ISPIPeripheral, NullRegistrationPoint>, IPeripheralContainer<II2CPeripheral, NumberRegistrationPoint<int>>
    {
        public NRF52832_SerialInterface(IMachine machine)
        {
            this.machine = machine;
            systemBus = machine.GetSystemBus(this);
            spiChildren = new NullRegistrationPointContainerHelper<ISPIPeripheral>(machine, this);
            i2cChildren = new SimpleContainerHelper<II2CPeripheral>(machine, this);

            IRQ = new GPIO();
            RegistersCollection = new DoubleWordRegisterCollection(this);
            DefineRegisters();
            Reset();
        }

        public void Reset()
        {
            RegistersCollection.Reset();
            Array.Clear(eventPending, 0, eventPending.Length);
            interruptEnable = 0;
            mode = Mode.Disabled;
            ClearTransferState();
            UpdateInterrupts();
        }

        public uint ReadDoubleWord(long offset)
        {
            return RegistersCollection.Read(offset);
        }

        public void WriteDoubleWord(long offset, uint value)
        {
            RegistersCollection.Write(offset, value);
        }

        public GPIO IRQ { get; }

        public DoubleWordRegisterCollection RegistersCollection { get; }

        public long Size => 0x1000;

        public event Action<uint> EventTriggered;

        // ====================================================================
        // Children
        // ====================================================================

        public void Register(ISPIPeripheral peripheral, NullRegistrationPoint registration_point)
        {
            spiChildren.Register(peripheral, registration_point);
        }

        public void Unregister(ISPIPeripheral peripheral)
        {
            spiChildren.Unregister(peripheral);
        }

        public IEnumerable<NullRegistrationPoint> GetRegistrationPoints(ISPIPeripheral peripheral)
        {
            return spiChildren.GetRegistrationPoints(peripheral);
        }

        IEnumerable<IRegistered<ISPIPeripheral, NullRegistrationPoint>> IPeripheralContainer<ISPIPeripheral, NullRegistrationPoint>.Children => spiChildren.Children;

        public void Register(II2CPeripheral peripheral, NumberRegistrationPoint<int> registration_point)
        {
            if(registration_point.Address < 0 || registration_point.Address > 0x7F)
            {
                throw new RegistrationException("An I2C device needs a 7-bit address.");
            }
            i2cChildren.Register(peripheral, registration_point);
        }

        public void Unregister(II2CPeripheral peripheral)
        {
            i2cChildren.Unregister(peripheral);
        }

        public IEnumerable<NumberRegistrationPoint<int>> GetRegistrationPoints(II2CPeripheral peripheral)
        {
            return i2cChildren.GetRegistrationPoints(peripheral);
        }

        IEnumerable<IRegistered<II2CPeripheral, NumberRegistrationPoint<int>>> IPeripheralContainer<II2CPeripheral, NumberRegistrationPoint<int>>.Children => i2cChildren.Children;

        // ====================================================================
        // Registers
        // ====================================================================

        private void DefineRegisters()
        {
            DefineTask(Registers.TasksStartReceive, "TASKS_STARTRX", StartReceiveTask);
            DefineTask(Registers.TasksStartTransmit, "TASKS_STARTTX", StartTransmitTask);
            DefineTask(Registers.TasksStart, "TASKS_START", StartTask);
            DefineTask(Registers.TasksStop, "TASKS_STOP", StopTask);
            DefineTask(Registers.TasksSuspend, "TASKS_SUSPEND", SuspendTask);
            DefineTask(Registers.TasksResume, "TASKS_RESUME", ResumeTask);

            foreach(var event_register in AllEvents)
            {
                var index = EventIndex(event_register);
                RegistersCollection.DefineRegister((long)event_register)
                    .WithFlag(0, name: event_register.ToString(),
                        valueProviderCallback: _ => eventPending[index],
                        writeCallback: (_, value) => { eventPending[index] = value; UpdateInterrupts(); })
                    .WithReservedBits(1, 31);
            }

            Registers.Shortcuts.Define(this)
                .WithValueField(0, 32, out shortcuts, name: "SHORTS");

            Registers.InterruptEnable.Define(this)
                .WithValueField(0, 32, name: "INTEN",
                    valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable = (uint)value & InterruptMask; UpdateInterrupts(); });

            Registers.InterruptEnableSet.Define(this)
                .WithValueField(0, 32, name: "INTENSET",
                    valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable |= (uint)value & InterruptMask; UpdateInterrupts(); });

            Registers.InterruptEnableClear.Define(this)
                .WithValueField(0, 32, name: "INTENCLR",
                    valueProviderCallback: _ => interruptEnable,
                    writeCallback: (_, value) => { interruptEnable &= ~(uint)value; UpdateInterrupts(); });

            Registers.ErrorSource.Define(this)
                .WithFlag(0, out overrunError, FieldMode.Read | FieldMode.WriteOneToClear, name: "OVERRUN")
                .WithFlag(1, out addressNackError, FieldMode.Read | FieldMode.WriteOneToClear, name: "ANACK")
                .WithFlag(2, FieldMode.Read | FieldMode.WriteOneToClear, name: "DNACK")
                .WithReservedBits(3, 29);

            Registers.Enable.Define(this)
                .WithValueField(0, 4, name: "ENABLE",
                    valueProviderCallback: _ => (ulong)mode,
                    writeCallback: (_, value) => SetMode(value))
                .WithReservedBits(4, 28);

            Registers.PinSelectClock.Define(this, 0xFFFFFFFF)
                .WithValueField(0, 32, name: "PSEL.SCK");
            Registers.PinSelectDataOut.Define(this, 0xFFFFFFFF)
                .WithValueField(0, 32, name: "PSEL.MOSI");
            Registers.PinSelectDataIn.Define(this, 0xFFFFFFFF)
                .WithValueField(0, 32, name: "PSEL.MISO");

            Registers.ReceiveData.Define(this)
                .WithValueField(0, 8, FieldMode.Read, name: "RXD", valueProviderCallback: _ => ReadReceiveData())
                .WithReservedBits(8, 24);

            Registers.TransmitData.Define(this)
                .WithValueField(0, 8, out transmitData, name: "TXD", writeCallback: (_, value) => WriteTransmitData((byte)value))
                .WithReservedBits(8, 24);

            Registers.Frequency.Define(this, 0x04000000)
                .WithValueField(0, 32, name: "FREQUENCY");

            Registers.ReceivePointer.Define(this)
                .WithValueField(0, 32, out receivePointer, name: "RXD.PTR", writeCallback: (_, value) => receiveListCursor = value);
            Registers.ReceiveMaximumCount.Define(this)
                .WithValueField(0, 8, out receiveMaximumCount, name: "RXD.MAXCNT")
                .WithReservedBits(8, 24);
            Registers.ReceiveAmount.Define(this)
                .WithValueField(0, 8, FieldMode.Read, name: "RXD.AMOUNT", valueProviderCallback: _ => receiveAmount)
                .WithReservedBits(8, 24);
            Registers.ReceiveList.Define(this)
                .WithValueField(0, 3, out receiveList, name: "RXD.LIST")
                .WithReservedBits(3, 29);

            Registers.TransmitPointer.Define(this)
                .WithValueField(0, 32, out transmitPointer, name: "TXD.PTR", writeCallback: (_, value) => transmitListCursor = value);
            Registers.TransmitMaximumCount.Define(this)
                .WithValueField(0, 8, out transmitMaximumCount, name: "TXD.MAXCNT")
                .WithReservedBits(8, 24);
            Registers.TransmitAmount.Define(this)
                .WithValueField(0, 8, FieldMode.Read, name: "TXD.AMOUNT", valueProviderCallback: _ => transmitAmount)
                .WithReservedBits(8, 24);
            Registers.TransmitList.Define(this)
                .WithValueField(0, 3, out transmitList, name: "TXD.LIST")
                .WithReservedBits(3, 29);

            Registers.Configuration.Define(this)
                .WithFlag(0, name: "ORDER")
                .WithFlag(1, name: "CPHA")
                .WithFlag(2, name: "CPOL")
                .WithReservedBits(3, 29);

            Registers.TwoWireAddress.Define(this)
                .WithValueField(0, 7, out twoWireAddress, name: "ADDRESS")
                .WithReservedBits(7, 25);

            Registers.OverReadCharacter.Define(this)
                .WithValueField(0, 8, out overReadCharacter, name: "ORC")
                .WithReservedBits(8, 24);
        }

        private void DefineTask(Registers task, string name, Action action)
        {
            task.Define(this)
                .WithFlag(0, FieldMode.Write, name: name, writeCallback: (_, value) => { if(value) action(); })
                .WithReservedBits(1, 31);
        }

        private void SetMode(ulong value)
        {
            var new_mode = Enum.IsDefined(typeof(Mode), (int)value) ? (Mode)value : Mode.Disabled;
            if(new_mode == Mode.Disabled && value != 0)
            {
                this.Log(LogLevel.Warning, "ENABLE value {0} is not a serial mode on this chip, so the block stays disabled", value);
            }
            if(new_mode != mode)
            {
                EndTwoWireTransactionSilently();
                ClearTransferState();
                mode = new_mode;
            }
            UpdateInterrupts();
        }

        private void ClearTransferState()
        {
            spiReceiveFifo.Clear();
            spiTransmitQueue.Clear();
            spiStartWaitingForResume = false;
            restartScheduled = false;
            suspended = false;
            twoWireActive = false;
            twoWireTarget = null;
            addressNotAcknowledged = false;
            pendingWriteBytes.Clear();
            receiveDataUnread = false;
            receiveData = 0;
            transmitDataWaiting = false;
            legacyReceiving = false;
            legacyTransmitting = false;
            taskWaitingForResume = WaitingTask.None;
            lastSegmentWasWrite = false;
            receiveAmount = 0;
            transmitAmount = 0;
        }

        // ====================================================================
        // Events and interrupts
        // ====================================================================

        private static int EventIndex(Registers event_register)
        {
            return ((int)event_register - 0x100) / 4;
        }

        private void RaiseEvent(Registers event_register)
        {
            eventPending[EventIndex(event_register)] = true;
            EventTriggered?.Invoke((uint)event_register);
            UpdateInterrupts();
        }

        private void UpdateInterrupts()
        {
            var pending = false;
            if(mode != Mode.Disabled)
            {
                for(var index = 0; index < eventPending.Length; index++)
                {
                    pending |= eventPending[index] && (interruptEnable & (1u << index)) != 0;
                }
            }
            IRQ.Set(pending);
        }

        private bool Shortcut(Shortcuts shortcut)
        {
            return (shortcuts.Value & (1ul << (int)shortcut)) != 0;
        }

        // ====================================================================
        // Tasks
        // ====================================================================

        private void StartTask()
        {
            if(mode != Mode.SpiMaster)
            {
                this.Log(LogLevel.Warning, "TASKS_START ignored outside SPIM mode");
                return;
            }
            if(suspended)
            {
                spiStartWaitingForResume = true;
                return;
            }
            RunSpiMasterTransfer();
        }

        private void StartTransmitTask()
        {
            if(mode == Mode.TwoWireMaster)
            {
                if(suspended)
                {
                    taskWaitingForResume = WaitingTask.StartTransmit;
                    return;
                }
                RunTwoWireMasterTransmit(continue_previous_write: false, chain_depth: 0);
            }
            else if(mode == Mode.TwoWire)
            {
                StartLegacyTransmit();
            }
            else
            {
                this.Log(LogLevel.Warning, "TASKS_STARTTX ignored outside TWI and TWIM modes");
            }
        }

        private void StartReceiveTask()
        {
            if(mode == Mode.TwoWireMaster)
            {
                if(suspended)
                {
                    taskWaitingForResume = WaitingTask.StartReceive;
                    return;
                }
                RunTwoWireMasterReceive(chain_depth: 0);
            }
            else if(mode == Mode.TwoWire)
            {
                StartLegacyReceive();
            }
            else
            {
                this.Log(LogLevel.Warning, "TASKS_STARTRX ignored outside TWI and TWIM modes");
            }
        }

        private void StopTask()
        {
            switch(mode)
            {
            case Mode.SpiMaster:
                spiStartWaitingForResume = false;
                restartScheduled = false;
                suspended = false;
                RaiseEvent(Registers.EventsStopped);
                break;
            case Mode.TwoWire:
            case Mode.TwoWireMaster:
                StopTwoWire();
                break;
            default:
                this.Log(LogLevel.Warning, "TASKS_STOP ignored in mode {0}", mode);
                break;
            }
        }

        private void SuspendTask()
        {
            switch(mode)
            {
            case Mode.SpiMaster:
                suspended = true;
                break;
            case Mode.TwoWire:
            case Mode.TwoWireMaster:
                if(twoWireActive)
                {
                    SuspendTwoWire();
                }
                break;
            default:
                this.Log(LogLevel.Warning, "TASKS_SUSPEND ignored in mode {0}", mode);
                break;
            }
        }

        private void ResumeTask()
        {
            if(!suspended)
            {
                return;
            }
            suspended = false;

            switch(mode)
            {
            case Mode.SpiMaster:
                if(spiStartWaitingForResume)
                {
                    spiStartWaitingForResume = false;
                    RunSpiMasterTransfer();
                }
                break;
            case Mode.TwoWire:
                ContinueLegacyTransfer();
                break;
            case Mode.TwoWireMaster:
                var waiting = taskWaitingForResume;
                taskWaitingForResume = WaitingTask.None;
                if(waiting == WaitingTask.StartTransmit)
                {
                    RunTwoWireMasterTransmit(continue_previous_write: lastSegmentWasWrite, chain_depth: 0);
                }
                else if(waiting == WaitingTask.StartReceive)
                {
                    RunTwoWireMasterReceive(chain_depth: 0);
                }
                break;
            }
        }

        // ====================================================================
        // Legacy SPI
        // ====================================================================

        private void WriteTransmitData(byte value)
        {
            if(mode == Mode.Spi)
            {
                if(spiTransmitQueue.Count >= LegacyBufferDepth)
                {
                    this.Log(LogLevel.Warning, "TXD written while both transmit buffers are full, byte 0x{0:X2} dropped", value);
                    return;
                }
                spiTransmitQueue.Enqueue(value);
                PumpLegacySpi();
            }
            else if(mode == Mode.TwoWire)
            {
                transmitDataWaiting = true;
                ContinueLegacyTransfer();
            }
        }

        private uint ReadReceiveData()
        {
            if(mode == Mode.Spi)
            {
                if(spiReceiveFifo.Count == 0)
                {
                    this.Log(LogLevel.Warning, "RXD read with no received byte");
                    return 0;
                }
                var value = spiReceiveFifo.Dequeue();
                if(spiReceiveFifo.Count > 0)
                {
                    RaiseEvent(Registers.EventsReady);
                }
                PumpLegacySpi();
                return value;
            }
            if(mode == Mode.TwoWire)
            {
                var value = receiveData;
                receiveDataUnread = false;
                ContinueLegacyTransfer();
                return value;
            }
            return 0;
        }

        // Sends queued bytes while the two-byte receive buffer has room, as the hardware stalls otherwise.
        private void PumpLegacySpi()
        {
            while(spiTransmitQueue.Count > 0 && spiReceiveFifo.Count < LegacyBufferDepth)
            {
                var received = ExchangeSpiByte(spiTransmitQueue.Dequeue());
                spiReceiveFifo.Enqueue(received);
                if(spiReceiveFifo.Count == 1)
                {
                    RaiseEvent(Registers.EventsReady);
                }
            }
        }

        private byte ExchangeSpiByte(byte value)
        {
            var device = spiChildren.RegisteredPeripheral;
            if(device == null)
            {
                this.Log(LogLevel.Noisy, "No SPI device attached, byte 0x{0:X2} goes nowhere", value);
                return 0;
            }
            return device.Transmit(value);
        }

        // ====================================================================
        // SPIM
        // ====================================================================

        private void RunSpiMasterTransfer()
        {
            RaiseEvent(Registers.EventsStarted);

            var transmit_count = (int)transmitMaximumCount.Value;
            var receive_count = (int)receiveMaximumCount.Value;
            var transmit_address = transmitList.Value == ArrayList ? transmitListCursor : transmitPointer.Value;
            var receive_address = receiveList.Value == ArrayList ? receiveListCursor : receivePointer.Value;

            var outgoing = transmit_count > 0 ? ReadMemory(transmit_address, transmit_count) : new byte[0];
            var incoming = new byte[receive_count];
            var total = Math.Max(transmit_count, receive_count);
            for(var index = 0; index < total; index++)
            {
                var sent = index < transmit_count ? outgoing[index] : (byte)overReadCharacter.Value;
                var received = ExchangeSpiByte(sent);
                if(index < receive_count)
                {
                    incoming[index] = received;
                }
            }
            if(receive_count > 0)
            {
                WriteMemory(incoming, receive_address);
            }

            transmitAmount = (uint)transmit_count;
            receiveAmount = (uint)receive_count;
            AdvanceListCursors(transmit_count, receive_count);

            RaiseEvent(Registers.EventsEndTransmit);
            RaiseEvent(Registers.EventsEndReceive);
            RaiseEvent(Registers.EventsEnd);

            if(Shortcut(Shortcuts.EndStart) && !restartScheduled)
            {
                // The next transfer waits for a sync point so the CPU can run between back-to-back transfers.
                restartScheduled = true;
                machine.LocalTimeSource.ExecuteInNearestSyncedState(_ => RestartFromShortcut());
            }
        }

        private void RestartFromShortcut()
        {
            if(!restartScheduled)
            {
                return;
            }
            restartScheduled = false;
            if(mode != Mode.SpiMaster || !Shortcut(Shortcuts.EndStart))
            {
                return;
            }
            StartTask();
        }

        // ====================================================================
        // TWI and TWIM shared transaction
        // ====================================================================

        // Starts an I2C segment with a start or repeated start; false means no device answered the address.
        private bool BeginTwoWireSegment()
        {
            DeliverPendingWrite();

            var address = (int)twoWireAddress.Value;
            i2cChildren.TryGetByAddress(address, out var target);
            if(twoWireTarget != null && twoWireTarget != target)
            {
                twoWireTarget.FinishTransmission();
            }
            twoWireTarget = target;
            twoWireActive = true;

            if(target == null)
            {
                this.Log(LogLevel.Debug, "No I2C device answers address 0x{0:X2}", address);
                addressNotAcknowledged = true;
                addressNackError.Value = true;
                RaiseEvent(Registers.EventsError);
                return false;
            }
            addressNotAcknowledged = false;
            return true;
        }

        private void DeliverPendingWrite()
        {
            if(pendingWriteBytes.Count == 0)
            {
                return;
            }
            var bytes = pendingWriteBytes.ToArray();
            pendingWriteBytes.Clear();
            twoWireTarget?.Write(bytes);
        }

        private byte[] ReadFromTwoWireTarget(int count)
        {
            var result = Enumerable.Repeat(IdleBusByte, count).ToArray();
            if(twoWireTarget == null || count == 0)
            {
                return result;
            }
            var data = twoWireTarget.Read(count) ?? new byte[0];
            Array.Copy(data, result, Math.Min(data.Length, count));
            return result;
        }

        private void StopTwoWire()
        {
            DeliverPendingWrite();
            twoWireTarget?.FinishTransmission();
            twoWireTarget = null;
            twoWireActive = false;
            addressNotAcknowledged = false;
            suspended = false;
            legacyReceiving = false;
            legacyTransmitting = false;
            taskWaitingForResume = WaitingTask.None;
            RaiseEvent(Registers.EventsStopped);
        }

        private void EndTwoWireTransactionSilently()
        {
            if(twoWireActive)
            {
                DeliverPendingWrite();
                twoWireTarget?.FinishTransmission();
            }
        }

        private void SuspendTwoWire()
        {
            suspended = true;
            RaiseEvent(Registers.EventsSuspended);
        }

        // ====================================================================
        // Legacy TWI
        // ====================================================================

        private void StartLegacyTransmit()
        {
            legacyReceiving = false;
            legacyTransmitting = true;
            suspended = false;
            if(!BeginTwoWireSegment())
            {
                return;
            }
            lastSegmentWasWrite = true;
            ContinueLegacyTransfer();
        }

        private void StartLegacyReceive()
        {
            legacyTransmitting = false;
            legacyReceiving = true;
            suspended = false;
            receiveDataUnread = false;
            if(!BeginTwoWireSegment())
            {
                return;
            }
            lastSegmentWasWrite = false;
            ContinueLegacyTransfer();
        }

        // Moves the next byte if the bus is running: a written TXD goes out, or a read RXD lets the next byte in.
        private void ContinueLegacyTransfer()
        {
            if(mode != Mode.TwoWire || !twoWireActive || suspended || addressNotAcknowledged)
            {
                return;
            }

            if(legacyTransmitting && transmitDataWaiting)
            {
                transmitDataWaiting = false;
                pendingWriteBytes.Add((byte)transmitData.Value);
                RaiseEvent(Registers.EventsTransmitDataSent);
                FinishLegacyByte();
            }
            else if(legacyReceiving && !receiveDataUnread)
            {
                receiveData = ReadFromTwoWireTarget(1)[0];
                receiveDataUnread = true;
                RaiseEvent(Registers.EventsReady);
                FinishLegacyByte();
            }
        }

        private void FinishLegacyByte()
        {
            RaiseEvent(Registers.EventsByteBoundary);
            if(Shortcut(Shortcuts.ByteBoundaryStop))
            {
                StopTwoWire();
            }
            else if(Shortcut(Shortcuts.ByteBoundarySuspend))
            {
                SuspendTwoWire();
            }
        }

        // ====================================================================
        // TWIM
        // ====================================================================

        private void RunTwoWireMasterTransmit(bool continue_previous_write, int chain_depth)
        {
            RaiseEvent(Registers.EventsTransmitStarted);
            if(!continue_previous_write || !twoWireActive || addressNotAcknowledged)
            {
                if(!BeginTwoWireSegment())
                {
                    transmitAmount = 0;
                    return;
                }
            }
            lastSegmentWasWrite = true;

            var count = (int)transmitMaximumCount.Value;
            var address = transmitList.Value == ArrayList ? transmitListCursor : transmitPointer.Value;
            if(count > 0)
            {
                pendingWriteBytes.AddRange(ReadMemory(address, count));
            }
            transmitAmount = (uint)count;
            AdvanceListCursors(count, 0);

            RaiseEvent(Registers.EventsLastTransmit);

            if(Shortcut(Shortcuts.LastTransmitStartReceive))
            {
                if(ChainAllowed(chain_depth))
                {
                    RunTwoWireMasterReceive(chain_depth + 1);
                }
            }
            else if(Shortcut(Shortcuts.LastTransmitSuspend))
            {
                SuspendTwoWire();
            }
            else if(Shortcut(Shortcuts.LastTransmitStop))
            {
                StopTwoWire();
            }
        }

        private void RunTwoWireMasterReceive(int chain_depth)
        {
            RaiseEvent(Registers.EventsReceiveStarted);
            if(!BeginTwoWireSegment())
            {
                receiveAmount = 0;
                return;
            }
            lastSegmentWasWrite = false;

            var count = (int)receiveMaximumCount.Value;
            var address = receiveList.Value == ArrayList ? receiveListCursor : receivePointer.Value;
            if(count > 0)
            {
                WriteMemory(ReadFromTwoWireTarget(count), address);
            }
            receiveAmount = (uint)count;
            AdvanceListCursors(0, count);

            RaiseEvent(Registers.EventsLastReceive);

            if(Shortcut(Shortcuts.LastReceiveStartTransmit))
            {
                if(ChainAllowed(chain_depth))
                {
                    RunTwoWireMasterTransmit(continue_previous_write: false, chain_depth: chain_depth + 1);
                }
            }
            else if(Shortcut(Shortcuts.LastReceiveStop))
            {
                StopTwoWire();
            }
        }

        private bool ChainAllowed(int chain_depth)
        {
            if(chain_depth < MaximumShortcutChain)
            {
                return true;
            }
            this.Log(LogLevel.Warning, "TWIM shortcuts loop between transmit and receive, stopping the chain");
            return false;
        }

        // ====================================================================
        // EasyDMA
        // ====================================================================

        private byte[] ReadMemory(ulong address, int count)
        {
            return systemBus.ReadBytes(address, count, context: this);
        }

        private void WriteMemory(byte[] bytes, ulong address)
        {
            systemBus.WriteBytes(bytes, address, context: this);
        }

        // In array-list mode each transfer moves its pointer on by MAXCNT, so the next one uses the next buffer.
        private void AdvanceListCursors(int transmit_count, int receive_count)
        {
            transmitListCursor = transmitList.Value == ArrayList ? transmitListCursor + (ulong)transmit_count : transmitPointer.Value;
            receiveListCursor = receiveList.Value == ArrayList ? receiveListCursor + (ulong)receive_count : receivePointer.Value;
        }

        // ====================================================================
        // State
        // ====================================================================

        private const int LegacyBufferDepth = 2;
        private const ulong ArrayList = 1;
        private const byte IdleBusByte = 0xFF;
        private const int MaximumShortcutChain = 16;

        // Every INTEN bit any of the four modes defines.
        private const uint InterruptMask = (1u << 1) | (1u << 2) | (1u << 4) | (1u << 6) | (1u << 7) | (1u << 8) | (1u << 9)
            | (1u << 14) | (1u << 18) | (1u << 19) | (1u << 20) | (1u << 23) | (1u << 24);

        private static readonly Registers[] AllEvents =
        {
            Registers.EventsStopped, Registers.EventsReady, Registers.EventsEndReceive, Registers.EventsEnd,
            Registers.EventsTransmitDataSent, Registers.EventsEndTransmit, Registers.EventsError, Registers.EventsByteBoundary,
            Registers.EventsSuspended, Registers.EventsStarted, Registers.EventsTransmitStarted, Registers.EventsLastReceive,
            Registers.EventsLastTransmit,
        };

        private readonly IMachine machine;
        private readonly IBusController systemBus;
        private readonly NullRegistrationPointContainerHelper<ISPIPeripheral> spiChildren;
        private readonly SimpleContainerHelper<II2CPeripheral> i2cChildren;

        private readonly bool[] eventPending = new bool[64];
        private uint interruptEnable;
        private Mode mode;
        private bool suspended;

        private readonly Queue<byte> spiReceiveFifo = new Queue<byte>();
        private readonly Queue<byte> spiTransmitQueue = new Queue<byte>();
        private bool spiStartWaitingForResume;
        private bool restartScheduled;

        private II2CPeripheral twoWireTarget;
        private bool twoWireActive;
        private bool addressNotAcknowledged;
        private bool lastSegmentWasWrite;
        private readonly List<byte> pendingWriteBytes = new List<byte>();
        private bool legacyReceiving;
        private bool legacyTransmitting;
        private bool transmitDataWaiting;
        private bool receiveDataUnread;
        private byte receiveData;
        private WaitingTask taskWaitingForResume;

        private ulong transmitListCursor;
        private ulong receiveListCursor;
        private uint transmitAmount;
        private uint receiveAmount;

        private IValueRegisterField shortcuts;
        private IValueRegisterField transmitData;
        private IValueRegisterField receivePointer;
        private IValueRegisterField receiveMaximumCount;
        private IValueRegisterField receiveList;
        private IValueRegisterField transmitPointer;
        private IValueRegisterField transmitMaximumCount;
        private IValueRegisterField transmitList;
        private IValueRegisterField twoWireAddress;
        private IValueRegisterField overReadCharacter;
        private IFlagRegisterField overrunError;
        private IFlagRegisterField addressNackError;

        private enum Mode
        {
            Disabled = 0,
            Spi = 1,
            TwoWire = 5,
            TwoWireMaster = 6,
            SpiMaster = 7,
        }

        private enum WaitingTask
        {
            None,
            StartTransmit,
            StartReceive,
        }

        private enum Shortcuts
        {
            ByteBoundarySuspend = 0,
            ByteBoundaryStop = 1,
            LastTransmitStartReceive = 7,
            LastTransmitSuspend = 8,
            LastTransmitStop = 9,
            LastReceiveStartTransmit = 10,
            LastReceiveStop = 12,
            EndStart = 17,
        }

        private enum Registers
        {
            TasksStartReceive = 0x000,
            TasksStartTransmit = 0x008,
            TasksStart = 0x010,
            TasksStop = 0x014,
            TasksSuspend = 0x01C,
            TasksResume = 0x020,

            EventsStopped = 0x104,
            // READY in SPI mode and RXDREADY in TWI mode.
            EventsReady = 0x108,
            EventsEndReceive = 0x110,
            EventsEnd = 0x118,
            EventsTransmitDataSent = 0x11C,
            EventsEndTransmit = 0x120,
            EventsError = 0x124,
            EventsByteBoundary = 0x138,
            EventsSuspended = 0x148,
            // STARTED in SPIM mode and RXSTARTED in TWIM mode.
            EventsStarted = 0x14C,
            EventsReceiveStarted = 0x14C,
            EventsTransmitStarted = 0x150,
            EventsLastReceive = 0x15C,
            EventsLastTransmit = 0x160,

            Shortcuts = 0x200,
            InterruptEnable = 0x300,
            InterruptEnableSet = 0x304,
            InterruptEnableClear = 0x308,
            ErrorSource = 0x4C4,
            Enable = 0x500,
            // SCK or SCL.
            PinSelectClock = 0x508,
            // MOSI or SDA.
            PinSelectDataOut = 0x50C,
            PinSelectDataIn = 0x510,
            ReceiveData = 0x518,
            TransmitData = 0x51C,
            Frequency = 0x524,
            ReceivePointer = 0x534,
            ReceiveMaximumCount = 0x538,
            ReceiveAmount = 0x53C,
            ReceiveList = 0x540,
            TransmitPointer = 0x544,
            TransmitMaximumCount = 0x548,
            TransmitAmount = 0x54C,
            TransmitList = 0x550,
            Configuration = 0x554,
            TwoWireAddress = 0x588,
            OverReadCharacter = 0x5C0,
        }
    }
}
