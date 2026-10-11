# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Reset Emulation
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${PLATFORM}       @${CURDIR}/../scripts/pinetime.resc
${GPREGRET}       0x4000051C
${RESETREAS}      0x40000400
${AIRCR}          0xE000ED0C

*** Keywords ***
Create PineTime With An Idle Loop
    Execute Command           include ${PLATFORM}
    # A vector table pointing at a branch-to-self, so the CPU idles while time passes.
    Execute Command           sysbus WriteDoubleWord 0x0 0x20001000
    Execute Command           sysbus WriteDoubleWord 0x4 0x101
    Execute Command           sysbus WriteWord 0x100 0xE7FE
    Execute Command           cpu PC 0x100

Register Should Be
    [Arguments]    ${address}    ${expected}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    Should Be Equal As Integers    ${value}    ${expected}

*** Test Cases ***
Starts With No Reset Reason
    Create PineTime With An Idle Loop
    Register Should Be        ${RESETREAS}    0

Keeps GPREGRET Across A Soft Reset And Records It
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord ${GPREGRET} 0xB1
    Execute Command           sysbus WriteDoubleWord ${AIRCR} 0x05FA0004
    Execute Command           emulation RunFor "0.001"
    Register Should Be        ${GPREGRET}    0xB1
    Register Should Be        ${RESETREAS}    0x4

Clears A Reset Reason When Written With One
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord ${AIRCR} 0x05FA0004
    Execute Command           emulation RunFor "0.001"
    Execute Command           sysbus WriteDoubleWord ${RESETREAS} 0x4
    Register Should Be        ${RESETREAS}    0

Power Cycle Clears GPREGRET And RAM
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord 0x2000FFF0 0x4E454F54
    Execute Command           sysbus WriteDoubleWord ${GPREGRET} 0xB1
    Execute Command           powerClock PowerCycle
    Execute Command           emulation RunFor "0.001"
    ${marker}=                Execute Command    sysbus ReadDoubleWord 0x2000FFF0
    Should Not Contain        ${marker}    0x4E454F54
    Register Should Be        ${GPREGRET}    0
    Register Should Be        ${RESETREAS}    0

Watchdog Resets The Chip When Not Fed
    Create PineTime With An Idle Loop
    # Half a second: 16384 ticks of 32768 Hz.
    Execute Command           sysbus WriteDoubleWord 0x40010504 16383
    Execute Command           sysbus WriteDoubleWord 0x40010000 1
    Register Should Be        0x40010400    1
    Execute Command           emulation RunFor "0.4"
    Register Should Be        ${RESETREAS}    0
    Execute Command           emulation RunFor "0.2"
    Register Should Be        ${RESETREAS}    0x2
    Register Should Be        0x40010400    0

Watchdog Feeding Keeps The Chip Running
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord 0x40010504 16383
    Execute Command           sysbus WriteDoubleWord 0x40010000 1
    FOR    ${step}    IN RANGE    4
        Execute Command       emulation RunFor "0.3"
        Execute Command       sysbus WriteDoubleWord 0x40010600 0x6E524635
    END
    Register Should Be        ${RESETREAS}    0
    Register Should Be        0x40010400    1

Watchdog Needs Every Enabled Reload Register
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord 0x40010504 16383
    Execute Command           sysbus WriteDoubleWord 0x40010508 0x3
    Execute Command           sysbus WriteDoubleWord 0x40010000 1
    Execute Command           emulation RunFor "0.3"
    Execute Command           sysbus WriteDoubleWord 0x40010600 0x6E524635
    Execute Command           emulation RunFor "0.3"
    Register Should Be        ${RESETREAS}    0x2

High Frequency Clock Start Raises Its Event
    Create PineTime With An Idle Loop
    Execute Command           sysbus WriteDoubleWord 0x40000000 1
    Register Should Be        0x40000100    1
    Register Should Be        0x4000040C    0x10001
