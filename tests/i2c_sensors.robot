# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Include Sensor Models
Suite Teardown    Teardown
Test Setup        Create Sensor Machine
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${PERIPHERALS}    ${CURDIR}/../peripherals
@{PLATFORM}       twi: I2C.NRF52840_I2C @ sysbus 0x40003000
...               touch: Sensors.PineTime_CST816S @ twi 0x15
...               accel: Sensors.PineTime_BMA42x @ twi 0x18
...               accel5: Sensors.PineTime_BMA42x @ twi 0x19 { model: \\"BMA425\\" }
...               heart: Sensors.PineTime_HRS3300 @ twi 0x44

*** Keywords ***
Include Sensor Models
    Setup
    Execute Command           include @${PERIPHERALS}/PineTime_CST816S.cs
    Execute Command           include @${PERIPHERALS}/PineTime_BMA42x.cs
    Execute Command           include @${PERIPHERALS}/PineTime_HRS3300.cs

Create Sensor Machine
    Execute Command           mach create
    FOR    ${line}    IN    @{PLATFORM}
        Execute Command       machine LoadPlatformDescriptionFromString "${line}"
    END
    Execute Command           emulation RunFor "0.001"

# Reads registers and gives them as one line of hex bytes, such as "0x01 0x02".
Read Registers
    [Arguments]               ${device}    ${register}    ${count}
    Execute Command           twi.${device} Write [${register}]
    ${output}=                Execute Command    twi.${device} Read ${count}
    ${bytes}=                 Evaluate    " ".join("0x%02X" % int(token, 16) for token in __import__("re").findall(r"0x[0-9A-Fa-f]+", $output))
    RETURN                    ${bytes}

Register Should Be
    [Arguments]               ${device}    ${register}    ${expected}
    ${bytes}=                 Read Registers    ${device}    ${register}    1
    Should Be Equal           ${bytes}    ${expected}

Write Register
    [Arguments]               ${device}    ${register}    ${value}
    Execute Command           twi.${device} Write [${register}, ${value}]

Touch Report Should Be
    [Arguments]               ${expected}
    ${bytes}=                 Read Registers    touch    0x01    6
    Should Be Equal           ${bytes}    ${expected}

Interrupt Line Should Be
    [Arguments]               ${expected}
    ${state}=                 Execute Command    twi.touch IRQ IsSet
    Should Contain            ${state}    ${expected}

HRS3300 Data Should Be Zero
    ${data}=                  Read Registers    heart    0x08    3
    Should Be Equal           ${data}    0x00 0x00 0x00
    ${data}=                  Read Registers    heart    0x0D    3
    Should Be Equal           ${data}    0x00 0x00 0x00

Load Bosch Config File
    [Arguments]               ${device}
    Write Register            ${device}    0x7E    0xB6
    Write Register            ${device}    0x7C    0x02
    Write Register            ${device}    0x59    0x00
    # Two 16-byte chunks at word addresses 0 and 8, as the Bosch API streams them.
    Write Register            ${device}    0x5B    0x00
    Write Register            ${device}    0x5C    0x00
    Execute Command           twi.${device} Write [0x5E, 0xC8, 0x2E, 0x00, 0x2E, 0x80, 0x2E, 0x3D, 0xB1, 0xC8, 0x2E, 0x00, 0x2E, 0x80, 0x2E, 0x91, 0x03]
    Write Register            ${device}    0x5B    0x08
    Write Register            ${device}    0x5C    0x00
    Execute Command           twi.${device} Write [0x5E, 0x80, 0x2E, 0xBC, 0x03, 0x80, 0x2E, 0x8A, 0xB1, 0x80, 0x2E, 0x10, 0x00, 0x80, 0x2E, 0x00, 0x2E]

*** Test Cases ***
CST816S Reports Its Chip Id
    Register Should Be        touch    0xA7    0xB4
    ${ids}=                   Read Registers    touch    0xA7    3
    Should Be Equal           ${ids}    0xB4 0x00 0x01

CST816S Touch Raises The Interrupt And Reports Coordinates
    # InfiniTime's setup: double taps, and interrupts on touch, change and motion.
    Write Register            touch    0xEC    0x05
    Write Register            touch    0xFA    0x70
    Interrupt Line Should Be  True
    Execute Command           twi.touch Touch 100 200
    Interrupt Line Should Be  False
    Touch Report Should Be    0x00 0x01 0x00 0x64 0x00 0xC8
    Execute Command           emulation RunFor "0.002"
    Interrupt Line Should Be  True
    Execute Command           twi.touch Move 102 201
    Touch Report Should Be    0x00 0x01 0x80 0x66 0x00 0xC9

CST816S Release Ends The Touch With A Tap
    Execute Command           twi.touch Touch 30 40
    Execute Command           emulation RunFor "0.05"
    Execute Command           twi.touch Release
    Touch Report Should Be    0x05 0x00 0x40 0x1E 0x00 0x28

CST816S Swipe Reports The Slide Gesture While Touching
    Execute Command           twi.touch Swipe "up"
    Execute Command           emulation RunFor "0.1"
    ${report}=                Read Registers    touch    0x01    2
    Should Be Equal           ${report}    0x02 0x01
    Execute Command           emulation RunFor "0.2"
    ${report}=                Read Registers    touch    0x01    2
    Should Be Equal           ${report}    0x02 0x00

CST816S Swipes Use The Finger's Direction
    Execute Command           twi.touch Swipe "down"
    Execute Command           emulation RunFor "0.3"
    Register Should Be        touch    0x01    0x01
    Execute Command           twi.touch Swipe "left"
    Execute Command           emulation RunFor "0.3"
    Register Should Be        touch    0x01    0x03
    Execute Command           twi.touch Swipe "right"
    Execute Command           emulation RunFor "0.3"
    Register Should Be        touch    0x01    0x04

CST816S Mouse Drag Becomes A Slide
    Execute Command           twi.touch MoveTo 120 60
    Execute Command           twi.touch Press
    Execute Command           emulation RunFor "0.02"
    Execute Command           twi.touch MoveTo 120 90
    Execute Command           emulation RunFor "0.02"
    Register Should Be        touch    0x01    0x00
    Execute Command           twi.touch MoveTo 120 160
    Touch Report Should Be    0x01 0x01 0x80 0x78 0x00 0xA0
    Execute Command           emulation RunFor "0.02"
    Execute Command           twi.touch Release
    Touch Report Should Be    0x01 0x00 0x40 0x78 0x00 0xA0

CST816S Long Press And Double Tap
    Execute Command           twi.touch LongPress 50 60
    Execute Command           emulation RunFor "1.1"
    Touch Report Should Be    0x0C 0x01 0x80 0x32 0x00 0x3C
    Execute Command           emulation RunFor "0.2"
    Write Register            touch    0xEC    0x01
    Execute Command           twi.touch DoubleTap 70 80
    Execute Command           emulation RunFor "0.1"
    Register Should Be        touch    0x01    0x00
    Execute Command           emulation RunFor "0.2"
    Register Should Be        touch    0x01    0x0B

CST816S Sleeps Until Reset
    Write Register            touch    0xA5    0x03
    Register Should Be        touch    0xA7    0xFF
    Execute Command           twi.touch Touch 10 10
    Interrupt Line Should Be  True
    Execute Command           twi.touch OnGPIO 0 false
    Execute Command           twi.touch OnGPIO 0 true
    Register Should Be        touch    0xA7    0xB4

BMA42x Reports The Chip Id Of Each Variant
    Register Should Be        accel     0x00    0x11
    Register Should Be        accel5    0x00    0x13

BMA42x Accepts The Bosch Init Sequence
    Register Should Be        accel    0x2A    0x00
    Load Bosch Config File    accel
    # The config file reads back through the same port before it is started.
    Write Register            accel    0x5B    0x08
    ${chunk}=                 Read Registers    accel    0x5E    4
    Should Be Equal           ${chunk}    0x80 0x2E 0xBC 0x03
    Write Register            accel    0x59    0x01
    Register Should Be        accel    0x2A    0x01
    ${address}=               Read Registers    accel    0x5B    2
    Should Be Equal           ${address}    0x00 0x00

BMA42x Without A Config File Fails To Start
    Write Register            accel    0x7E    0xB6
    Write Register            accel    0x59    0x01
    Register Should Be        accel    0x2A    0x02

BMA42x Feature Area Reads Back What Was Written
    Load Bosch Config File    accel
    Write Register            accel    0x59    0x01
    Execute Command           twi.accel Write [0x5E, 0x11, 0x22, 0x33, 0x44]
    ${features}=              Read Registers    accel    0x5E    4
    Should Be Equal           ${features}    0x11 0x22 0x33 0x44
    # A chunked access at word address 2 lands four bytes in.
    Write Register            accel    0x5B    0x02
    ${features}=              Read Registers    accel    0x5E    2
    Should Be Equal           ${features}    0x00 0x00
    Write Register            accel    0x5B    0x01
    ${features}=              Read Registers    accel    0x5E    2
    Should Be Equal           ${features}    0x33 0x44

BMA42x Reports Acceleration In The Chosen Range
    Write Register            accel    0x41    0x00
    Execute Command           twi.accel SetAcceleration 1000 -500 250
    # 2 g range: 1024 counts per g, left-justified in 16 bits.
    ${data}=                  Read Registers    accel    0x12    6
    Should Be Equal           ${data}    0x00 0x40 0x00 0xE0 0x00 0x10
    Write Register            accel    0x41    0x02
    ${data}=                  Read Registers    accel    0x12    2
    Should Be Equal           ${data}    0x00 0x10

BMA42x Counts Steps
    Execute Command           twi.accel SetSteps 70000
    ${steps}=                 Read Registers    accel    0x1E    4
    Should Be Equal           ${steps}    0x70 0x11 0x01 0x00
    Execute Command           twi.accel AddSteps 5
    ${steps}=                 Read Registers    accel    0x1E    4
    Should Be Equal           ${steps}    0x75 0x11 0x01 0x00

BMA42x Step Counter Reset Bit Clears The Count
    Load Bosch Config File    accel
    Write Register            accel    0x59    0x01
    Execute Command           twi.accel SetSteps 42
    # Word address 29 (byte 0x3A) is 0x0D in register 0x5B and 0x01 in register 0x5C.
    Write Register            accel    0x5B    0x0D
    Write Register            accel    0x5C    0x01
    Execute Command           twi.accel Write [0x5E, 0x00, 0x14]
    ${steps}=                 Read Registers    accel    0x1E    4
    Should Be Equal           ${steps}    0x00 0x00 0x00 0x00
    ${control}=               Read Registers    accel    0x5E    2
    Should Be Equal           ${control}    0x00 0x10

BMA42x Reports Temperature And Sensor Time
    Execute Command           twi.accel SetTemperature 30
    Register Should Be        accel    0x22    0x07
    ${before}=                Read Registers    accel    0x18    3
    Execute Command           emulation RunFor "0.01"
    ${after}=                 Read Registers    accel    0x18    3
    Should Not Be Equal       ${before}    ${after}

HRS3300 Reports Its Id
    Register Should Be        heart    0x00    0x21

HRS3300 Gives Zero While Disabled
    Write Register            heart    0x01    0x60
    HRS3300 Data Should Be Zero

HRS3300 Pulse Changes Over Virtual Time When Enabled
    Write Register            heart    0x16    0x88
    Write Register            heart    0x17    0x00
    Execute Command           twi.heart SetHeartRate 60
    Execute Command           twi.heart SetAmbientLight 1234
    Write Register            heart    0x01    0xE0
    ${first}=                 Read Registers    heart    0x08    8
    Execute Command           emulation RunFor "0.25"
    ${second}=                Read Registers    heart    0x08    8
    Should Not Be Equal       ${first}    ${second}
    # Ambient light 1234 packs as 0x9A in C1DATAM, 0x00 in C1DATAH and 0x02 in C1DATAL.
    ${als}=                   Read Registers    heart    0x08    1
    Should Be Equal           ${als}    0x9A
    ${als}=                   Read Registers    heart    0x0D    2
    Should Be Equal           ${als}    0x00 0x02
    Write Register            heart    0x01    0x60
    HRS3300 Data Should Be Zero
