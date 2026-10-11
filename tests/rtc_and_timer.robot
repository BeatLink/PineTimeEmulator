# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Create Machine
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${PERIPHERALS}    ${CURDIR}/../peripherals
${PLATFORM}       SEPARATOR=
...               nvic: IRQControllers.NVIC @ sysbus 0xE000E000 { -> cpu@0 };
...               cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4f"; nvic: nvic };
...               ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x10000 };
...               timer0: Timers.NRF52832_TIMER @ sysbus 0x40008000 { -> nvic@8 };
...               rtc0: Timers.NRF52832_RTC @ sysbus 0x4000B000 { numberOfCompareChannels: 3; -> nvic@11 };
...               timer3: Timers.NRF52832_TIMER @ sysbus 0x4001A000 { numberOfCompareChannels: 6; -> nvic@26 };
...               ppi: Miscellaneous.NRF52840_PPI @ sysbus 0x4001F000
${RTC}            0x4000B000
${TIMER}          0x40008000
${TIMER3}         0x4001A000
${PPI}            0x4001F000
# One tick of the RTC with PRESCALER 255 lasts 1/128 s.
${RTC_TICK}       0.0078125
${RTC_HALF_TICK}  0.00390625

*** Keywords ***
Create Machine
    Execute Command           include @${PERIPHERALS}/NRF52832_RTC.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_TIMER.cs
    Execute Command           mach create
    Execute Command           machine LoadPlatformDescriptionFromString '${PLATFORM}'
    # A vector table and a sleep loop in RAM, so virtual time can run quickly.
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x20008000
    Execute Command           sysbus WriteDoubleWord 0x20000004 0x20000101
    Execute Command           sysbus WriteDoubleWord 0x20000100 0xE7FDBF30
    Execute Command           cpu VectorTableOffset 0x20000000
    Execute Command           cpu PC 0x20000100

Register Should Be
    [Arguments]               ${address}    ${expected}
    ${address}=               Evaluate    ${address}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    Should Be Equal As Integers    ${value}    ${expected}

Register Should Be Between
    [Arguments]               ${address}    ${lowest}    ${highest}
    ${address}=               Evaluate    ${address}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    ${value}=                 Convert To Integer    ${value.strip()}
    Should Be True            ${lowest} <= ${value} <= ${highest}    ${value} is outside ${lowest}..${highest}

Write
    [Arguments]               ${address}    ${value}
    ${address}=               Evaluate    ${address}
    Execute Command           sysbus WriteDoubleWord ${address} ${value}

Run For
    [Arguments]               ${seconds}
    Execute Command           emulation RunFor "${seconds}"

Interrupt Should Be
    [Arguments]               ${peripheral}    ${expected}
    ${state}=                 Execute Command    ${peripheral} IRQ IsSet
    Should Contain            ${state}    ${expected}

Start Slow RTC
    # PRESCALER 255 makes the RTC tick at 128 Hz.
    Write                     ${RTC}+0x508    255
    Write                     ${RTC}+0x000    1

Capture Timer
    [Arguments]               ${base}    ${channel}
    Write                     ${base}+0x40+4*${channel}    1
    ${cc}=                    Evaluate    ${base}+0x540+4*${channel}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${cc}
    ${value}=                 Convert To Integer    ${value.strip()}
    RETURN                    ${value}

Captured Count Should Be
    [Arguments]               ${base}    ${channel}    ${expected}
    ${value}=                 Capture Timer    ${base}    ${channel}
    Should Be Equal As Integers    ${value}    ${expected}

*** Test Cases ***
RTC Counts At 32768 Hz
    Write                     ${RTC}+0x000    1
    Run For                   1
    Register Should Be        ${RTC}+0x504    32768

RTC Counts At The Prescaled Rate
    Start Slow RTC
    Run For                   0.5
    Register Should Be        ${RTC}+0x504    64
    Run For                   0.0078
    Register Should Be        ${RTC}+0x504    64
    Run For                   0.00002
    Register Should Be        ${RTC}+0x504    65

RTC Ignores A Prescaler Write While Running
    Start Slow RTC
    Write                     ${RTC}+0x508    0
    Register Should Be        ${RTC}+0x508    255
    Write                     ${RTC}+0x004    1
    Write                     ${RTC}+0x508    0
    Register Should Be        ${RTC}+0x508    0

RTC Compare Raises Its Event And Interrupt On Time
    Write                     ${RTC}+0x540    10
    Write                     ${RTC}+0x304    0x10000
    Start Slow RTC
    # Ten ticks of 1/128 s is 78.125 ms.
    Run For                   0.078
    Register Should Be        ${RTC}+0x140    0
    Interrupt Should Be       rtc0    False
    Run For                   0.0002
    Register Should Be        ${RTC}+0x140    1
    Register Should Be        ${RTC}+0x504    10
    Interrupt Should Be       rtc0    True
    Write                     ${RTC}+0x140    0
    Interrupt Should Be       rtc0    False

RTC Compare Needs INTEN Or EVTEN
    Write                     ${RTC}+0x544    5
    Start Slow RTC
    Run For                   0.1
    Register Should Be        ${RTC}+0x144    0
    Interrupt Should Be       rtc0    False

RTC Compare One Ahead Fires And Compare Equal Waits For A Wrap
    Write                     ${RTC}+0x304    0x30000
    Start Slow RTC
    # Half way through the tick where COUNTER is 64.
    Run For                   0.50390625
    Write                     ${RTC}+0x540    65
    Write                     ${RTC}+0x544    64
    Run For                   ${RTC_TICK}
    Register Should Be        ${RTC}+0x140    1
    Register Should Be        ${RTC}+0x144    0
    Run For                   0.1
    Register Should Be        ${RTC}+0x144    0

RTC Tick Event Only When Enabled
    Start Slow RTC
    Run For                   0.1
    Register Should Be        ${RTC}+0x100    0
    # EVTEN alone sets the event register but raises no interrupt.
    Write                     ${RTC}+0x344    0x1
    Run For                   0.01
    Register Should Be        ${RTC}+0x100    1
    Interrupt Should Be       rtc0    False
    Write                     ${RTC}+0x348    0x1
    Write                     ${RTC}+0x100    0
    Run For                   0.05
    Register Should Be        ${RTC}+0x100    0
    # INTEN alone sets the event register and raises the interrupt.
    Write                     ${RTC}+0x304    0x1
    Run For                   0.01
    Register Should Be        ${RTC}+0x100    1
    Interrupt Should Be       rtc0    True
    Write                     ${RTC}+0x100    0
    Interrupt Should Be       rtc0    False
    Run For                   0.01
    Interrupt Should Be       rtc0    True

RTC Trigger Overflow Sets The Counter Near The Top
    Write                     ${RTC}+0x00C    1
    Register Should Be        ${RTC}+0x504    0xFFFFF0

RTC Overflows Sixteen Ticks After TRIGOVRFLW
    Write                     ${RTC}+0x304    0x2
    Start Slow RTC
    Run For                   0.5
    Write                     ${RTC}+0x00C    1
    Register Should Be        ${RTC}+0x504    0xFFFFF0
    # 14.5 ticks, then 15.5, then 16.5.
    Run For                   0.11328125
    Register Should Be        ${RTC}+0x504    0xFFFFFE
    Run For                   ${RTC_TICK}
    Register Should Be        ${RTC}+0x504    0xFFFFFF
    Register Should Be        ${RTC}+0x104    0
    Interrupt Should Be       rtc0    False
    Run For                   ${RTC_TICK}
    Register Should Be        ${RTC}+0x504    0
    Register Should Be        ${RTC}+0x104    1
    Interrupt Should Be       rtc0    True

RTC Clear Resets The Counter Without A Compare At Zero
    Write                     ${RTC}+0x540    0
    Write                     ${RTC}+0x304    0x10000
    Start Slow RTC
    Run For                   0.5
    Register Should Be        ${RTC}+0x504    64
    Write                     ${RTC}+0x008    1
    Register Should Be        ${RTC}+0x504    0
    Register Should Be        ${RTC}+0x140    0
    Run For                   0.04296875
    Register Should Be        ${RTC}+0x504    5

RTC Stop Freezes The Counter
    Start Slow RTC
    Run For                   0.25
    Write                     ${RTC}+0x004    1
    Register Should Be        ${RTC}+0x504    32
    Run For                   1
    Register Should Be        ${RTC}+0x504    32
    Write                     ${RTC}+0x000    1
    Run For                   0.04296875
    Register Should Be        ${RTC}+0x504    37

RTC Tick Through PPI Captures The TIMER
    Execute Command           emulation SetGlobalQuantum "0.00001"
    # TIMER0 at 1 MHz, and PPI channel 0 from RTC0 TICK to TIMER0 CAPTURE[0].
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x000    1
    Write                     ${PPI}+0x510    0x4000B100
    Write                     ${PPI}+0x514    0x40008040
    Write                     ${PPI}+0x504    0x1
    Write                     ${RTC}+0x344    0x1
    Start Slow RTC
    # The first tick is at 7812.5 us; the PPI runs its task at the next sync point.
    Run For                   0.005
    Register Should Be        ${TIMER}+0x540    0
    Run For                   0.005
    Register Should Be Between    ${TIMER}+0x540    7812    7823
    Register Should Be        ${RTC}+0x100    1
    Interrupt Should Be       rtc0    False

RTC Compare Through PPI Starts The TIMER
    Write                     ${TIMER}+0x508    3
    Write                     ${PPI}+0x510    0x4000B144
    Write                     ${PPI}+0x514    0x40008000
    Write                     ${PPI}+0x504    0x1
    Write                     ${RTC}+0x544    4
    Write                     ${RTC}+0x344    0x20000
    Start Slow RTC
    Run For                   0.03
    Captured Count Should Be    ${TIMER}    1    0
    Run For                   0.01
    ${count}=                 Capture Timer    ${TIMER}    1
    # Four ticks is 31.25 ms, so up to 8.75 ms have been counted, less the PPI's wait for a sync point.
    Should Be True            8600 <= ${count} <= 8750

TIMER Compare At The Exact Microsecond
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x540    1000
    Write                     ${TIMER}+0x304    0x10000
    Register Should Be        ${TIMER}+0x510    4
    Write                     ${TIMER}+0x000    1
    Run For                   0.000999
    Register Should Be        ${TIMER}+0x140    0
    Interrupt Should Be       timer0    False
    Run For                   0.000001
    Register Should Be        ${TIMER}+0x140    1
    Interrupt Should Be       timer0    True
    Write                     ${TIMER}+0x308    0x10000
    Interrupt Should Be       timer0    False

TIMER Capture Reads The Exact Count
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x510    0
    Write                     ${TIMER}+0x000    1
    Run For                   0.001
    Captured Count Should Be    ${TIMER}    2    16000
    Run For                   0.0000001
    Captured Count Should Be    ${TIMER}    2    16001

TIMER Compare Clear Shortcut Gives Periodic Events
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x540    100
    Write                     ${TIMER}+0x200    0x1
    Write                     ${TIMER}+0x000    1
    Run For                   0.0000995
    Register Should Be        ${TIMER}+0x140    0
    Run For                   0.000001
    Register Should Be        ${TIMER}+0x140    1
    Write                     ${TIMER}+0x140    0
    Run For                   0.000099
    Register Should Be        ${TIMER}+0x140    0
    Run For                   0.000001
    Register Should Be        ${TIMER}+0x140    1
    Captured Count Should Be    ${TIMER}    1    0
    Run For                   0.00005
    Captured Count Should Be    ${TIMER}    1    50
    # A hundred more periods still land on whole multiples of 100 us.
    Run For                   0.01
    Captured Count Should Be    ${TIMER}    1    50

TIMER Compare Stop Shortcut Freezes The Counter
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x540    100
    Write                     ${TIMER}+0x200    0x100
    Write                     ${TIMER}+0x000    1
    Run For                   0.001
    Register Should Be        ${TIMER}+0x140    1
    Captured Count Should Be    ${TIMER}    1    100
    Write                     ${TIMER}+0x000    1
    Run For                   0.00001
    Captured Count Should Be    ${TIMER}    1    110

TIMER Stop And Clear Tasks
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x000    1
    Run For                   0.0002
    Write                     ${TIMER}+0x004    1
    Run For                   0.001
    Captured Count Should Be    ${TIMER}    0    200
    Write                     ${TIMER}+0x00C    1
    Captured Count Should Be    ${TIMER}    0    0

TIMER Counter Mode Counts COUNT Tasks
    Write                     ${TIMER}+0x504    1
    Write                     ${TIMER}+0x540    3
    Write                     ${TIMER}+0x008    1
    Captured Count Should Be    ${TIMER}    1    0
    Write                     ${TIMER}+0x000    1
    Write                     ${TIMER}+0x008    1
    Write                     ${TIMER}+0x008    1
    Run For                   0.01
    Register Should Be        ${TIMER}+0x140    0
    Captured Count Should Be    ${TIMER}    1    2
    Write                     ${TIMER}+0x008    1
    Register Should Be        ${TIMER}+0x140    1

TIMER Counter Mode Clear Shortcut
    Write                     ${TIMER}+0x504    2
    Write                     ${TIMER}+0x540    2
    Write                     ${TIMER}+0x200    0x1
    Write                     ${TIMER}+0x000    1
    Write                     ${TIMER}+0x008    1
    Write                     ${TIMER}+0x008    1
    Register Should Be        ${TIMER}+0x140    1
    Captured Count Should Be    ${TIMER}    1    0
    Write                     ${TIMER}+0x008    1
    Captured Count Should Be    ${TIMER}    1    1

TIMER Wraps At 8 Bits
    Write                     ${TIMER}+0x508    1
    Write                     ${TIMER}+0x540    0
    Write                     ${TIMER}+0x000    1
    Run For                   0.000255
    Register Should Be        ${TIMER}+0x140    0
    Run For                   0.000045
    Register Should Be        ${TIMER}+0x140    1
    Captured Count Should Be    ${TIMER}    1    44

TIMER Wraps At 16 Bits
    Write                     ${TIMER}+0x000    1
    Run For                   0.07
    Captured Count Should Be    ${TIMER}    0    4464

TIMER Wraps At 24 Bits
    Write                     ${TIMER}+0x508    2
    Write                     ${TIMER}+0x510    0
    Write                     ${TIMER}+0x000    1
    Run For                   1.1
    Captured Count Should Be    ${TIMER}    0    822784

TIMER Wraps At 32 Bits
    # The CPU sleeps, so let virtual time run ahead of real time.
    Execute Command           emulation SetGlobalQuantum "0.01"
    Execute Command           emulation SetAdvanceImmediately true
    Write                     ${TIMER}+0x508    3
    Write                     ${TIMER}+0x510    0
    Write                     ${TIMER}+0x000    1
    Run For                   1.1
    Captured Count Should Be    ${TIMER}    0    17600000
    Run For                   268.9
    Captured Count Should Be    ${TIMER}    0    25032704

TIMER With Six Channels Captures On Channel 5
    Write                     ${TIMER3}+0x508    3
    Write                     ${TIMER3}+0x554    20
    Write                     ${TIMER3}+0x200    0x2020
    Write                     ${TIMER3}+0x304    0x200000
    Write                     ${TIMER3}+0x000    1
    Run For                   0.0001
    Register Should Be        ${TIMER3}+0x154    1
    Interrupt Should Be       timer3    True
    Captured Count Should Be    ${TIMER3}    5    0
