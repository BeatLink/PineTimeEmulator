*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}
Library           OperatingSystem

*** Variables ***
${MODEL}         @${CURDIR}/../peripherals/PineTime_ST7789.cs
${WINDOW}        @${CURDIR}/../scripts/window.resc
${LCD}           sysbus.spi0.lcd
${BRIDGE}        sysbus.gpio0.windowBridge

*** Keywords ***
Create Screen
    Execute Command           include ${MODEL}
    Execute Command           mach create
    Execute Command           machine LoadPlatformDescriptionFromString "spi0: SPI.NRF52840_SPI @ sysbus 0x40003000"
    Execute Command           machine LoadPlatformDescriptionFromString "lcd: Video.PineTime_ST7789 @ spi0"

Send Command
    [Arguments]    ${command}
    Execute Command           ${LCD} OnGPIO 0 false
    Execute Command           ${LCD} Transmit ${command}

Send Data
    [Arguments]    @{bytes}
    Execute Command           ${LCD} OnGPIO 0 true
    FOR    ${byte}    IN    @{bytes}
        Execute Command       ${LCD} Transmit ${byte}
    END

Shared File Number
    [Arguments]    ${path}    ${offset}    ${format}
    ${bytes}=                 Get Binary File    ${path}
    ${value}=                 Evaluate    __import__('struct').unpack_from('<${format}', $bytes, ${offset})[0]
    RETURN                    ${value}

*** Test Cases ***
Shares The Shown Frame Through A Mapped File
    Create Screen
    ${path}=                  Set Variable    ${TEMPDIR}/pinetime-frame-test
    Send Command              0x11
    Send Command              0x3A
    Send Data                 0x55
    Send Command              0x21
    Send Command              0x29
    Send Command              0x2A
    Send Data                 0x00    0x01    0x00    0x01
    Send Command              0x2B
    Send Data                 0x00    0x00    0x00    0x00
    Send Command              0x2C
    Send Data                 0xF8    0x00

    Execute Command           ${LCD} ShareFramesThrough "${path}"

    ${magic}=                 Shared File Number    ${path}    0     4s
    Should Be Equal           ${magic}    ${{b'PTFB'}}
    ${sequence}=              Shared File Number    ${path}    8     I
    Should Be Equal As Integers    ${{ $sequence % 2 }}    0
    ${width}=                 Shared File Number    ${path}    16    H
    Should Be Equal As Integers    ${width}    240
    ${flags}=                 Shared File Number    ${path}    20    I
    Should Be Equal As Integers    ${flags}    10
    ${backlight}=             Shared File Number    ${path}    24    I
    Should Be Equal As Integers    ${backlight}    3
    ${pixel}=                 Shared File Number    ${path}    67    3s
    Should Be Equal           ${pixel}    ${{b'\xff\x00\x00'}}

    Execute Command           ${LCD} StopSharingFrames
    Remove File               ${path}

Shares Battery And Button State Through A Mapped File
    ${path}=                  Set Variable    ${TEMPDIR}/pinetime-status-test
    Execute Command           include ${WINDOW}
    Execute Command           sysbus.gpio0.battery Percent 50
    Execute Command           sysbus.gpio0.battery ChargerConnected true
    Execute Command           sysbus.gpio0.sideButton Press

    Execute Command           ${BRIDGE} ShareStatusThrough "${path}"

    ${magic}=                 Shared File Number    ${path}    0     4s
    Should Be Equal           ${magic}    ${{b'PTST'}}
    ${flags}=                 Shared File Number    ${path}    12    I
    Should Be Equal As Integers    ${{ $flags & 14 }}    14
    ${percent}=               Shared File Number    ${path}    20    i
    Should Be Equal As Integers    ${percent}    50

    Execute Command           ${BRIDGE} StopSharingStatus
    Remove File               ${path}
