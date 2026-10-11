# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Create Machines
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${PERIPHERALS}    ${CURDIR}/../peripherals
${PLATFORM}       SEPARATOR=
...               nvic: IRQControllers.NVIC @ sysbus 0xE000E000 { -> cpu@0 };
...               cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4f"; nvic: nvic };
...               ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x10000 };
...               radio: Wireless.NRF52832_RADIO @ sysbus 0x40001000 { -> nvic@1 };
...               timer0: Timers.NRF52832_TIMER @ sysbus 0x40008000 { -> nvic@8 };
...               ppi: Miscellaneous.NRF52840_PPI @ sysbus 0x4001F000;
...               recorder: Mocks.RadioEventRecorder @ sysbus 0x70000000 { radio: radio }
${STOCK_PLATFORM}    SEPARATOR=
...               nvic: IRQControllers.NVIC @ sysbus 0xE000E000 { -> cpu@0 };
...               cpu: CPU.CortexM @ sysbus { cpuType: "cortex-m4f"; nvic: nvic };
...               ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x10000 };
...               radio: Wireless.NRF52840_Radio @ sysbus 0x40001000 { -> nvic@1 };
...               recorder: Mocks.RadioEventRecorder @ sysbus 0x70000000 { radio: radio }
${RADIO}          0x40001000
${TIMER}          0x40008000
${PPI}            0x4001F000
${TX_BUFFER}      0x20001000
${RX_BUFFER}      0x20002000
# Shortcuts.
${READY_START}    0x1
${END_DISABLE}    0x2
${DISABLED_TXEN}  0x4
${DISABLED_RXEN}  0x8
${ADDRESS_RSSISTART}    0x10
${ADDRESS_BCSTART}      0x40
${DISABLED_RSSISTOP}    0x100

*** Keywords ***
Create Machines
    Execute Command           include @${PERIPHERALS}/NRF52832_RADIO.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_TIMER.cs
    Execute Command           include @${CURDIR}/RadioTestDevices.cs
    Execute Command           emulation SetGlobalQuantum "0.00001"
    Execute Command           emulation CreateBLEMedium "air"
    Create Machine            sender    ${PLATFORM}
    Create Machine            receiver    ${PLATFORM}

Create Machine
    [Arguments]               ${name}    ${platform}
    Execute Command           mach create "${name}"
    Execute Command           machine LoadPlatformDescriptionFromString '${platform}'
    Execute Command           connector Connect sysbus.radio air
    # A vector table and a sleep loop in RAM, so virtual time can run quickly.
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x20008000
    Execute Command           sysbus WriteDoubleWord 0x20000004 0x20000101
    Execute Command           sysbus WriteDoubleWord 0x20000100 0xE7FDBF30
    Execute Command           cpu VectorTableOffset 0x20000000
    Execute Command           cpu PC 0x20000100
    Configure Radio

On
    [Arguments]               ${name}
    Execute Command           mach set "${name}"

Write
    [Arguments]               ${offset}    ${value}
    ${address}=               Evaluate    ${RADIO} + ${offset}
    ${value}=                 Evaluate    ${value}
    Execute Command           sysbus WriteDoubleWord ${address} ${value}

Write Memory
    [Arguments]               ${address}    ${value}
    ${address}=               Evaluate    ${address}
    Execute Command           sysbus WriteDoubleWord ${address} ${value}

Register Should Be
    [Arguments]               ${offset}    ${expected}
    ${address}=               Evaluate    ${RADIO} + ${offset}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    Should Be Equal As Integers    ${value}    ${expected}

Memory Should Be
    [Arguments]               ${address}    ${expected}
    ${address}=               Evaluate    ${address}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    Should Be Equal As Integers    ${value}    ${expected}

Run For
    [Arguments]               ${seconds}
    Execute Command           emulation RunFor "${seconds}"

Events Should Be
    [Arguments]               ${expected}
    ${events}=                Execute Command    recorder Events
    Should Be Equal           ${events.strip()}    ${expected}

# Bluetooth LE advertising settings at 1 Mbit: S0, an 8-bit LENGTH and an S1 byte in RAM, the advertising access address, channel 37.
Configure Radio
    Write                     0x510    3
    Write                     0x514    0x00100108
    Write                     0x518    0x020300FF
    Write                     0x51C    0x89BED600
    Write                     0x524    0x8E
    Write                     0x52C    0
    Write                     0x530    1
    Write                     0x534    0x103
    Write                     0x538    0x65B
    Write                     0x53C    0x555555
    Write                     0x508    2
    Write                     0x554    37
    Write                     0x544    150
    # Fast ramp-up.
    Write                     0x650    0x201

# ADV_IND with a random address C0:DE:0A:1B:2C:3D and the flags 06.
Write Advertisement
    Write Memory              ${TX_BUFFER}    0x3D000940
    Write Memory              ${TX_BUFFER}+4    0xDE0A1B2C
    Write Memory              ${TX_BUFFER}+8    0x060102C0
    Write                     0x504    ${TX_BUFFER}

Send Advertisement
    [Arguments]               ${shortcuts}=${READY_START} | ${END_DISABLE}
    On                        sender
    Write Advertisement
    Write                     0x200    ${shortcuts}
    Write                     0x000    1

Listen
    [Arguments]               ${shortcuts}=${READY_START} | ${END_DISABLE} | ${ADDRESS_BCSTART} | ${ADDRESS_RSSISTART} | ${DISABLED_RSSISTOP}
    On                        receiver
    Write                     0x504    ${RX_BUFFER}
    Write                     0x560    8
    Write                     0x200    ${shortcuts}
    Write                     0x004    1

Advertisement Should Be In The Receive Buffer
    On                        receiver
    Memory Should Be          ${RX_BUFFER}    0x3D000940
    Memory Should Be          ${RX_BUFFER}+4    0xDE0A1B2C
    Memory Should Be          ${RX_BUFFER}+8    0x060102C0

*** Test Cases ***
Transmit Events Land On Their Bit Times
    Send Advertisement
    Run For                   0.001
    # 40 us ramp-up, 8 preamble and 32 address bits, 16 header and 72 payload bits, 24 CRC bits, 6 us to disable.
    Events Should Be          READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198
    Register Should Be        0x550    0

Normal Ramp-Up Takes 140 Microseconds
    On                        sender
    Write                     0x650    0x200
    Send Advertisement
    Run For                   0.001
    Events Should Be          READY@140 ADDRESS@180 PAYLOAD@268 END@292 DISABLED@298

Two Megabit Packets Take Half The Time With A Longer Preamble
    On                        sender
    Write                     0x510    4
    Write                     0x514    0x01100108
    Send Advertisement
    Run For                   0.001
    # 16 preamble and 32 address bits at 0.5 us each, then 112 bits, and 4 us to disable.
    Events Should Be          READY@40 ADDRESS@64 PAYLOAD@108 END@120 DISABLED@124

States Follow The Packet
    Send Advertisement
    Register Should Be        0x550    9
    Run For                   0.00004
    Register Should Be        0x550    11
    Run For                   0.000152
    Register Should Be        0x550    12
    Run For                   0.000006
    Register Should Be        0x550    0

Receiver Raises Its Events At The Air Times
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    # The first bit is on air 4 us after START, the address ends 40 us later, and the receiver reports it 6 us after that.
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202
    Advertisement Should Be In The Receive Buffer
    Register Should Be        0x400    1
    Register Should Be        0x408    0
    Register Should Be        0x548    40
    Register Should Be        0x550    0

Receiver Answers Exactly TIFS After The Packet
    On                        receiver
    Write Memory              0x20003000    0x00000004
    Write Memory              0x20003004    0x00000000
    Listen                    ${READY_START} | ${END_DISABLE} | ${DISABLED_TXEN}
    Send Advertisement        ${READY_START} | ${END_DISABLE} | ${DISABLED_RXEN}
    # Between DISABLED and START the receiver points at its answer and drops the turnaround shortcut, as firmware does.
    Run For                   0.00025
    On                        receiver
    Write                     0x504    0x20003000
    Write                     0x200    ${READY_START} | ${END_DISABLE}
    Run For                   0.001
    # The advertisement ends on air at 196 us; the answer starts on air at 346 us, so START is at 342 us.
    Events Should Be          READY@40 ADDRESS@90 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202 READY@342 ADDRESS@382 PAYLOAD@398 END@422 DISABLED@428
    On                        sender
    Events Should Be          READY@40 ADDRESS@80 PAYLOAD@168 END@192 DISABLED@198 READY@238 ADDRESS@392 PAYLOAD@408 CRCOK@432 END@432 DISABLED@432 READY@472

Frame Is Lost While The Receiver Still Ramps Up
    # The access address ends on air at 84 us; this receiver is ready only at 85 us.
    Send Advertisement
    Run For                   0.000045
    Listen
    Run For                   0.001
    Events Should Be          READY@85
    Register Should Be        0x550    3

Frame Is Taken When Listening Starts Just Before The Address Ends
    Send Advertisement
    Run For                   0.000043
    Listen
    Run For                   0.001
    Events Should Be          READY@83 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202
    Advertisement Should Be In The Receive Buffer

Frame Is Lost When The Receiver Is Not Listening
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          ${EMPTY}
    Listen
    Run For                   0.001
    Events Should Be          READY@1040
    Register Should Be        0x550    3

Address Mismatch Drops The Frame
    On                        receiver
    Write                     0x51C    0x12345600
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          READY@40
    Register Should Be        0x550    3

Second Logical Address Matches And Shows In RXMATCH
    On                        receiver
    Write                     0x51C    0x12345600
    Write                     0x520    0x89BED600
    Write                     0x524    0x8E00
    Write                     0x530    0x2
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202
    Register Should Be        0x408    1

Another Channel Never Hears The Frame
    On                        receiver
    Write                     0x508    26
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          READY@40

Device Address Match After The Advertiser Address
    On                        receiver
    Write                     0x600    0x0A1B2C3D
    Write                     0x620    0xC0DE
    # Logical address 0 enabled, with TxAdd set for a random address.
    Write                     0x640    0x0101
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    # 16 header bits and 48 address bits after ADDRESS at 90 us.
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 DEVMATCH@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202
    Register Should Be        0x410    0

Device Address Miss On A Different Address
    On                        receiver
    Write                     0x604    0x0A1B2C3D
    Write                     0x624    0xC0DF
    Write                     0x640    0x0202
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 DEVMISS@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202

Bit Counter Matches Again After BCC Moves On
    Listen
    Send Advertisement
    Run For                   0.0001
    On                        receiver
    Register Should Be        0x128    1
    Write                     0x128    0
    Write                     0x560    64
    Run For                   0.001
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 BCMATCH@154 PAYLOAD@178 CRCOK@202 END@202 DISABLED@202
    Register Should Be        0x128    1

Different CRC Seed Gives A CRC Error
    On                        receiver
    Write                     0x53C    0x123456
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    Events Should Be          READY@40 ADDRESS@90 RSSIEND@90.25 BCMATCH@98 PAYLOAD@178 CRCERROR@202 END@202 DISABLED@202
    Register Should Be        0x400    0

Stop Abandons The Packet
    Send Advertisement        ${READY_START}
    Run For                   0.0001
    Write                     0x00C    1
    Register Should Be        0x550    10
    Run For                   0.001
    Events Should Be          READY@40 ADDRESS@80

End Interrupt Follows INTEN
    On                        receiver
    Write                     0x304    0x8
    Listen
    Send Advertisement
    Run For                   0.001
    On                        receiver
    ${state}=                 Execute Command    radio IRQ IsSet
    Should Contain            ${state}    True
    Write                     0x10C    0
    ${state}=                 Execute Command    radio IRQ IsSet
    Should Contain            ${state}    False

Unknown Registers Keep Their Value
    Write                     0x774    0x01000000
    Register Should Be        0x774    0x01000000
    Register Should Be        0x554    0x65

PPI Captures Radio Events And Starts The Radio From TIMER0
    On                        sender
    Write Advertisement
    Write                     0x200    ${READY_START} | ${END_DISABLE}
    # TIMER0 at 1 MHz with CC[0] at 100 us; channels 20, 26 and 27 are TXEN, ADDRESS capture and END capture.
    Execute Command           sysbus WriteDoubleWord 0x40008508 3
    Execute Command           sysbus WriteDoubleWord 0x40008540 100
    Execute Command           sysbus WriteDoubleWord 0x4001F504 0x0C100000
    Execute Command           sysbus WriteDoubleWord 0x40008000 1
    Run For                   0.001
    # Each PPI task waits for the next 10 us sync point.
    ${events}=                Execute Command    recorder Events
    Should Be Equal           ${events.strip()}    READY@140 ADDRESS@180 PAYLOAD@268 END@292 DISABLED@298
    ${address}=               Execute Command    sysbus ReadDoubleWord 0x40008544
    Should Be Equal As Integers    ${address}    180
    ${end}=                   Execute Command    sysbus ReadDoubleWord 0x40008548
    Should Be Equal As Integers    ${end}    300

Stock Radio Receives A Frame From This Model
    Execute Command           mach create "stock"
    Execute Command           machine LoadPlatformDescriptionFromString '${STOCK_PLATFORM}'
    Execute Command           connector Connect sysbus.radio air
    Configure Radio
    Write                     0x504    ${RX_BUFFER}
    Write                     0x200    ${READY_START} | ${END_DISABLE}
    Write                     0x004    1
    Send Advertisement
    Run For                   0.001
    On                        stock
    # The stock model copies three header bytes for S1INCL, so the first payload byte also lands in the S1 slot.
    Memory Should Be          ${RX_BUFFER}    0x3D3D0940
    Memory Should Be          ${RX_BUFFER}+4    0xDE0A1B2C
    Memory Should Be          ${RX_BUFFER}+8    0x060102C0

Frame From The Stock Radio Counts From Its Hand-Over
    Execute Command           mach create "stock"
    Execute Command           machine LoadPlatformDescriptionFromString '${STOCK_PLATFORM}'
    Execute Command           connector Connect sysbus.radio air
    Configure Radio
    Listen
    Run For                   0.0001
    On                        stock
    Write Advertisement
    Write                     0x200    ${READY_START} | ${END_DISABLE}
    Write                     0x000    1
    Run For                   0.001
    On                        receiver
    # The stock radio raises ADDRESS as it starts, so its frame has its address the moment the medium hands it over, at the sender's next sync point.
    Events Should Be          READY@40 ADDRESS@116 RSSIEND@116.25 BCMATCH@124 PAYLOAD@204 CRCOK@228 END@228 DISABLED@228
    Advertisement Should Be In The Receive Buffer

Frame From The Stock Radio Is Lost While The Receiver Ramps Up
    Execute Command           mach create "stock"
    Execute Command           machine LoadPlatformDescriptionFromString '${STOCK_PLATFORM}'
    Execute Command           connector Connect sysbus.radio air
    Configure Radio
    Write Advertisement
    Write                     0x200    ${READY_START} | ${END_DISABLE}
    Write                     0x000    1
    Listen
    Run For                   0.001
    Events Should Be          READY@40
    Register Should Be        0x550    3

Bridge Variant Adds The nRF52840 Ready And PHYEND Events
    Execute Command           mach create "bridge"
    Execute Command           machine LoadPlatformDescriptionFromString 'radio: Wireless.NRF52832_RADIO @ sysbus 0x40001000 { nrf52840Registers: true }; recorder: Mocks.RadioEventRecorder @ sysbus 0x70000000 { radio: radio }; ram: Memory.MappedMemory @ sysbus 0x20000000 { size: 0x10000 }'
    Configure Radio
    Write Advertisement
    # TXREADY_START and PHYEND_DISABLE stand in for READY_START and END_DISABLE.
    Write                     0x200    0x140000
    Write                     0x000    1
    Run For                   0.001
    Events Should Be          READY@40 TXREADY@40 ADDRESS@80 PAYLOAD@168 END@192 PHYEND@192 DISABLED@198
