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
