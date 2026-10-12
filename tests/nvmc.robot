# SPDX-License-Identifier: GPL-3.0-or-later
*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}

*** Variables ***
${PLATFORM}      @${CURDIR}/../scripts/pinetime.resc

*** Test Cases ***
Erases A Page Only With Erase Enabled
    Execute Command           include ${PLATFORM}
    Execute Command           sysbus WriteDoubleWord 0x2000 0x12345678
    Execute Command           sysbus WriteDoubleWord 0x4001E508 0x2000
    ${value}=                Execute Command    sysbus ReadDoubleWord 0x2000
    Should Contain            ${value}    0x12345678
    Execute Command           sysbus WriteDoubleWord 0x4001E504 0x2
    Execute Command           sysbus WriteDoubleWord 0x4001E508 0x2000
    ${value}=                Execute Command    sysbus ReadDoubleWord 0x2000
    Should Contain            ${value}    0xFFFFFFFF

Ignores A CPU Write To Flash Without Write Enabled
    Run A Flash Store From RAM    0x0
    ${value}=                Execute Command    sysbus ReadDoubleWord 0x2000
    Should Contain            ${value}    0x12345678

Lets A CPU Write Only Clear Bits With Write Enabled
    Run A Flash Store From RAM    0x1
    ${value}=                Execute Command    sysbus ReadDoubleWord 0x2000
    Should Contain            ${value}    0x00005678

*** Keywords ***
Run A Flash Store From RAM
    [Arguments]               ${config}
    Execute Command           include ${PLATFORM}
    Execute Command           sysbus WriteDoubleWord 0x2000 0x12345678
    Execute Command           sysbus WriteDoubleWord 0x4001E504 ${config}
    # ldr r0, =0x2000; ldr r1, =0x0000FFFF; str r1, [r0]; then a sleep loop.
    Execute Command           sysbus WriteDoubleWord 0x20000100 0x49034802
    Execute Command           sysbus WriteDoubleWord 0x20000104 0xBF306001
    Execute Command           sysbus WriteDoubleWord 0x20000108 0x0000E7FD
    Execute Command           sysbus WriteDoubleWord 0x2000010C 0x00002000
    Execute Command           sysbus WriteDoubleWord 0x20000110 0x0000FFFF
    Execute Command           cpu SP 0x20008000
    Execute Command           cpu PC 0x20000101
    Execute Command           emulation RunFor "0.001"
