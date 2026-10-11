*** Settings ***
Suite Setup       Setup
Suite Teardown    Teardown
Test Setup        Create Screen
Test Teardown     Test Teardown
Resource          ${RENODEKEYWORDS}
Library           OperatingSystem

*** Variables ***
${MODEL}         @${CURDIR}/../peripherals/PineTime_ST7789.cs
${LCD}           sysbus.spi0.lcd
${BLACK}         0x00000000
${RED}           0x00FF0000
${GREEN}         0x0000FF00
${BLUE}          0x000000FF
${CYAN}          0x0000FFFF

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

Initialise Screen
    Send Command              0x01
    Send Command              0x11
    Send Command              0x3A
    Send Data                 0x55
    Send Command              0x36
    Send Data                 0x00
    Send Command              0xB2
    Send Data                 0x0C    0x0C    0x00    0x33    0x33
    Send Command              0xBB
    Send Data                 0x35
    Send Command              0x21
    Send Command              0x13
    Send Command              0x29

Set Window
    [Arguments]    ${first_column}    ${last_column}    ${first_row}    ${last_row}
    Send Command              0x2A
    Send Data                 0x00    ${first_column}    0x00    ${last_column}
    Send Command              0x2B
    Send Data                 0x00    ${first_row}    0x00    ${last_row}

Write Pixels
    [Arguments]    @{bytes}
    Send Command              0x2C
    Send Data                 @{bytes}

Pixel Should Be
    [Arguments]    ${x}    ${y}    ${expected}
    ${shown}=                 Execute Command    ${LCD} ShownPixel ${x} ${y}
    Should Contain            ${shown}    ${expected}

State Should Be
    [Arguments]    ${property}    ${expected}
    ${value}=                 Execute Command    ${LCD} ${property}
    Should Contain            ${value}    ${expected}

*** Test Cases ***
Shows A Filled Window After The Init Sequence
    Initialise Screen
    State Should Be           Sleeping    False
    State Should Be           DisplayOn    True
    State Should Be           Inverted    True

    Set Window                10    12    20    21
    Write Pixels              0xF8    0x00    0xF8    0x00    0xF8    0x00    0x07    0xE0    0x07    0xE0    0x07    0xE0    0x00    0x1F

    Pixel Should Be           10    20    ${BLUE}
    Pixel Should Be           12    20    ${RED}
    Pixel Should Be           10    21    ${GREEN}
    Pixel Should Be           12    21    ${GREEN}
    Pixel Should Be           13    20    ${BLACK}
    Pixel Should Be           9     21    ${BLACK}

Shows Inverted Colours Without INVON
    Initialise Screen
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00
    Pixel Should Be           0    0    ${RED}

    Send Command              0x20

    State Should Be           Inverted    False
    Pixel Should Be           0    0    ${CYAN}
    Pixel Should Be           1    0    0x00FFFFFF

Scrolls The Middle Area And Keeps The Fixed Top
    Initialise Screen
    Set Window                0    0    1    1
    Write Pixels              0x00    0x1F
    Set Window                0    0    5    5
    Write Pixels              0xF8    0x00
    Send Command              0x2A
    Send Data                 0x00    0x00    0x00    0x00
    Send Command              0x2B
    Send Data                 0x01    0x3F    0x01    0x3F
    Write Pixels              0x07    0xE0

    Send Command              0x33
    Send Data                 0x00    0x02    0x01    0x3E    0x00    0x00
    Send Command              0x37
    Send Data                 0x00    0x04

    State Should Be           ScrollStart    4
    Pixel Should Be           0    1    ${BLUE}
    Pixel Should Be           0    3    ${RED}
    Pixel Should Be           0    5    ${BLACK}

    Send Command              0x37
    Send Data                 0x01    0x3F

    Pixel Should Be           0    1    ${BLUE}
    Pixel Should Be           0    2    ${GREEN}

    Send Command              0x13

    Pixel Should Be           0    5    ${RED}

Places Pixels By MADCTL Mirror And Exchange Bits
    Initialise Screen
    Send Command              0x36
    Send Data                 0x40
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00
    Pixel Should Be           239    0    ${RED}
    Pixel Should Be           0    0    ${BLACK}

    Send Command              0x36
    Send Data                 0x20
    Set Window                5    5    0    0
    Write Pixels              0x07    0xE0
    Pixel Should Be           0    5    ${GREEN}

    Send Command              0x36
    Send Data                 0x60
    Set Window                7    7    0    0
    Write Pixels              0x00    0x1F
    Pixel Should Be           239    7    ${BLUE}

    Send Command              0x36
    Send Data                 0x80
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00
    ${stored}=                Execute Command    ${LCD} MemoryPixel 0 319
    Should Contain            ${stored}    0xF800

Continues A Memory Write Across Transmissions
    Initialise Screen
    Set Window                0    2    0    0
    Send Command              0x2C
    Send Data                 0xF8
    Execute Command           ${LCD} FinishTransmission
    Send Data                 0x00    0x07
    Execute Command           ${LCD} FinishTransmission
    Send Data                 0xE0

    Send Command              0x3C
    Send Data                 0x00    0x1F

    Pixel Should Be           0    0    ${RED}
    Pixel Should Be           1    0    ${GREEN}
    Pixel Should Be           2    0    ${BLUE}

Shows Black While Asleep Or Off
    Initialise Screen
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00

    Send Command              0x10
    State Should Be           Sleeping    True
    Pixel Should Be           0    0    ${BLACK}

    Send Command              0x11
    Pixel Should Be           0    0    ${RED}

    Send Command              0x28
    State Should Be           DisplayOn    False
    Pixel Should Be           0    0    ${BLACK}

Reset Pin Clears The Controller State
    Initialise Screen
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00

    Execute Command           ${LCD} OnGPIO 1 false
    State Should Be           InReset    True
    Pixel Should Be           0    0    ${BLACK}
    Execute Command           ${LCD} OnGPIO 1 true

    State Should Be           InReset    False
    State Should Be           Sleeping    True
    State Should Be           DisplayOn    False
    State Should Be           Inverted    False
    State Should Be           PixelFormatSetting    0x66
    ${stored}=                Execute Command    ${LCD} MemoryPixel 0 0
    Should Contain            ${stored}    0xF800

Dims With The Backlight Pins
    Initialise Screen
    Set Window                0    0    0    0
    Write Pixels              0xF8    0x00

    Execute Command           ${LCD} OnGPIO 2 false
    State Should Be           BacklightLevel    1
    Pixel Should Be           0    0    0x00660000

    Execute Command           ${LCD} OnGPIO 4 false
    Pixel Should Be           0    0    ${RED}

    Execute Command           ${LCD} OnGPIO 2 true
    Execute Command           ${LCD} OnGPIO 4 true
    Pixel Should Be           0    0    ${BLACK}

Answers Identification And Power Reads
    Initialise Screen
    Send Command              0x04
    Execute Command           ${LCD} OnGPIO 0 true
    Execute Command           ${LCD} Transmit 0x00
    ${first}=                 Execute Command    ${LCD} Transmit 0x00
    ${second}=                Execute Command    ${LCD} Transmit 0x00
    ${third}=                 Execute Command    ${LCD} Transmit 0x00
    Should Contain            ${first}    0x85
    Should Contain            ${second}    0x85
    Should Contain            ${third}    0x52

    Send Command              0x0A
    Execute Command           ${LCD} OnGPIO 0 true
    ${power}=                 Execute Command    ${LCD} Transmit 0x00
    Should Contain            ${power}    0x9C

Saves A PNG Screenshot And A Stable Hash
    Initialise Screen
    Set Window                0    1    0    0
    Write Pixels              0xF8    0x00    0x07    0xE0
    ${path}=                  Set Variable    ${TEMPDIR}/st7789-screenshot-test.png
    Remove File               ${path}

    Execute Command           ${LCD} SaveScreenshot @${path}

    File Should Exist         ${path}
    ${signature}=             Evaluate    open(r'${path}', 'rb').read(8).hex()
    Should Be Equal           ${signature}    89504e470d0a1a0a
    ${size}=                  Get File Size    ${path}
    Should Be True            ${size} > 100
    Remove File               ${path}

    ${first_hash}=            Execute Command    ${LCD} ShownImageHash
    ${second_hash}=           Execute Command    ${LCD} ShownImageHash
    Should Be Equal           ${first_hash}    ${second_hash}
    Send Command              0x28
    ${dark_hash}=             Execute Command    ${LCD} ShownImageHash
    Should Not Be Equal       ${first_hash}    ${dark_hash}
