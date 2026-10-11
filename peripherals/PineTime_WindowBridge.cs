// Shares the watch's non-screen outputs (motor, button, battery, steps, emulated time) with an outside window through a memory-mapped file.

using System;
using System.IO;
using System.IO.MemoryMappedFiles;
using System.Threading;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.Sensors;

namespace Antmicro.Renode.Peripherals.Miscellaneous
{
    public sealed class PineTime_WindowBridge : IGPIOReceiver
    {
        public PineTime_WindowBridge(IMachine machine, ILed motor, PineTime_SideButton button, PineTime_Battery battery, PineTime_BMA42x accelerometer)
        {
            this.machine = machine;
            this.motor = motor;
            this.button = button;
            this.battery = battery;
            this.accelerometer = accelerometer;
            motor.StateChanged += OnMotorChanged;
        }

        public void Reset()
        {
        }

        // The bridge listens to nothing on its pins; it sits on the GPIO port only to have a place in the machine.
        public void OnGPIO(int number, bool value)
        {
        }

        // ====================================================================
        // Sharing
        // ====================================================================

        // Writes the status block to the path every interval of host time; see frontend/pinetime_emulator/shared.py for its layout.
        public void ShareStatusThrough(string path, int intervalMilliseconds = 10)
        {
            lock(stateLock)
            {
                StopSharingStatus();
                statusFile = MemoryMappedFile.CreateFromFile(path, FileMode.Create, null, StatusBytes, MemoryMappedFileAccess.ReadWrite);
                statusView = statusFile.CreateViewAccessor(0, StatusBytes);
                statusPath = path;
                sequence = 0;
                statusView.Write(0, StatusMagic);
                statusView.Write(4, StatusVersion);
                WriteStatus(null);
                timer = new Timer(WriteStatus, null, intervalMilliseconds, intervalMilliseconds);
            }
        }

        public void StopSharingStatus()
        {
            lock(stateLock)
            {
                timer?.Dispose();
                statusView?.Dispose();
                statusFile?.Dispose();
                timer = null;
                statusView = null;
                statusFile = null;
                statusPath = null;
            }
        }

        public string SharedStatusPath { get { lock(stateLock) { return statusPath; } } }

        // Counts each time the motor starts, so a buzz shorter than the window's refresh is still seen.
        public uint MotorStarts { get { lock(stateLock) { return motorStarts; } } }

        private void OnMotorChanged(ILed led, bool running)
        {
            lock(stateLock)
            {
                if(running)
                {
                    motorStarts++;
                }
            }
        }

        // Writers bump the sequence to odd before and to even after, so a reader retries when it sees odd or a change.
        private void WriteStatus(object unused)
        {
            lock(stateLock)
            {
                if(statusView == null)
                {
                    return;
                }

                try
                {
                    var flags = (motor.State ? 1u : 0)
                        | (button.Pressed ? 2u : 0)
                        | (battery.ChargerConnected ? 4u : 0)
                        | (battery.Charging ? 8u : 0)
                        | (machine.IsPaused ? 16u : 0);

                    sequence++;
                    statusView.Write(8, sequence);
                    statusView.Write(12, flags);
                    statusView.Write(16, motorStarts);
                    statusView.Write(20, battery.Percent);
                    statusView.Write(24, battery.Volts);
                    statusView.Write(32, (ulong)machine.ElapsedVirtualTime.TimeElapsed.TotalMicroseconds);
                    statusView.Write(40, accelerometer.Steps);
                    sequence++;
                    statusView.Write(8, sequence);
                }
                catch(Exception error)
                {
                    this.Log(LogLevel.Warning, "Could not write the window status: {0}", error.Message);
                }
            }
        }

        // ====================================================================
        // Constants and fields
        // ====================================================================

        // The bytes "PTST" read as a little-endian number.
        private const uint StatusMagic = 0x54535450;
        private const uint StatusVersion = 1;
        private const int StatusBytes = 64;

        private readonly object stateLock = new object();
        private readonly IMachine machine;
        private readonly ILed motor;
        private readonly PineTime_SideButton button;
        private readonly PineTime_Battery battery;
        private readonly PineTime_BMA42x accelerometer;

        private MemoryMappedFile statusFile;
        private MemoryMappedViewAccessor statusView;
        private string statusPath;
        private Timer timer;
        private uint sequence;
        private uint motorStarts;
    }
}
