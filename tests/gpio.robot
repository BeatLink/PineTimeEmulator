*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Create GPIO Machine
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${GPIO}           0x50000000
${GPIOTE}         0x40006000
${PPI}            0x4001F000
${NVIC_PENDING}   0xE000E200
${OUT}            0x504
${OUTSET}         0x508
${OUTCLR}         0x50C
${IN}             0x510
${DIR}            0x514
${DIRSET}         0x518
${DIRCLR}         0x51C
${LATCH}          0x520
${DETECTMODE}     0x524
${EVENTS_PORT}    0x17C
${INTENSET}       0x304
${INTENCLR}       0x308

*** Keywords ***
Create GPIO Machine
    Execute Command           include @${CURDIR}/../peripherals/NRF52832_GPIO.cs
    Execute Command           include @${CURDIR}/../peripherals/NRF52832_GPIOTE.cs
    Execute Command           include @${CURDIR}/GpioTestDevices.cs
    Execute Command           mach create "gpio"
    Execute Command           machine LoadPlatformDescription @${CURDIR}/gpio.repl

Write Register
    [Arguments]               ${base}    ${offset}    ${value}
    ${address}=               Evaluate    ${base} + ${offset}
    Execute Command           sysbus WriteDoubleWord ${address} ${value}

Register Should Be
    [Arguments]               ${base}    ${offset}    ${expected}
    ${address}=               Evaluate    ${base} + ${offset}
    ${output}=                Execute Command    sysbus ReadDoubleWord ${address}
    ${value}=                 Convert To Integer    ${output.strip()}
    Should Be Equal As Integers    ${value}    ${expected}

Configure Pin
    [Arguments]               ${pin}    ${value}
    ${offset}=                Evaluate    0x700 + 4 * ${pin}
    Write Register            ${GPIO}    ${offset}    ${value}

Configure Channel
    [Arguments]               ${channel}    ${value}
    ${offset}=                Evaluate    0x510 + 4 * ${channel}
    Write Register            ${GPIOTE}    ${offset}    ${value}

Trigger Task
    [Arguments]               ${base_offset}    ${channel}
    ${offset}=                Evaluate    ${base_offset} + 4 * ${channel}
    Write Register            ${GPIOTE}    ${offset}    1

Event In Should Be
    [Arguments]               ${channel}    ${expected}
    ${offset}=                Evaluate    0x100 + 4 * ${channel}
    Register Should Be        ${GPIOTE}    ${offset}    ${expected}

Clear Event In
    [Arguments]               ${channel}
    ${offset}=                Evaluate    0x100 + 4 * ${channel}
    Write Register            ${GPIOTE}    ${offset}    0

Probe Should Read
    [Arguments]               ${pin}    ${expected}
    ${output}=                Execute Command    sysbus.gpio0.probe Level ${pin}
    Should Be Equal           ${output.strip()}    ${expected}

Pin Level Should Be
    [Arguments]               ${pin}    ${expected}
    ${output}=                Execute Command    sysbus.gpio0 PinLevel ${pin}
    Should Be Equal           ${output.strip()}    ${expected}

In Bit Should Be
    [Arguments]               ${pin}    ${expected}
    ${address}=               Evaluate    ${GPIO} + ${IN}
    ${output}=                Execute Command    sysbus ReadDoubleWord ${address}
    ${bit}=                   Evaluate    (int(${output.strip()}) >> ${pin}) & 1
    Should Be Equal As Integers    ${bit}    ${expected}

Detect Count Should Be
    [Arguments]               ${expected}
    ${output}=                Execute Command    sysbus.gpio0.detectCounter Count
    Should Be Equal As Integers    ${output.strip()}    ${expected}

Gpiote Interrupt Should Be
    [Arguments]               ${expected}
    ${output}=                Execute Command    sysbus.gpiote IRQ IsSet
    Should Be Equal           ${output.strip()}    ${expected}

*** Test Cases ***
Output Drives A Connected Receiver
    Probe Should Read         1    False
    Write Register            ${GPIO}    ${DIRSET}    0x2
    Write Register            ${GPIO}    ${OUTSET}    0x2
    Probe Should Read         1    True
    Pin Level Should Be       1    True
    Register Should Be        ${GPIO}    ${OUT}    0x2
    Register Should Be        ${GPIO}    ${DIR}    0x2
    Write Register            ${GPIO}    ${OUTCLR}    0x2
    Probe Should Read         1    False
    # OUT written while the pin is an input only shows once it becomes an output.
    Write Register            ${GPIO}    ${DIRCLR}    0x2
    Write Register            ${GPIO}    ${OUT}    0x2
    Probe Should Read         1    False
    Configure Pin             1    0x3
    Probe Should Read         1    True

Switching To Input Releases The Line To The Board Pull Up
    Probe Should Read         5    True
    Probe Should Read         25    True
    Write Register            ${GPIO}    ${DIRSET}    0x02000020
    Write Register            ${GPIO}    ${OUTCLR}    0x02000020
    Probe Should Read         5    False
    Probe Should Read         25    False
    Write Register            ${GPIO}    ${DIRCLR}    0x02000020
    Probe Should Read         5    True
    Probe Should Read         25    True
    Execute Command           sysbus.gpio0 ClearBoardPull 25
    Probe Should Read         25    False

Internal Pulls Set The Level Of An Undriven Input
    Configure Pin             2    0xC
    Probe Should Read         2    True
    In Bit Should Be          2    1
    Configure Pin             2    0x4
    Probe Should Read         2    False
    In Bit Should Be          2    0
    # An internal pull-down beats the weaker board pull-up.
    Configure Pin             25    0x4
    Probe Should Read         25    False
    Configure Pin             25    0x0
    Probe Should Read         25    True

External Drive Reads Through IN
    Configure Pin             10    0x0
    In Bit Should Be          10    0
    Execute Command           sysbus.gpio0.driver Drive true
    In Bit Should Be          10    1
    Execute Command           sysbus.gpio0.driver Drive false
    In Bit Should Be          10    0
    Configure Pin             3    0xC
    Execute Command           sysbus.gpio0 DriveExternally 3 false
    In Bit Should Be          3    0
    Execute Command           sysbus.gpio0 ReleaseExternalDrive 3
    In Bit Should Be          3    1
    # The chip's own output beats an outside driver.
    Configure Pin             3    0x1
    Write Register            ${GPIO}    ${OUTSET}    0x8
    Execute Command           sysbus.gpio0 DriveExternally 3 false
    In Bit Should Be          3    1

Disconnected Input Buffer Reads Zero
    Execute Command           sysbus.gpio0.driver Drive true
    Configure Pin             10    0x2
    Pin Level Should Be       10    True
    In Bit Should Be          10    0
    Configure Pin             10    0x0
    In Bit Should Be          10    1

Open Drain Pins Release The Line Instead Of Driving High
    Execute Command           sysbus.gpio0 SetBoardPull 3 true
    # S0D1: drives low, lets go for a one.
    Configure Pin             3    0x601
    Probe Should Read         3    False
    Write Register            ${GPIO}    ${OUTSET}    0x8
    Probe Should Read         3    True
    Execute Command           sysbus.gpio0 DriveExternally 3 false
    Probe Should Read         3    False
    In Bit Should Be          3    0
    Execute Command           sysbus.gpio0 ReleaseExternalDrive 3
    Probe Should Read         3    True
    # H0D1 behaves the same way.
    Configure Pin             3    0x701
    Probe Should Read         3    True
    Write Register            ${GPIO}    ${OUTCLR}    0x8
    Probe Should Read         3    False
    # D0S1: lets go for a zero, so the pull decides.
    Execute Command           sysbus.gpio0 ClearBoardPull 3
    Configure Pin             3    0x40D
    Probe Should Read         3    True
    Configure Pin             3    0x405
    Probe Should Read         3    False
    Write Register            ${GPIO}    ${OUTSET}    0x8
    Probe Should Read         3    True

Sense Sets Latch And Default Detect Follows The Pins
    Configure Pin             10    0x20000
    Register Should Be        ${GPIO}    ${LATCH}    0
    Detect Count Should Be    0
    Execute Command           sysbus.gpio0.driver Drive true
    Register Should Be        ${GPIO}    ${LATCH}    0x400
    Detect Count Should Be    1
    Execute Command           sysbus.gpio0.driver Drive false
    Register Should Be        ${GPIO}    ${LATCH}    0x400
    Write Register            ${GPIO}    ${LATCH}    0x400
    Register Should Be        ${GPIO}    ${LATCH}    0
    Execute Command           sysbus.gpio0.driver Drive true
    Detect Count Should Be    2
    # A latch bit cleared while its condition still holds sets again at once.
    Write Register            ${GPIO}    ${LATCH}    0x400
    Register Should Be        ${GPIO}    ${LATCH}    0x400
    Detect Count Should Be    2
    # Sensing looks through the input buffer.
    Execute Command           sysbus.gpio0.driver Drive false
    Write Register            ${GPIO}    ${LATCH}    0x400
    Configure Pin             10    0x20002
    Execute Command           sysbus.gpio0.driver Drive true
    Register Should Be        ${GPIO}    ${LATCH}    0
    Detect Count Should Be    2

LDETECT Stays High While Any Latch Bit Is Set
    Write Register            ${GPIO}    ${DETECTMODE}    1
    Register Should Be        ${GPIO}    ${DETECTMODE}    1
    Configure Pin             10    0x20000
    Execute Command           sysbus.gpio0.driver Drive true
    Detect Count Should Be    1
    Execute Command           sysbus.gpio0.driver Drive false
    Execute Command           sysbus.gpio0.driver Drive true
    # DETECT never fell because the latch held it, so no new edge.
    Detect Count Should Be    1
    Execute Command           sysbus.gpio0.driver Drive false
    Write Register            ${GPIO}    ${LATCH}    0x400
    Register Should Be        ${GPIO}    ${LATCH}    0
    Execute Command           sysbus.gpio0.driver Drive true
    Detect Count Should Be    2
    # Clearing a latch whose pin still meets its condition gives a fresh edge.
    Write Register            ${GPIO}    ${LATCH}    0x400
    Register Should Be        ${GPIO}    ${LATCH}    0x400
    Detect Count Should Be    3

Detect Event Fires On A Sense Low Pin
    Execute Command           sysbus.gpio0 SetBoardPull 7 true
    Configure Pin             7    0x30000
    Detect Count Should Be    0
    Execute Command           sysbus.gpio0 DriveExternally 7 false
    Detect Count Should Be    1
    Register Should Be        ${GPIO}    ${LATCH}    0x80
    Execute Command           sysbus.gpio0 ReleaseExternalDrive 7
    Execute Command           sysbus.gpio0 DriveExternally 7 false
    Detect Count Should Be    2

GPIOTE Event Mode Raises IN On A Rising Edge
    Configure Channel         0    0x10A01
    Write Register            ${GPIOTE}    ${INTENSET}    0x1
    Register Should Be        ${GPIOTE}    0x510    0x10A01
    Event In Should Be        0    0
    Execute Command           sysbus.gpio0.driver Drive true
    Event In Should Be        0    1
    Gpiote Interrupt Should Be    True
    Register Should Be        ${NVIC_PENDING}    0    0x40
    Clear Event In            0
    Gpiote Interrupt Should Be    False
    Execute Command           sysbus.gpio0.driver Drive false
    Event In Should Be        0    0

GPIOTE Event Mode Raises IN On Falling And Toggle
    Execute Command           sysbus.gpio0.driver Drive true
    Configure Channel         2    0x20A01
    Execute Command           sysbus.gpio0.driver Drive false
    Event In Should Be        2    1
    Gpiote Interrupt Should Be    False
    Write Register            ${GPIOTE}    ${INTENSET}    0x4
    Gpiote Interrupt Should Be    True
    Write Register            ${GPIOTE}    ${INTENCLR}    0x4
    Gpiote Interrupt Should Be    False
    Clear Event In            2
    Configure Channel         2    0x30A01
    Execute Command           sysbus.gpio0.driver Drive true
    Event In Should Be        2    1
    Clear Event In            2
    Execute Command           sysbus.gpio0.driver Drive false
    Event In Should Be        2    1

GPIOTE Event Mode Makes An Output Pin An Input
    Write Register            ${GPIO}    ${DIRSET}    0x20
    Write Register            ${GPIO}    ${OUTCLR}    0x20
    Probe Should Read         5    False
    # Taking the pin over releases it to the board pull-up without raising an event.
    Configure Channel         1    0x10501
    Probe Should Read         5    True
    Event In Should Be        1    0
    Configure Channel         1    0x0
    Probe Should Read         5    False

GPIOTE Task Mode Drives A Pin
    Configure Channel         1    0x130103
    Probe Should Read         1    True
    Trigger Task              0x60    1
    Probe Should Read         1    False
    Trigger Task              0x30    1
    Probe Should Read         1    True
    Trigger Task              0x00    1
    Probe Should Read         1    False
    Trigger Task              0x00    1
    Probe Should Read         1    True
    # OUT and DIR have no say while the channel owns the pin.
    Write Register            ${GPIO}    ${DIRSET}    0x2
    Write Register            ${GPIO}    ${OUTCLR}    0x2
    Probe Should Read         1    True
    # LoToHi makes OUT a set task.
    Configure Channel         1    0x010103
    Probe Should Read         1    False
    Trigger Task              0x00    1
    Probe Should Read         1    True
    Configure Channel         1    0x0
    Probe Should Read         1    False
    # Tasks on a channel that is not in task mode do nothing.
    Write Register            ${GPIO}    ${DIRCLR}    0x2
    Trigger Task              0x30    1
    Probe Should Read         1    False

GPIOTE PORT Event Follows DETECT
    Configure Pin             10    0x20000
    Write Register            ${GPIOTE}    ${INTENSET}    0x80000000
    Register Should Be        ${GPIOTE}    ${INTENSET}    0x80000000
    Register Should Be        ${GPIOTE}    ${EVENTS_PORT}    0
    Execute Command           sysbus.gpio0.driver Drive true
    Register Should Be        ${GPIOTE}    ${EVENTS_PORT}    1
    Gpiote Interrupt Should Be    True
    Register Should Be        ${NVIC_PENDING}    0    0x40
    Write Register            ${GPIOTE}    ${EVENTS_PORT}    0
    Gpiote Interrupt Should Be    False

PPI Links A GPIOTE Event To A GPIOTE Task
    Configure Channel         0    0x10A01
    Configure Channel         1    0x000103
    Write Register            ${PPI}    0x510    0x40006100
    Write Register            ${PPI}    0x514    0x40006034
    Write Register            ${PPI}    0x504    0x1
    Probe Should Read         1    False
    # PPI runs its task once emulated time moves, so the CPU spins in a loop for a moment.
    Execute Command           sysbus WriteDoubleWord 0x20000040 0xE7FEE7FE
    Execute Command           cpu VectorTableOffset 0x20000000
    Execute Command           cpu PC 0x20000040
    Execute Command           sysbus.gpio0.driver Drive true
    Execute Command           emulation RunFor "0.001"
    Probe Should Read         1    True

Reset Returns Every Pin To A Disconnected Input
    Write Register            ${GPIO}    ${DIRSET}    0x02000022
    Write Register            ${GPIO}    ${OUTCLR}    0x02000020
    Write Register            ${GPIO}    ${OUTSET}    0x2
    Configure Pin             10    0x2000C
    Configure Channel         3    0x130303
    Write Register            ${GPIO}    ${DETECTMODE}    1
    Probe Should Read         25    False
    Probe Should Read         1    True
    Execute Command           machine Reset
    Register Should Be        ${GPIO}    ${DIR}    0
    Register Should Be        ${GPIO}    ${OUT}    0
    Register Should Be        ${GPIO}    0x700    0x2
    Register Should Be        ${GPIO}    0x728    0x2
    Register Should Be        ${GPIO}    ${DETECTMODE}    0
    Register Should Be        ${GPIO}    ${LATCH}    0
    Register Should Be        ${GPIOTE}    0x51C    0
    Register Should Be        ${GPIO}    ${IN}    0
    Probe Should Read         1    False
    Probe Should Read         3    False
    Probe Should Read         5    True
    Probe Should Read         25    True
