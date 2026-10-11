# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Create Serial Machine
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${SERIAL0}        0x40003000
${SERIAL1}        0x40004000
${NVIC_PENDING}   0xE000E200

*** Keywords ***
Create Serial Machine
    Execute Command           include @${CURDIR}/../peripherals/NRF52832_SerialInterface.cs
    Execute Command           include @${CURDIR}/SerialInterfaceTestDevices.cs
    Execute Command           mach create "serial"
    Execute Command           machine LoadPlatformDescription @${CURDIR}/serial_interface.repl

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

Memory Should Be
    [Arguments]               ${address}    ${expected}
    ${output}=                Execute Command    sysbus ReadDoubleWord ${address}
    ${value}=                 Convert To Integer    ${output.strip()}
    Should Be Equal As Integers    ${value}    ${expected}

Device Output Should Be
    [Arguments]               ${command}    ${expected}
    ${output}=                Execute Command    ${command}
    Should Be Equal           ${output.strip()}    ${expected}

*** Test Cases ***
Legacy SPI Exchanges Bytes Through The Double Buffer
    Write Register            ${SERIAL0}    0x500    1
    Write Register            ${SERIAL0}    0x304    0x4
    Register Should Be        ${SERIAL0}    0x108    0
    Write Register            ${SERIAL0}    0x51C    0xA5
    Write Register            ${SERIAL0}    0x51C    0x3C
    # A third byte waits until the receive buffer has room.
    Write Register            ${SERIAL0}    0x51C    0x01
    Device Output Should Be   sysbus.spi0.spidevice Received    A5 3C
    Register Should Be        ${SERIAL0}    0x108    1
    Memory Should Be          ${NVIC_PENDING}    0x8
    Write Register            ${SERIAL0}    0x108    0
    Register Should Be        ${SERIAL0}    0x518    0x5A
    Register Should Be        ${SERIAL0}    0x108    1
    Device Output Should Be   sysbus.spi0.spidevice Received    A5 3C 01
    Write Register            ${SERIAL0}    0x108    0
    Register Should Be        ${SERIAL0}    0x518    0xC3
    Register Should Be        ${SERIAL0}    0x108    1
    Register Should Be        ${SERIAL0}    0x518    0xFE

SPIM Runs A Full Duplex DMA Transfer Padded With ORC
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x00332211
    Write Register            ${SERIAL0}    0x500    7
    Write Register            ${SERIAL0}    0x304    0x40
    Write Register            ${SERIAL0}    0x544    0x20000000
    Write Register            ${SERIAL0}    0x548    2
    Write Register            ${SERIAL0}    0x534    0x20000100
    Write Register            ${SERIAL0}    0x538    4
    Write Register            ${SERIAL0}    0x5C0    0xEE
    Register Should Be        ${SERIAL0}    0x118    0
    Write Register            ${SERIAL0}    0x010    1
    Device Output Should Be   sysbus.spi0.spidevice Received    11 22 EE EE
    Memory Should Be          0x20000100    0x1111DDEE
    Register Should Be        ${SERIAL0}    0x54C    2
    Register Should Be        ${SERIAL0}    0x53C    4
    Register Should Be        ${SERIAL0}    0x14C    1
    Register Should Be        ${SERIAL0}    0x120    1
    Register Should Be        ${SERIAL0}    0x110    1
    Register Should Be        ${SERIAL0}    0x118    1
    Memory Should Be          ${NVIC_PENDING}    0x8
    # A longer transmit than receive drops the extra received bytes.
    Execute Command           sysbus WriteDoubleWord 0x20000104 0xAAAAAAAA
    Write Register            ${SERIAL0}    0x548    3
    Write Register            ${SERIAL0}    0x534    0x20000104
    Write Register            ${SERIAL0}    0x538    1
    Write Register            ${SERIAL0}    0x010    1
    Device Output Should Be   sysbus.spi0.spidevice Received    11 22 EE EE 11 22 33
    Memory Should Be          0x20000104    0xAAAAAAEE

SPIM Array List Moves To The Next Buffer After Each Transfer
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x00002211
    Write Register            ${SERIAL0}    0x500    7
    Write Register            ${SERIAL0}    0x544    0x20000000
    Write Register            ${SERIAL0}    0x548    1
    Write Register            ${SERIAL0}    0x550    1
    Write Register            ${SERIAL0}    0x538    0
    Write Register            ${SERIAL0}    0x010    1
    Write Register            ${SERIAL0}    0x010    1
    Device Output Should Be   sysbus.spi0.spidevice Received    11 22
    Register Should Be        ${SERIAL0}    0x544    0x20000000

TWIM Writes To A Device
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x00CDAB10
    Write Register            ${SERIAL1}    0x500    6
    Write Register            ${SERIAL1}    0x588    0x15
    Write Register            ${SERIAL1}    0x544    0x20000000
    Write Register            ${SERIAL1}    0x548    3
    Write Register            ${SERIAL1}    0x200    0x200
    Write Register            ${SERIAL1}    0x304    0x2
    Write Register            ${SERIAL1}    0x008    1
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    W 10 AB CD | F
    Device Output Should Be   sysbus.serial1.i2cdevice GetRegister 0x11    0x000000CD
    Register Should Be        ${SERIAL1}    0x54C    3
    Register Should Be        ${SERIAL1}    0x150    1
    Register Should Be        ${SERIAL1}    0x160    1
    Register Should Be        ${SERIAL1}    0x104    1
    Register Should Be        ${SERIAL1}    0x124    0
    Memory Should Be          ${NVIC_PENDING}    0x10

TWIM Write Then Read Uses A Repeated Start
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x20 0x01
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x21 0x02
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x22 0x03
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x20
    Execute Command           sysbus WriteDoubleWord 0x20000200 0xFFFFFFFF
    Write Register            ${SERIAL1}    0x500    6
    Write Register            ${SERIAL1}    0x588    0x15
    Write Register            ${SERIAL1}    0x544    0x20000000
    Write Register            ${SERIAL1}    0x548    1
    Write Register            ${SERIAL1}    0x534    0x20000200
    Write Register            ${SERIAL1}    0x538    3
    Write Register            ${SERIAL1}    0x200    0x1080
    Write Register            ${SERIAL1}    0x008    1
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    W 20 | R 3 | F
    Memory Should Be          0x20000200    0xFF030201
    Register Should Be        ${SERIAL1}    0x53C    3
    Register Should Be        ${SERIAL1}    0x160    1
    Register Should Be        ${SERIAL1}    0x14C    1
    Register Should Be        ${SERIAL1}    0x15C    1
    Register Should Be        ${SERIAL1}    0x104    1

TWIM Reports A NACK From A Missing Address
    Write Register            ${SERIAL1}    0x500    6
    Write Register            ${SERIAL1}    0x588    0x33
    Write Register            ${SERIAL1}    0x544    0x20000000
    Write Register            ${SERIAL1}    0x548    1
    Write Register            ${SERIAL1}    0x200    0x200
    Write Register            ${SERIAL1}    0x304    0x200
    Write Register            ${SERIAL1}    0x008    1
    Register Should Be        ${SERIAL1}    0x124    1
    Register Should Be        ${SERIAL1}    0x4C4    0x2
    Register Should Be        ${SERIAL1}    0x160    0
    Register Should Be        ${SERIAL1}    0x104    0
    Memory Should Be          ${NVIC_PENDING}    0x10
    Write Register            ${SERIAL1}    0x014    1
    Register Should Be        ${SERIAL1}    0x104    1
    Write Register            ${SERIAL1}    0x4C4    0x2
    Register Should Be        ${SERIAL1}    0x4C4    0
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    ${EMPTY}

Legacy TWI Reads Several Bytes
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x30 0x41
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x31 0x42
    Execute Command           sysbus.serial1.i2cdevice SetRegister 0x32 0x43
    Write Register            ${SERIAL1}    0x500    5
    Write Register            ${SERIAL1}    0x588    0x15
    Write Register            ${SERIAL1}    0x200    0x1
    Write Register            ${SERIAL1}    0x51C    0x30
    Write Register            ${SERIAL1}    0x008    1
    Register Should Be        ${SERIAL1}    0x11C    1
    Register Should Be        ${SERIAL1}    0x148    1
    Write Register            ${SERIAL1}    0x148    0
    Write Register            ${SERIAL1}    0x000    1
    Register Should Be        ${SERIAL1}    0x108    1
    Register Should Be        ${SERIAL1}    0x148    1
    Write Register            ${SERIAL1}    0x108    0
    Write Register            ${SERIAL1}    0x148    0
    Register Should Be        ${SERIAL1}    0x518    0x41
    Register Should Be        ${SERIAL1}    0x108    0
    Write Register            ${SERIAL1}    0x020    1
    Register Should Be        ${SERIAL1}    0x108    1
    Write Register            ${SERIAL1}    0x108    0
    Register Should Be        ${SERIAL1}    0x518    0x42
    # The last byte stops the bus at its byte boundary.
    Write Register            ${SERIAL1}    0x200    0x2
    Write Register            ${SERIAL1}    0x020    1
    Register Should Be        ${SERIAL1}    0x108    1
    Register Should Be        ${SERIAL1}    0x104    1
    Register Should Be        ${SERIAL1}    0x518    0x43
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    W 30 | R 1 | R 1 | R 1 | F
    Register Should Be        ${SERIAL1}    0x4C4    0

Legacy TWI Reports A NACK From A Missing Address
    Write Register            ${SERIAL1}    0x500    5
    Write Register            ${SERIAL1}    0x588    0x50
    Write Register            ${SERIAL1}    0x000    1
    Register Should Be        ${SERIAL1}    0x124    1
    Register Should Be        ${SERIAL1}    0x4C4    0x2
    Register Should Be        ${SERIAL1}    0x108    0
    Write Register            ${SERIAL1}    0x014    1
    Register Should Be        ${SERIAL1}    0x104    1

ENABLE Switches Between The Four Modes
    Write Register            ${SERIAL1}    0x500    1
    Register Should Be        ${SERIAL1}    0x500    1
    Write Register            ${SERIAL1}    0x51C    0x42
    Register Should Be        ${SERIAL1}    0x518    0xBD
    Device Output Should Be   sysbus.serial1.spidevice1 Received    42
    # SPIM tasks do nothing in legacy SPI mode.
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x77
    Write Register            ${SERIAL1}    0x544    0x20000000
    Write Register            ${SERIAL1}    0x548    1
    Write Register            ${SERIAL1}    0x010    1
    Register Should Be        ${SERIAL1}    0x118    0
    Write Register            ${SERIAL1}    0x500    7
    Register Should Be        ${SERIAL1}    0x500    7
    Write Register            ${SERIAL1}    0x010    1
    Register Should Be        ${SERIAL1}    0x118    1
    Device Output Should Be   sysbus.serial1.spidevice1 Received    42 77
    Write Register            ${SERIAL1}    0x500    6
    Register Should Be        ${SERIAL1}    0x500    6
    Write Register            ${SERIAL1}    0x588    0x15
    Write Register            ${SERIAL1}    0x200    0x200
    Write Register            ${SERIAL1}    0x008    1
    Register Should Be        ${SERIAL1}    0x104    1
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    W 77 | F
    Write Register            ${SERIAL1}    0x500    5
    Register Should Be        ${SERIAL1}    0x500    5
    Write Register            ${SERIAL1}    0x200    0x2
    Write Register            ${SERIAL1}    0x51C    0x05
    Write Register            ${SERIAL1}    0x008    1
    Device Output Should Be   sysbus.serial1.i2cdevice TransactionLog    W 77 | F | W 05 | F
    Device Output Should Be   sysbus.serial1.spidevice1 Received    42 77
    Write Register            ${SERIAL1}    0x500    3
    Register Should Be        ${SERIAL1}    0x500    0
    Write Register            ${SERIAL1}    0x500    0
    Register Should Be        ${SERIAL1}    0x500    0
