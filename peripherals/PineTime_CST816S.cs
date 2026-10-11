// SPDX-License-Identifier: GPL-3.0-or-later
// The PineTime's Hynitron CST816S touch panel: one finger, gestures, an active-low interrupt line and a reset pin.

using System;

using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.I2C;
using Antmicro.Renode.Peripherals.Input;
using Antmicro.Renode.Time;

namespace Antmicro.Renode.Peripherals.Sensors
{
    public class PineTime_CST816S : II2CPeripheral, IGPIOReceiver, IAbsolutePositionPointerInput
    {
        public PineTime_CST816S(IMachine machine)
        {
            this.machine = machine;
            IRQ = new GPIO();
            registers = new byte[RegisterCount];
            Reset();
        }

        // ====================================================================
        // Public interface
        // ====================================================================

        // The interrupt line, idle high, pulled low for a short pulse on each report.
        public GPIO IRQ { get; }

        // When true the chip ignores the bus once it has been idle for its auto-sleep time, as the real one does.
        public bool IgnoresBusWhenIdle { get; set; }

        public int MinX => 0;
        public int MinY => 0;
        public int MaxX => PanelSize - 1;
        public int MaxY => PanelSize - 1;

        public void Reset()
        {
            lock(stateLock)
            {
                RestoreRegisterDefaults();
                deepSleep = false;
                registerPointer = 0;
                ForgetTouch();
                pointerX = PanelSize / 2;
                pointerY = PanelSize / 2;
                pointerPressed = false;
                lastActivity = Now;
                IRQ.Set(true);
            }
        }

        // Puts a finger down.
        public void Touch(int x, int y)
        {
            lock(stateLock)
            {
                sequenceGeneration++;
                FingerDown(x, y);
            }
        }

        // Moves the finger that is already down.
        public void Move(int x, int y)
        {
            lock(stateLock)
            {
                sequenceGeneration++;
                FingerMove(x, y);
            }
        }

        // Lifts the finger; the mouse button argument only exists for the display window.
        public void Release(MouseButton button = MouseButton.Left)
        {
            lock(stateLock)
            {
                sequenceGeneration++;
                pointerPressed = false;
                FingerUp();
            }
        }

        // A short touch and lift at one place, reported as a single tap.
        public void Tap(int x, int y)
        {
            lock(stateLock)
            {
                var sequence = StartSequence();
                FingerDown(x, y);
                After(TapHoldMilliseconds, sequence, FingerUp);
            }
        }

        // Two quick taps at one place, reported as a double tap when the motion mask allows it.
        public void DoubleTap(int x, int y)
        {
            lock(stateLock)
            {
                var sequence = StartSequence();
                FingerDown(x, y);
                After(TapHoldMilliseconds, sequence, FingerUp);
                After(TapHoldMilliseconds + DoubleTapGapMilliseconds, sequence, () => FingerDown(x, y));
                After(2 * TapHoldMilliseconds + DoubleTapGapMilliseconds, sequence, FingerUp);
            }
        }

        // A finger held still until the panel reports a long press, then lifted.
        public void LongPress(int x, int y)
        {
            lock(stateLock)
            {
                var sequence = StartSequence();
                FingerDown(x, y);
                After(LongPressMilliseconds + LongPressReleaseDelayMilliseconds, sequence, FingerUp);
            }
        }

        // A finger dragged across the middle of the panel in the direction given: up, down, left or right.
        public void Swipe(string direction)
        {
            int startX, startY, endX, endY;
            switch(direction.Trim().ToLowerInvariant())
            {
                case "up":
                    startX = 120; startY = 200; endX = 120; endY = 40;
                    break;
                case "down":
                    startX = 120; startY = 40; endX = 120; endY = 200;
                    break;
                case "left":
                    startX = 200; startY = 120; endX = 40; endY = 120;
                    break;
                case "right":
                    startX = 40; startY = 120; endX = 200; endY = 120;
                    break;
                default:
                    throw new ArgumentException($"Unknown swipe direction '{direction}'; use up, down, left or right");
            }

            lock(stateLock)
            {
                var sequence = StartSequence();
                FingerDown(startX, startY);
                for(var step = 1; step <= SwipeSteps; step++)
                {
                    var x = startX + (endX - startX) * step / SwipeSteps;
                    var y = startY + (endY - startY) * step / SwipeSteps;
                    After((ulong)step * SwipeStepMilliseconds, sequence, () => FingerMove(x, y));
                }
                After((ulong)(SwipeSteps + 1) * SwipeStepMilliseconds, sequence, FingerUp);
            }
        }

        // The display window's mouse: moving it while pressed drags the finger.
        public void MoveTo(int x, int y)
        {
            lock(stateLock)
            {
                pointerX = x;
                pointerY = y;
                if(pointerPressed)
                {
                    sequenceGeneration++;
                    FingerMove(x, y);
                }
            }
        }

        // The display window's mouse button puts the finger down where the pointer is.
        public void Press(MouseButton button = MouseButton.Left)
        {
            lock(stateLock)
            {
                sequenceGeneration++;
                pointerPressed = true;
                FingerDown(pointerX, pointerY);
            }
        }

        // The reset pin, active low: held low the chip is off, and on its release the chip starts afresh.
        public void OnGPIO(int number, bool value)
        {
            if(number != ResetPin)
            {
                this.Log(LogLevel.Warning, "Ignored a signal on unknown input {0}", number);
                return;
            }

            lock(stateLock)
            {
                if(!value)
                {
                    heldInReset = true;
                    ForgetTouch();
                    IRQ.Set(true);
                }
                else if(heldInReset)
                {
                    heldInReset = false;
                    RestoreRegisterDefaults();
                    deepSleep = false;
                    lastActivity = Now;
                    this.Log(LogLevel.Debug, "Came out of reset");
                }
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
                if(!AnswersBus())
                {
                    this.Log(LogLevel.Debug, "Ignored a write to 0x{0:X2} while asleep", data[0]);
                    return;
                }

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
                if(!AnswersBus())
                {
                    this.Log(LogLevel.Debug, "Read while asleep gives an idle bus");
                    for(var index = 0; index < count; index++)
                    {
                        result[index] = 0xFF;
                    }
                    return result;
                }

                for(var index = 0; index < count; index++)
                {
                    result[index] = registers[registerPointer];
                    registerPointer = (byte)(registerPointer + 1);
                }
            }
            return result;
        }

        public void FinishTransmission()
        {
        }

        private bool AnswersBus()
        {
            if(heldInReset || deepSleep)
            {
                return false;
            }
            if(IgnoresBusWhenIdle && !fingerIsDown && registers[(int)Registers.DisableAutoSleep] == 0)
            {
                var idleMilliseconds = (Now - lastActivity).TotalMilliseconds;
                return idleMilliseconds < registers[(int)Registers.AutoSleepTime] * 1000.0;
            }
            return true;
        }

        private void WriteRegister(byte address, byte value)
        {
            switch((Registers)address)
            {
                case Registers.GestureId:
                case Registers.FingerCount:
                case Registers.EventAndXHigh:
                case Registers.XLow:
                case Registers.IdAndYHigh:
                case Registers.YLow:
                case Registers.ChipId:
                case Registers.ProjectId:
                case Registers.FirmwareVersion:
                    this.Log(LogLevel.Warning, "Ignored a write of 0x{0:X2} to read-only register 0x{1:X2}", value, address);
                    return;
                case Registers.PowerMode:
                    registers[address] = value;
                    if(value == DeepSleepCommand)
                    {
                        deepSleep = true;
                        ForgetTouch();
                        IRQ.Set(true);
                        this.Log(LogLevel.Debug, "Entered deep sleep; only a reset wakes it");
                    }
                    return;
                default:
                    registers[address] = value;
                    return;
            }
        }

        private void RestoreRegisterDefaults()
        {
            Array.Clear(registers, 0, registers.Length);
            registers[(int)Registers.EventAndXHigh] = (byte)(FingerEvent.None << EventShift);
            registers[(int)Registers.ChipId] = ChipIdValue;
            registers[(int)Registers.ProjectId] = ProjectIdValue;
            registers[(int)Registers.FirmwareVersion] = FirmwareVersionValue;
            registers[(int)Registers.MotionMask] = 0x00;
            registers[(int)Registers.InterruptPulseWidth] = 10;
            registers[(int)Registers.NormalScanPeriod] = 1;
            registers[(int)Registers.AutoSleepTime] = 2;
            registers[(int)Registers.InterruptControl] = InterruptOnMotion;
            registers[(int)Registers.AutoReset] = 5;
            registers[(int)Registers.LongPressTime] = 10;
        }

        // ====================================================================
        // Finger and gestures
        // ====================================================================

        private bool Listening()
        {
            if(heldInReset || deepSleep)
            {
                this.Log(LogLevel.Debug, "Ignored a touch while {0}", heldInReset ? "held in reset" : "in deep sleep");
                return false;
            }
            return true;
        }

        private void FingerDown(int x, int y)
        {
            if(!Listening())
            {
                return;
            }

            touchGeneration++;
            fingerIsDown = true;
            fingerMoved = false;
            fingerX = Clamp(x);
            fingerY = Clamp(y);
            startX = fingerX;
            startY = fingerY;
            touchStart = Now;
            lastActivity = touchStart;

            // A second touch soon after a held-back tap may become a double tap.
            secondTapPossible = tapPending && (touchStart - lastTapTime).TotalMilliseconds <= DoubleTapWindowMilliseconds;
            tapPending = false;

            gesture = Gesture.None;
            fingerEvent = FingerEvent.Down;
            UpdateReport();
            Report(InterruptOnTouch | InterruptOnChange);

            var touch = touchGeneration;
            AfterTouch(LongPressMilliseconds, touch, CheckLongPress);
            AfterTouch(ScanPeriodMilliseconds, touch, PeriodicReport);
        }

        private void FingerMove(int x, int y)
        {
            if(!Listening())
            {
                return;
            }
            if(!fingerIsDown)
            {
                this.Log(LogLevel.Warning, "Ignored a move with no finger down");
                return;
            }

            fingerX = Clamp(x);
            fingerY = Clamp(y);
            fingerEvent = FingerEvent.Contact;
            lastActivity = Now;

            var deltaX = fingerX - startX;
            var deltaY = fingerY - startY;
            if(Math.Abs(deltaX) > MoveTolerancePixels || Math.Abs(deltaY) > MoveTolerancePixels)
            {
                fingerMoved = true;
            }

            var newGesture = Gesture.None;
            if(gesture == Gesture.None && Math.Max(Math.Abs(deltaX), Math.Abs(deltaY)) >= SlideThresholdPixels)
            {
                if(Math.Abs(deltaY) >= Math.Abs(deltaX))
                {
                    newGesture = deltaY > 0 ? Gesture.SlideDown : Gesture.SlideUp;
                }
                else
                {
                    newGesture = deltaX > 0 ? Gesture.SlideRight : Gesture.SlideLeft;
                }
                gesture = newGesture;
            }

            UpdateReport();
            Report(InterruptOnTouch | InterruptOnChange | (newGesture != Gesture.None ? InterruptOnMotion : 0));
        }

        private void FingerUp()
        {
            if(!Listening())
            {
                return;
            }
            if(!fingerIsDown)
            {
                return;
            }

            touchGeneration++;
            fingerIsDown = false;
            fingerEvent = FingerEvent.Up;
            var now = Now;
            lastActivity = now;

            var newGesture = Gesture.None;
            var wasShortTap = gesture == Gesture.None && !fingerMoved && (now - touchStart).TotalMilliseconds < LongPressMilliseconds;
            if(wasShortTap)
            {
                if(secondTapPossible)
                {
                    newGesture = Gesture.DoubleTap;
                }
                else if(DoubleTapsEnabled)
                {
                    // The chip holds a tap back while it waits to see whether a second one follows.
                    tapPending = true;
                    lastTapTime = now;
                    var touch = touchGeneration;
                    AfterTouch(DoubleTapWindowMilliseconds, touch, ConfirmSingleTap);
                }
                else
                {
                    newGesture = Gesture.SingleTap;
                }
            }
            secondTapPossible = false;
            if(newGesture != Gesture.None)
            {
                gesture = newGesture;
            }

            UpdateReport();
            Report(InterruptOnChange | (newGesture != Gesture.None ? InterruptOnMotion : 0));
        }

        private void CheckLongPress()
        {
            if(fingerIsDown && !fingerMoved && gesture == Gesture.None)
            {
                gesture = Gesture.LongPress;
                fingerEvent = FingerEvent.Contact;
                UpdateReport();
                Report(InterruptOnMotion);
            }
        }

        private void ConfirmSingleTap()
        {
            if(tapPending)
            {
                tapPending = false;
                gesture = Gesture.SingleTap;
                UpdateReport();
                Report(InterruptOnMotion);
            }
        }

        private void PeriodicReport()
        {
            if(!fingerIsDown)
            {
                return;
            }
            if(fingerEvent == FingerEvent.Down)
            {
                fingerEvent = FingerEvent.Contact;
                UpdateReport();
            }
            Report(InterruptOnTouch);
            AfterTouch(ScanPeriodMilliseconds, touchGeneration, PeriodicReport);
        }

        private void ForgetTouch()
        {
            touchGeneration++;
            sequenceGeneration++;
            fingerIsDown = false;
            tapPending = false;
            secondTapPossible = false;
            irqPulseActive = false;
        }

        private void UpdateReport()
        {
            registers[(int)Registers.GestureId] = (byte)gesture;
            registers[(int)Registers.FingerCount] = (byte)(fingerIsDown ? 1 : 0);
            registers[(int)Registers.EventAndXHigh] = (byte)((fingerEvent << EventShift) | ((fingerX >> 8) & 0x0F));
            registers[(int)Registers.XLow] = (byte)fingerX;
            registers[(int)Registers.IdAndYHigh] = (byte)((fingerY >> 8) & 0x0F);
            registers[(int)Registers.YLow] = (byte)fingerY;
        }

        private bool DoubleTapsEnabled => (registers[(int)Registers.MotionMask] & MotionMaskDoubleTap) != 0;

        private static int Clamp(int value)
        {
            return Math.Max(0, Math.Min(PanelSize - 1, value));
        }

        // ====================================================================
        // Interrupt line
        // ====================================================================

        // Pulses the interrupt line when any of the given reasons is enabled in the interrupt control register.
        private void Report(int reasons)
        {
            if((registers[(int)Registers.InterruptControl] & reasons) == 0)
            {
                return;
            }
            if(irqPulseActive)
            {
                return;
            }

            irqPulseActive = true;
            IRQ.Set(false);
            var widthMicroseconds = (ulong)Math.Max(1, (int)registers[(int)Registers.InterruptPulseWidth]) * 100UL;
            machine.ScheduleAction(TimeInterval.FromMicroseconds(widthMicroseconds), _ =>
            {
                lock(stateLock)
                {
                    irqPulseActive = false;
                    IRQ.Set(true);
                }
            }, "CST816S interrupt pulse end");
        }

        // ====================================================================
        // Timing
        // ====================================================================

        private TimeInterval Now => machine.ElapsedVirtualTime.TimeElapsed;

        private ulong ScanPeriodMilliseconds => Math.Max(1UL, registers[(int)Registers.NormalScanPeriod]) * 10UL;

        // Starts a scripted gesture, cancelling the steps of any earlier one.
        private ulong StartSequence()
        {
            return ++sequenceGeneration;
        }

        // Runs a step of a scripted gesture later, unless a newer gesture or a reset has replaced it.
        private void After(ulong milliseconds, ulong sequence, Action step)
        {
            machine.ScheduleAction(TimeInterval.FromMilliseconds(milliseconds), _ =>
            {
                lock(stateLock)
                {
                    if(sequence != sequenceGeneration)
                    {
                        return;
                    }
                    step();
                }
            }, "CST816S gesture step");
        }

        // Runs a check later, unless the finger has gone down or up again since.
        private void AfterTouch(ulong milliseconds, ulong touch, Action check)
        {
            machine.ScheduleAction(TimeInterval.FromMilliseconds(milliseconds), _ =>
            {
                lock(stateLock)
                {
                    if(touch == touchGeneration)
                    {
                        check();
                    }
                }
            }, "CST816S touch timer");
        }

        // ====================================================================
        // State
        // ====================================================================

        private readonly IMachine machine;
        private readonly byte[] registers;
        private readonly object stateLock = new object();

        private byte registerPointer;
        private bool heldInReset;
        private bool deepSleep;
        private TimeInterval lastActivity;

        private bool fingerIsDown;
        private bool fingerMoved;
        private int fingerX;
        private int fingerY;
        private int startX;
        private int startY;
        private TimeInterval touchStart;
        private Gesture gesture;
        private int fingerEvent;

        private bool tapPending;
        private bool secondTapPossible;
        private TimeInterval lastTapTime;

        private int pointerX;
        private int pointerY;
        private bool pointerPressed;

        private bool irqPulseActive;
        private ulong touchGeneration;
        private ulong sequenceGeneration;

        // ====================================================================
        // Constants
        // ====================================================================

        private const int RegisterCount = 256;
        private const int PanelSize = 240;
        private const int ResetPin = 0;

        private const byte ChipIdValue = 0xB4;
        private const byte ProjectIdValue = 0x00;
        private const byte FirmwareVersionValue = 0x01;
        private const byte DeepSleepCommand = 0x03;

        private const byte InterruptOnTouch = 0x40;
        private const byte InterruptOnChange = 0x20;
        private const byte InterruptOnMotion = 0x10;
        private const byte MotionMaskDoubleTap = 0x01;

        private const int EventShift = 6;
        private const int SlideThresholdPixels = 40;
        private const int MoveTolerancePixels = 10;

        private const ulong LongPressMilliseconds = 1000;
        private const ulong LongPressReleaseDelayMilliseconds = 200;
        private const ulong TapHoldMilliseconds = 60;
        private const ulong DoubleTapGapMilliseconds = 120;
        private const ulong DoubleTapWindowMilliseconds = 300;
        private const int SwipeSteps = 8;
        private const ulong SwipeStepMilliseconds = 20;

        private static class FingerEvent
        {
            public const int Down = 0;
            public const int Up = 1;
            public const int Contact = 2;
            public const int None = 3;
        }

        private enum Gesture : byte
        {
            None = 0x00,
            SlideDown = 0x01,
            SlideUp = 0x02,
            SlideLeft = 0x03,
            SlideRight = 0x04,
            SingleTap = 0x05,
            DoubleTap = 0x0B,
            LongPress = 0x0C,
        }

        private enum Registers : byte
        {
            GestureId = 0x01,
            FingerCount = 0x02,
            EventAndXHigh = 0x03,
            XLow = 0x04,
            IdAndYHigh = 0x05,
            YLow = 0x06,
            PowerMode = 0xA5,
            ChipId = 0xA7,
            ProjectId = 0xA8,
            FirmwareVersion = 0xA9,
            MotionMask = 0xEC,
            InterruptPulseWidth = 0xED,
            NormalScanPeriod = 0xEE,
            AutoSleepTime = 0xF9,
            InterruptControl = 0xFA,
            AutoReset = 0xFB,
            LongPressTime = 0xFC,
            DisableAutoSleep = 0xFE,
        }
    }
}
