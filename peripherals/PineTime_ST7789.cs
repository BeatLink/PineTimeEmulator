// The PineTime's 240x240 screen and its Sitronix ST7789 controller, fed over SPI with separate data/command, reset and backlight pins.

using System;
using System.Collections.Generic;
using System.IO;
using System.IO.MemoryMappedFiles;
using System.Security.Cryptography;
using System.Text;

using Antmicro.Renode.Backends.Display;
using Antmicro.Renode.Core;
using Antmicro.Renode.Logging;
using Antmicro.Renode.Peripherals.SPI;
using Antmicro.Renode.Utilities;

namespace Antmicro.Renode.Peripherals.Video
{
    public class PineTime_ST7789 : AutoRepaintingVideo, ISPIPeripheral, IGPIOReceiver
    {
        public PineTime_ST7789(IMachine machine) : base(machine)
        {
            frame_memory = new ushort[MemoryColumns * MemoryRows];
            command_parameters = new List<byte>();
            pending_read_bytes = new Queue<byte>();
            pixel_bytes = new List<byte>();

            Endianess = ELFSharp.ELF.Endianess.BigEndian;
            Reconfigure(ScreenWidth, ScreenHeight, PixelFormat.RGBA8888);
            Reset();
        }

        // ====================================================================
        // Renode hooks
        // ====================================================================

        public override void Reset()
        {
            lock(state_lock)
            {
                Array.Clear(frame_memory, 0, frame_memory.Length);
                data_command_pin_high = false;
                reset_pin_low = false;
                backlight_pin_low = new bool[] { false, false, false };
                backlight_level = 3;
                ResetController();
            }
        }

        // Inputs: 0 data/command, 1 reset (active low), 2-4 backlight low, mid and high (active low).
        public void OnGPIO(int number, bool value)
        {
            lock(state_lock)
            {
                switch(number)
                {
                    case DataCommandInput:
                        data_command_pin_high = value;
                        break;
                    case ResetInput:
                        if(!value && !reset_pin_low)
                        {
                            ResetController();
                        }
                        reset_pin_low = !value;
                        break;
                    case BacklightLowInput:
                    case BacklightMidInput:
                    case BacklightHighInput:
                        backlight_pin_low[number - BacklightLowInput] = !value;
                        backlight_level = BacklightLevelFromPins();
                        break;
                    default:
                        this.Log(LogLevel.Warning, "Ignored unknown input {0}", number);
                        break;
                }
            }
        }

        public byte Transmit(byte data)
        {
            lock(state_lock)
            {
                if(reset_pin_low)
                {
                    return 0;
                }

                if(!data_command_pin_high)
                {
                    StartCommand(data);
                    return 0;
                }

                if(pending_read_bytes.Count > 0)
                {
                    return pending_read_bytes.Dequeue();
                }

                ReceiveData(data);
                return 0;
            }
        }

        // The chip forgets only a half-sent byte when its select line lifts, and commands are byte-sized here.
        public void FinishTransmission()
        {
        }

        protected override void Repaint()
        {
            lock(state_lock)
            {
                for(var y = 0; y < ScreenHeight; y++)
                {
                    for(var x = 0; x < ScreenWidth; x++)
                    {
                        var colour = ShownColour(x, y);
                        var offset = (y * ScreenWidth + x) * 4;
                        buffer[offset] = (byte)(colour >> 16);
                        buffer[offset + 1] = (byte)(colour >> 8);
                        buffer[offset + 2] = (byte)colour;
                        buffer[offset + 3] = 0xFF;
                    }
                }
                WriteSharedFrame();
            }
        }

        // ====================================================================
        // State for the monitor and tests
        // ====================================================================

        public bool Sleeping { get { lock(state_lock) { return sleeping; } } }

        public bool DisplayOn { get { lock(state_lock) { return display_on; } } }

        // True after INVON; the PineTime glass shows true colours only in this state.
        public bool Inverted { get { lock(state_lock) { return inversion_on; } } }

        public bool InReset { get { lock(state_lock) { return reset_pin_low; } } }

        public bool Scrolling { get { lock(state_lock) { return scroll_mode; } } }

        public int ScrollStart { get { lock(state_lock) { return scroll_start; } } }

        public int TopFixedArea { get { lock(state_lock) { return top_fixed_area; } } }

        public int ScrollArea { get { lock(state_lock) { return scroll_area; } } }

        public int BottomFixedArea { get { lock(state_lock) { return bottom_fixed_area; } } }

        public byte MemoryAccessControl { get { lock(state_lock) { return memory_access_control; } } }

        public byte PixelFormatSetting { get { lock(state_lock) { return interface_pixel_format; } } }

        public bool TearingEffectOn { get { lock(state_lock) { return tearing_effect_on; } } }

        // 0 is off, 1 low, 2 mid, 3 full; the backlight pins overwrite it whenever one changes.
        public int BacklightLevel
        {
            get { lock(state_lock) { return backlight_level; } }
            set { lock(state_lock) { backlight_level = Math.Max(0, Math.Min(3, value)); } }
        }

        // The colour the glass shows at (x, y), as 0xRRGGBB.
        public uint ShownPixel(int x, int y)
        {
            lock(state_lock)
            {
                CheckOnScreen(x, y);
                return ShownColour(x, y);
            }
        }

        // The RGB565 value stored in frame memory at a physical column and row.
        public ushort MemoryPixel(int column, int row)
        {
            lock(state_lock)
            {
                if(column < 0 || column >= MemoryColumns || row < 0 || row >= MemoryRows)
                {
                    throw new ArgumentOutOfRangeException("column/row", "Outside the 240x320 frame memory");
                }
                return frame_memory[row * MemoryColumns + column];
            }
        }

        // A SHA-256 hex digest of the shown image, for golden-image tests.
        public string ShownImageHash()
        {
            var rgb_bytes = ShownImageBytes(false);
            using(var sha = SHA256.Create())
            {
                var digest = sha.ComputeHash(rgb_bytes);
                var text = new StringBuilder();
                foreach(var digest_byte in digest)
                {
                    text.Append(digest_byte.ToString("x2"));
                }
                return text.ToString();
            }
        }

        public void SaveScreenshot(string path)
        {
            var image = new RawImageData(ShownImageBytes(true), ScreenWidth, ScreenHeight);
            using(var png = image.ToPng())
            using(var file = File.Create(path))
            {
                png.CopyTo(file);
            }
        }

        // ====================================================================
        // Frame sharing for an outside window
        // ====================================================================

        // Each repaint copies the shown image into a memory-mapped file at the path; see frontend/pinetime_emulator/shared.py for its layout.
        public void ShareFramesThrough(string path)
        {
            lock(state_lock)
            {
                StopSharingFrames();
                var size = SharedFrameHeaderBytes + ScreenWidth * ScreenHeight * 3;
                shared_frame_file = MemoryMappedFile.CreateFromFile(path, FileMode.Create, null, size, MemoryMappedFileAccess.ReadWrite);
                shared_frame_view = shared_frame_file.CreateViewAccessor(0, size);
                shared_frame_path = path;
                shared_frame_pixels = new byte[ScreenWidth * ScreenHeight * 3];
                shared_frame_sequence = 0;
                shared_frame_count = 0;
                shared_frame_view.Write(0, SharedFrameMagic);
                shared_frame_view.Write(4, SharedFrameVersion);
                shared_frame_view.Write(16, (ushort)ScreenWidth);
                shared_frame_view.Write(18, (ushort)ScreenHeight);
                shared_frame_view.Write(28, (uint)SharedFrameHeaderBytes);
                WriteSharedFrame();
            }
        }

        public void StopSharingFrames()
        {
            lock(state_lock)
            {
                shared_frame_view?.Dispose();
                shared_frame_file?.Dispose();
                shared_frame_view = null;
                shared_frame_file = null;
                shared_frame_path = null;
            }
        }

        public string SharedFramePath { get { lock(state_lock) { return shared_frame_path; } } }

        // Writers bump the sequence to odd before and to even after, so a reader retries when it sees odd or a change.
        private void WriteSharedFrame()
        {
            if(shared_frame_view == null)
            {
                return;
            }

            for(var y = 0; y < ScreenHeight; y++)
            {
                for(var x = 0; x < ScreenWidth; x++)
                {
                    var colour = ShownColour(x, y);
                    var offset = (y * ScreenWidth + x) * 3;
                    shared_frame_pixels[offset] = (byte)(colour >> 16);
                    shared_frame_pixels[offset + 1] = (byte)(colour >> 8);
                    shared_frame_pixels[offset + 2] = (byte)colour;
                }
            }

            var flags = (sleeping ? 1u : 0) | (display_on ? 2u : 0) | (reset_pin_low ? 4u : 0) | (inversion_on ? 8u : 0);
            shared_frame_sequence++;
            shared_frame_view.Write(8, shared_frame_sequence);
            shared_frame_count++;
            shared_frame_view.Write(12, shared_frame_count);
            shared_frame_view.Write(20, flags);
            shared_frame_view.Write(24, (uint)backlight_level);
            shared_frame_view.WriteArray(SharedFrameHeaderBytes, shared_frame_pixels, 0, shared_frame_pixels.Length);
            shared_frame_sequence++;
            shared_frame_view.Write(8, shared_frame_sequence);
        }

        // ====================================================================
        // Commands
        // ====================================================================

        private void StartCommand(byte command)
        {
            current_command = command;
            command_parameters.Clear();
            pending_read_bytes.Clear();
            pixel_bytes.Clear();

            switch(command)
            {
                case (byte)Command.NoOperation:
                    break;
                case (byte)Command.SoftwareReset:
                    ResetController();
                    break;
                case (byte)Command.SleepIn:
                    sleeping = true;
                    break;
                case (byte)Command.SleepOut:
                    sleeping = false;
                    break;
                case (byte)Command.PartialModeOn:
                    partial_mode = true;
                    scroll_mode = false;
                    break;
                case (byte)Command.NormalModeOn:
                    partial_mode = false;
                    scroll_mode = false;
                    break;
                case (byte)Command.InversionOff:
                    inversion_on = false;
                    break;
                case (byte)Command.InversionOn:
                    inversion_on = true;
                    break;
                case (byte)Command.DisplayOff:
                    display_on = false;
                    break;
                case (byte)Command.DisplayOn:
                    display_on = true;
                    break;
                case (byte)Command.TearingEffectOff:
                    tearing_effect_on = false;
                    break;
                case (byte)Command.IdleModeOff:
                    idle_mode = false;
                    break;
                case (byte)Command.IdleModeOn:
                    idle_mode = true;
                    break;
                case (byte)Command.MemoryWrite:
                    write_column = column_start;
                    write_row = row_start;
                    break;
                case (byte)Command.MemoryWriteContinue:
                    break;
                case (byte)Command.ReadDisplayId:
                case (byte)Command.ReadDisplayStatus:
                case (byte)Command.ReadDisplayPowerMode:
                case (byte)Command.ReadMemoryAccessControl:
                case (byte)Command.ReadPixelFormat:
                case (byte)Command.ReadImageMode:
                case (byte)Command.ReadSignalMode:
                case (byte)Command.ReadSelfDiagnostic:
                case (byte)Command.ReadId1:
                case (byte)Command.ReadId2:
                case (byte)Command.ReadId3:
                    QueueReadBytes(command);
                    break;
                default:
                    // Panel tuning commands such as porch, gate and gamma settings take their parameters and change nothing visible.
                    this.Log(LogLevel.Noisy, "Accepted command 0x{0:X2} without modelling it", command);
                    break;
            }
        }

        private void ReceiveData(byte data)
        {
            switch(current_command)
            {
                case (int)Command.MemoryWrite:
                case (int)Command.MemoryWriteContinue:
                    ReceivePixelByte(data);
                    return;
                case NoCommand:
                    return;
            }

            command_parameters.Add(data);
            var parameter_count = command_parameters.Count;

            switch(current_command)
            {
                case (int)Command.ColumnAddressSet:
                    if(parameter_count == 4)
                    {
                        column_start = Word(0);
                        column_end = Word(2);
                    }
                    break;
                case (int)Command.RowAddressSet:
                    if(parameter_count == 4)
                    {
                        row_start = Word(0);
                        row_end = Word(2);
                    }
                    break;
                case (int)Command.VerticalScrollDefinition:
                    if(parameter_count == 6)
                    {
                        top_fixed_area = Word(0);
                        scroll_area = Word(2);
                        bottom_fixed_area = Word(4);
                        if(top_fixed_area + scroll_area + bottom_fixed_area != MemoryRows)
                        {
                            this.Log(LogLevel.Warning, "Scroll areas {0}+{1}+{2} do not add up to {3} rows", top_fixed_area, scroll_area, bottom_fixed_area, MemoryRows);
                        }
                    }
                    break;
                case (int)Command.VerticalScrollStartAddress:
                    if(parameter_count == 2)
                    {
                        scroll_start = Word(0);
                        scroll_mode = true;
                    }
                    break;
                case (int)Command.TearingEffectOn:
                    if(parameter_count == 1)
                    {
                        tearing_effect_on = true;
                    }
                    break;
                case (int)Command.MemoryAccessControl:
                    if(parameter_count == 1)
                    {
                        memory_access_control = data;
                    }
                    break;
                case (int)Command.InterfacePixelFormat:
                    if(parameter_count == 1)
                    {
                        interface_pixel_format = data;
                    }
                    break;
            }
        }

        private int Word(int first_index)
        {
            return (command_parameters[first_index] << 8) | command_parameters[first_index + 1];
        }

        // ====================================================================
        // Reads
        // ====================================================================

        private void QueueReadBytes(byte command)
        {
            switch(command)
            {
                case (byte)Command.ReadDisplayId:
                    QueueBytes(0x00, ManufacturerId, VersionId, ModuleId);
                    break;
                case (byte)Command.ReadDisplayStatus:
                    QueueBytes(0x00, StatusFirstByte(), StatusSecondByte(), StatusThirdByte(), 0x00);
                    break;
                case (byte)Command.ReadDisplayPowerMode:
                    QueueBytes(PowerModeByte());
                    break;
                case (byte)Command.ReadMemoryAccessControl:
                    QueueBytes(memory_access_control);
                    break;
                case (byte)Command.ReadPixelFormat:
                    QueueBytes(interface_pixel_format);
                    break;
                case (byte)Command.ReadImageMode:
                    QueueBytes((byte)((scroll_mode ? 0x80 : 0) | (inversion_on ? 0x20 : 0)));
                    break;
                case (byte)Command.ReadSignalMode:
                    QueueBytes((byte)(tearing_effect_on ? 0x80 : 0));
                    break;
                case (byte)Command.ReadSelfDiagnostic:
                    QueueBytes(0x00);
                    break;
                case (byte)Command.ReadId1:
                    QueueBytes(ManufacturerId);
                    break;
                case (byte)Command.ReadId2:
                    QueueBytes(VersionId);
                    break;
                case (byte)Command.ReadId3:
                    QueueBytes(ModuleId);
                    break;
            }
        }

        private void QueueBytes(params byte[] bytes)
        {
            foreach(var read_byte in bytes)
            {
                pending_read_bytes.Enqueue(read_byte);
            }
        }

        private byte PowerModeByte()
        {
            var value = 0;
            if(!sleeping) value |= 0x80 | 0x10;
            if(idle_mode) value |= 0x40;
            if(partial_mode) value |= 0x20;
            if(!partial_mode) value |= 0x08;
            if(display_on) value |= 0x04;
            return (byte)value;
        }

        private byte StatusFirstByte()
        {
            return (byte)((sleeping ? 0 : 0x80) | ((memory_access_control & 0xFC) >> 1));
        }

        private byte StatusSecondByte()
        {
            var value = (interface_pixel_format & 0x07) << 4;
            if(idle_mode) value |= 0x08;
            if(partial_mode) value |= 0x04;
            if(!sleeping) value |= 0x02;
            if(!partial_mode) value |= 0x01;
            return (byte)value;
        }

        private byte StatusThirdByte()
        {
            var value = 0;
            if(scroll_mode) value |= 0x80;
            if(inversion_on) value |= 0x20;
            if(display_on) value |= 0x04;
            if(tearing_effect_on) value |= 0x02;
            return (byte)value;
        }

        // ====================================================================
        // Frame memory writes
        // ====================================================================

        private void ReceivePixelByte(byte data)
        {
            pixel_bytes.Add(data);
            var bits_per_pixel = interface_pixel_format & 0x07;

            if(bits_per_pixel == ColourFormat16Bit)
            {
                if(pixel_bytes.Count == 2)
                {
                    StorePixel((ushort)((pixel_bytes[0] << 8) | pixel_bytes[1]));
                    pixel_bytes.Clear();
                }
                return;
            }

            if(bits_per_pixel == ColourFormat12Bit)
            {
                if(pixel_bytes.Count == 3)
                {
                    StorePixel(Rgb444ToRgb565(pixel_bytes[0] >> 4, pixel_bytes[0] & 0xF, pixel_bytes[1] >> 4));
                    StorePixel(Rgb444ToRgb565(pixel_bytes[1] & 0xF, pixel_bytes[2] >> 4, pixel_bytes[2] & 0xF));
                    pixel_bytes.Clear();
                }
                return;
            }

            // The 18-bit and 24-bit formats send one byte per colour, using the top bits.
            if(pixel_bytes.Count == 3)
            {
                StorePixel((ushort)(((pixel_bytes[0] >> 3) << 11) | ((pixel_bytes[1] >> 2) << 5) | (pixel_bytes[2] >> 3)));
                pixel_bytes.Clear();
            }
        }

        private static ushort Rgb444ToRgb565(int red, int green, int blue)
        {
            return (ushort)((((red << 1) | (red >> 3)) << 11) | (((green << 2) | (green >> 2)) << 5) | ((blue << 1) | (blue >> 3)));
        }

        private void StorePixel(ushort colour)
        {
            var rows_and_columns_swapped = (memory_access_control & RowColumnExchangeBit) != 0;
            var logical_width = rows_and_columns_swapped ? MemoryRows : MemoryColumns;
            var logical_height = rows_and_columns_swapped ? MemoryColumns : MemoryRows;

            if(write_column < logical_width && write_row < logical_height)
            {
                var physical_column = rows_and_columns_swapped ? write_row : write_column;
                var physical_row = rows_and_columns_swapped ? write_column : write_row;
                if((memory_access_control & ColumnMirrorBit) != 0)
                {
                    physical_column = MemoryColumns - 1 - physical_column;
                }
                if((memory_access_control & RowMirrorBit) != 0)
                {
                    physical_row = MemoryRows - 1 - physical_row;
                }
                frame_memory[physical_row * MemoryColumns + physical_column] = colour;
            }

            AdvanceWritePointer();
        }

        private void AdvanceWritePointer()
        {
            write_column++;
            if(write_column <= column_end)
            {
                return;
            }

            write_column = column_start;
            write_row++;
            if(write_row > row_end)
            {
                write_row = row_start;
            }
        }

        // ====================================================================
        // What the glass shows
        // ====================================================================

        private uint ShownColour(int x, int y)
        {
            if(reset_pin_low || sleeping || !display_on || backlight_level == 0)
            {
                return 0;
            }

            var colour = frame_memory[ShownMemoryRow(y) * MemoryColumns + x];
            if(!inversion_on)
            {
                colour = (ushort)~colour;
            }

            var red = (colour >> 11) & 0x1F;
            var green = (colour >> 5) & 0x3F;
            var blue = colour & 0x1F;
            if((memory_access_control & BlueGreenRedOrderBit) != 0)
            {
                var swapped = red;
                red = blue;
                blue = swapped;
            }

            var red_8 = Dim((red << 3) | (red >> 2));
            var green_8 = Dim((green << 2) | (green >> 4));
            var blue_8 = Dim((blue << 3) | (blue >> 2));
            return (uint)((red_8 << 16) | (green_8 << 8) | blue_8);
        }

        private int ShownMemoryRow(int screen_line)
        {
            if(!scroll_mode || scroll_area <= 0)
            {
                return screen_line;
            }
            if(screen_line < top_fixed_area || screen_line >= top_fixed_area + scroll_area)
            {
                return screen_line;
            }

            var offset = (screen_line - top_fixed_area) + (scroll_start - top_fixed_area);
            var wrapped = ((offset % scroll_area) + scroll_area) % scroll_area;
            return Math.Min(top_fixed_area + wrapped, MemoryRows - 1);
        }

        private int Dim(int channel)
        {
            return channel * BacklightPercent[backlight_level] / 100;
        }

        private int BacklightLevelFromPins()
        {
            for(var level = 3; level >= 1; level--)
            {
                if(backlight_pin_low[level - 1])
                {
                    return level;
                }
            }
            return 0;
        }

        private byte[] ShownImageBytes(bool with_alpha)
        {
            var bytes_per_pixel = with_alpha ? 4 : 3;
            var image = new byte[ScreenWidth * ScreenHeight * bytes_per_pixel];
            lock(state_lock)
            {
                for(var y = 0; y < ScreenHeight; y++)
                {
                    for(var x = 0; x < ScreenWidth; x++)
                    {
                        var colour = ShownColour(x, y);
                        var offset = (y * ScreenWidth + x) * bytes_per_pixel;
                        image[offset] = (byte)(colour >> 16);
                        image[offset + 1] = (byte)(colour >> 8);
                        image[offset + 2] = (byte)colour;
                        if(with_alpha)
                        {
                            image[offset + 3] = 0xFF;
                        }
                    }
                }
            }
            return image;
        }

        private static void CheckOnScreen(int x, int y)
        {
            if(x < 0 || x >= ScreenWidth || y < 0 || y >= ScreenHeight)
            {
                throw new ArgumentOutOfRangeException("x/y", "Outside the 240x240 screen");
            }
        }

        // ====================================================================
        // Controller reset
        // ====================================================================

        // Both the reset pin and SWRESET restore these defaults and leave frame memory alone.
        private void ResetController()
        {
            current_command = NoCommand;
            command_parameters.Clear();
            pending_read_bytes.Clear();
            pixel_bytes.Clear();

            sleeping = true;
            display_on = false;
            inversion_on = false;
            idle_mode = false;
            partial_mode = false;
            tearing_effect_on = false;
            memory_access_control = 0;
            interface_pixel_format = 0x66;

            column_start = 0;
            column_end = MemoryColumns - 1;
            row_start = 0;
            row_end = MemoryRows - 1;
            write_column = 0;
            write_row = 0;

            scroll_mode = false;
            top_fixed_area = 0;
            scroll_area = MemoryRows;
            bottom_fixed_area = 0;
            scroll_start = 0;
        }

        // ====================================================================
        // Constants and fields
        // ====================================================================

        private const int ScreenWidth = 240;
        private const int ScreenHeight = 240;
        private const int MemoryColumns = 240;
        private const int MemoryRows = 320;

        private const int DataCommandInput = 0;
        private const int ResetInput = 1;
        private const int BacklightLowInput = 2;
        private const int BacklightMidInput = 3;
        private const int BacklightHighInput = 4;

        private const int NoCommand = -1;
        private const int ColourFormat12Bit = 3;
        private const int ColourFormat16Bit = 5;

        private const byte RowMirrorBit = 0x80;
        private const byte ColumnMirrorBit = 0x40;
        private const byte RowColumnExchangeBit = 0x20;
        private const byte BlueGreenRedOrderBit = 0x08;

        private const byte ManufacturerId = 0x85;
        private const byte VersionId = 0x85;
        private const byte ModuleId = 0x52;

        private static readonly int[] BacklightPercent = { 0, 40, 70, 100 };

        // The bytes "PTFB" read as a little-endian number.
        private const uint SharedFrameMagic = 0x42465450;
        private const uint SharedFrameVersion = 1;
        private const int SharedFrameHeaderBytes = 64;

        private readonly object state_lock = new object();
        private readonly ushort[] frame_memory;
        private readonly List<byte> command_parameters;
        private readonly Queue<byte> pending_read_bytes;
        private readonly List<byte> pixel_bytes;

        private bool data_command_pin_high;
        private bool reset_pin_low;
        private bool[] backlight_pin_low;
        private int backlight_level;

        private int current_command;
        private bool sleeping;
        private bool display_on;
        private bool inversion_on;
        private bool idle_mode;
        private bool partial_mode;
        private bool tearing_effect_on;
        private byte memory_access_control;
        private byte interface_pixel_format;

        private int column_start;
        private int column_end;
        private int row_start;
        private int row_end;
        private int write_column;
        private int write_row;

        private MemoryMappedFile shared_frame_file;
        private MemoryMappedViewAccessor shared_frame_view;
        private string shared_frame_path;
        private byte[] shared_frame_pixels;
        private uint shared_frame_sequence;
        private uint shared_frame_count;

        private bool scroll_mode;
        private int top_fixed_area;
        private int scroll_area;
        private int bottom_fixed_area;
        private int scroll_start;

        private enum Command : byte
        {
            NoOperation = 0x00,
            SoftwareReset = 0x01,
            ReadDisplayId = 0x04,
            ReadDisplayStatus = 0x09,
            ReadDisplayPowerMode = 0x0A,
            ReadMemoryAccessControl = 0x0B,
            ReadPixelFormat = 0x0C,
            ReadImageMode = 0x0D,
            ReadSignalMode = 0x0E,
            ReadSelfDiagnostic = 0x0F,
            SleepIn = 0x10,
            SleepOut = 0x11,
            PartialModeOn = 0x12,
            NormalModeOn = 0x13,
            InversionOff = 0x20,
            InversionOn = 0x21,
            DisplayOff = 0x28,
            DisplayOn = 0x29,
            ColumnAddressSet = 0x2A,
            RowAddressSet = 0x2B,
            MemoryWrite = 0x2C,
            VerticalScrollDefinition = 0x33,
            TearingEffectOff = 0x34,
            TearingEffectOn = 0x35,
            MemoryAccessControl = 0x36,
            VerticalScrollStartAddress = 0x37,
            IdleModeOff = 0x38,
            IdleModeOn = 0x39,
            InterfacePixelFormat = 0x3A,
            MemoryWriteContinue = 0x3C,
            ReadId1 = 0xDA,
            ReadId2 = 0xDB,
            ReadId3 = 0xDC,
        }
    }
}
