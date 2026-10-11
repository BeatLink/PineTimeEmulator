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
...               saadc: Analog.NRF52832_SAADC @ sysbus 0x40007000 { -> nvic@7 };
...               temp: Miscellaneous.NRF52832_TEMP @ sysbus 0x4000C000 { -> nvic@12 };
...               ccm_aar: Miscellaneous.NRF52832_CCM_AAR @ sysbus 0x4000F000 { -> nvic@15 };
...               qdec: Miscellaneous.NRF52832_QDEC @ sysbus 0x40012000 { -> nvic@18 };
...               comp_lpcomp: Miscellaneous.NRF52832_COMP_LPCOMP @ sysbus 0x40013000 { -> nvic@19 };
...               pwm0: Miscellaneous.NRF52832_PWM @ sysbus 0x4001C000 { IRQ -> nvic@28 };
...               mwu: Miscellaneous.NRF52832_MWU @ sysbus 0x40020000 { -> nvic@32 }

*** Keywords ***
Create Machine
    Execute Command           include @${PERIPHERALS}/NRF52832_SAADC.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_TEMP.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_CCM_AAR.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_MWU.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_PWM.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_QDEC.cs
    Execute Command           include @${PERIPHERALS}/NRF52832_COMP_LPCOMP.cs
    Execute Command           mach create
    Execute Command           machine LoadPlatformDescriptionFromString '${PLATFORM}'
    # A vector table and an endless loop in RAM, so virtual time can run.
    Execute Command           sysbus WriteDoubleWord 0x20000000 0x20008000
    Execute Command           sysbus WriteDoubleWord 0x20000004 0x20000101
    Execute Command           sysbus WriteDoubleWord 0x20000100 0xE7FEE7FE
    Execute Command           cpu VectorTableOffset 0x20000000

Register Should Be
    [Arguments]               ${address}    ${expected}
    ${value}=                 Execute Command    sysbus ReadDoubleWord ${address}
    Should Be Equal As Integers    ${value}    ${expected}

Write Words
    [Arguments]               ${address}    @{words}
    FOR    ${word}    IN    @{words}
        Execute Command       sysbus WriteDoubleWord ${address} ${word}
        ${address}=           Evaluate    ${address} + 4
    END

Run For Milliseconds
    [Arguments]               ${milliseconds}
    ${seconds}=               Evaluate    ${milliseconds} / 1000
    Execute Command           emulation RunFor "${seconds}"

Duty Should Be
    [Arguments]               ${channel}    ${expected}
    ${value}=                 Execute Command    pwm0 GetDutyCycle ${channel}
    Should Be Equal As Numbers    ${value}    ${expected}    precision=4

Configure CCM
    [Arguments]               ${counter_low}    ${counter_high}    ${direction}
    # The session key from the Bluetooth Core sample data, MSB first.
    Write Words               0x20001000    0x521BAD99    0x3E7EA326    0x8E3B8E05    0x66C6C227
    Write Words               0x20001010    ${counter_low}    ${counter_high}
    # The direction byte, then the IV 0xDEAFBABEBADCAB24 least significant byte first.
    ${direction_word}=        Evaluate    0xDCAB2400 | ${direction}
    Write Words               0x20001018    ${direction_word}    0xAFBABEBA    0x000000DE
    Execute Command           sysbus WriteDoubleWord 0x4000F500 2
    Execute Command           sysbus WriteDoubleWord 0x4000F508 0x20001000

Run CCM
    [Arguments]               ${mode}    ${input}    ${output}
    Execute Command           sysbus WriteDoubleWord 0x4000F504 ${mode}
    Execute Command           sysbus WriteDoubleWord 0x4000F50C ${input}
    Execute Command           sysbus WriteDoubleWord 0x4000F510 ${output}
    Execute Command           sysbus WriteDoubleWord 0x4000F104 0
    Execute Command           sysbus WriteDoubleWord 0x4000F200 1
    Execute Command           sysbus WriteDoubleWord 0x4000F000 1
    Register Should Be        0x4000F100    1
    Register Should Be        0x4000F104    1

Configure AAR
    # A wrong IRK first, then the Bluetooth Core sample IRK 0xEC0234A357C8AD05341010A60A397D9B, MSB first.
    Write Words               0x20001FF0    0x11111111    0x22222222    0x33333333    0x44444444
    Write Words               0x20002000    0xA33402EC    0x05ADC857    0xA6101034    0x9B7D390A
    Execute Command           sysbus WriteDoubleWord 0x4000F500 3
    Execute Command           sysbus WriteDoubleWord 0x4000F504 2
    Execute Command           sysbus WriteDoubleWord 0x4000F508 0x20001FF0
    Execute Command           sysbus WriteDoubleWord 0x4000F510 0x20002100

Start PWM Sequence
    [Arguments]               ${sequence}    ${pointer}    ${count}
    ${sequence_base}=         Evaluate    0x4001C520 + 0x20 * ${sequence}
    Execute Command           sysbus WriteDoubleWord ${sequence_base} ${pointer}
    ${address}=               Evaluate    ${sequence_base} + 4
    Execute Command           sysbus WriteDoubleWord ${address} ${count}
    ${address}=               Evaluate    ${sequence_base} + 8
    Execute Command           sysbus WriteDoubleWord ${address} 0
    ${task}=                  Evaluate    0x4001C008 + 4 * ${sequence}
    Execute Command           sysbus WriteDoubleWord ${task} 1

Configure PWM With A One Millisecond Period
    Execute Command           sysbus WriteDoubleWord 0x4001C500 1
    Execute Command           sysbus WriteDoubleWord 0x4001C508 16000
    Execute Command           sysbus WriteDoubleWord 0x4001C50C 0
    Execute Command           sysbus WriteDoubleWord 0x4001C510 0

*** Test Cases ***
SAADC Converts A Single-Ended Input At 10 Bits
    Execute Command           saadc SetInputVoltage 0 1.65
    Execute Command           sysbus WriteDoubleWord 0x40007500 1
    # AIN0, gain 1/6, internal 0.6 V reference (the CONFIG reset value), 10 bits.
    Execute Command           sysbus WriteDoubleWord 0x40007510 1
    Execute Command           sysbus WriteDoubleWord 0x400075F0 1
    Execute Command           sysbus WriteDoubleWord 0x4000762C 0x20000400
    Execute Command           sysbus WriteDoubleWord 0x40007630 1
    Execute Command           sysbus WriteDoubleWord 0x40007304 0x2
    Execute Command           sysbus WriteDoubleWord 0x40007000 1
    Register Should Be        0x40007100    1
    Execute Command           sysbus WriteDoubleWord 0x40007004 1
    # 1.65 V * (1/6) / 0.6 V * 1024 = 469.33, so 469.
    ${value}=                 Execute Command    sysbus ReadWord 0x20000400
    Should Be Equal As Integers    ${value}    469
    Register Should Be        0x40007634    1
    Register Should Be        0x40007104    1
    Register Should Be        0x40007108    1
    Register Should Be        0x4000710C    1
    # END is enabled, so SAADC_IRQn (7) is pending in the NVIC.
    ${pending}=               Execute Command    sysbus ReadDoubleWord 0xE000E200
    ${bit}=                   Evaluate    (${pending} >> 7) & 1
    Should Be Equal As Integers    ${bit}    1

SAADC Converts Two Channels At 12 Bits Into RAM
    Execute Command           saadc SetInputVoltage 7 2.0
    Execute Command           sysbus WriteDoubleWord 0x40007500 1
    # Channel 0 reads VDD (3.3 V by default), channel 1 reads AIN7, both at gain 1/6 and the internal reference.
    Execute Command           sysbus WriteDoubleWord 0x40007510 9
    Execute Command           sysbus WriteDoubleWord 0x40007520 8
    Execute Command           sysbus WriteDoubleWord 0x400075F0 2
    Execute Command           sysbus WriteDoubleWord 0x4000762C 0x20000400
    Execute Command           sysbus WriteDoubleWord 0x40007630 2
    Execute Command           sysbus WriteDoubleWord 0x40007000 1
    Execute Command           sysbus WriteDoubleWord 0x40007004 1
    # 3.3 / 6 / 0.6 * 4096 = 3754.67 and 2.0 / 6 / 0.6 * 4096 = 2275.56.
    ${first}=                 Execute Command    sysbus ReadWord 0x20000400
    Should Be Equal As Integers    ${first}    3754
    ${second}=                Execute Command    sysbus ReadWord 0x20000402
    Should Be Equal As Integers    ${second}    2275
    Register Should Be        0x40007634    2
    Register Should Be        0x40007104    1

SAADC Uses The VDD Quarter Reference, Gain And Limits
    Execute Command           saadc SetInputVoltage 2 0.5
    Execute Command           saadc SetSupplyVoltage 3.0
    Execute Command           sysbus WriteDoubleWord 0x40007500 1
    Execute Command           sysbus WriteDoubleWord 0x40007510 3
    # Gain 1/4 (2), VDD/4 reference, 10 bits: 0.5 * 0.25 / 0.75 * 1024 = 170.67.
    Execute Command           sysbus WriteDoubleWord 0x40007518 0x00021200
    # A high limit of 100 makes the result raise CH[0].LIMITH.
    Execute Command           sysbus WriteDoubleWord 0x4000751C 0x00648000
    Execute Command           sysbus WriteDoubleWord 0x4000762C 0x20000400
    Execute Command           sysbus WriteDoubleWord 0x40007630 1
    Execute Command           sysbus WriteDoubleWord 0x40007000 1
    Execute Command           sysbus WriteDoubleWord 0x40007004 1
    ${value}=                 Execute Command    sysbus ReadWord 0x20000400
    Should Be Equal As Integers    ${value}    170
    Register Should Be        0x40007118    1
    Register Should Be        0x4000711C    0
    Execute Command           sysbus WriteDoubleWord 0x4000700C 1
    Register Should Be        0x40007110    1
    Execute Command           sysbus WriteDoubleWord 0x40007008 1
    Register Should Be        0x40007114    1

TEMP Reports The Set Temperature In Quarter Degrees
    Execute Command           temp SetTemperature 31.5
    Execute Command           sysbus WriteDoubleWord 0x4000C000 1
    Register Should Be        0x4000C100    1
    Register Should Be        0x4000C508    126
    Execute Command           temp SetTemperature -10.25
    Execute Command           sysbus WriteDoubleWord 0x4000C100 0
    Execute Command           sysbus WriteDoubleWord 0x4000C000 1
    Register Should Be        0x4000C508    0xFFFFFFD7
    Register Should Be        0x4000C520    0x320
    Register Should Be        0x4000C554    0x3DD0
    Register Should Be        0x4000C570    0x50
    Execute Command           sysbus WriteDoubleWord 0x4000C520 0x123
    Register Should Be        0x4000C520    0x123

CCM Encrypts The Bluetooth Core Sample Packet
    Configure CCM             0x00000000    0x00000000    1
    # LL_START_ENC_RSP, master to slave: header 0x0F, length 1, payload 0x06.
    Execute Command           sysbus WriteDoubleWord 0x20001100 0x0600010F
    Run CCM                   0    0x20001100    0x20001200
    # 0F 05 00 9F CD A7 F4 48: the encrypted opcode and its MIC from the specification.
    Register Should Be        0x20001200    0x9F00050F
    Register Should Be        0x20001204    0x48F4A7CD

CCM Encrypts The Slave To Master Sample Packet
    Configure CCM             0x00000000    0x00000000    0
    Execute Command           sysbus WriteDoubleWord 0x20001100 0x0600010F
    Run CCM                   0    0x20001100    0x20001200
    Register Should Be        0x20001200    0xA300050F
    Register Should Be        0x20001204    0x15A4134C

CCM Round Trips A 27-Byte Payload And Detects A Bad MIC
    Configure CCM             0x3456789A    0x00000012    0
    Write Words               0x20001100    0x01001B02    0x05040302    0x09080706    0x0D0C0B0A
    ...                       0x11100F0E    0x15141312    0x19181716    0x00001B1A
    Run CCM                   0    0x20001100    0x20001200
    # The expected ciphertext and MIC come from an independent AES-CCM implementation.
    Register Should Be        0x20001200    0x8D001F02
    Register Should Be        0x20001204    0xC41D1825
    Register Should Be        0x20001210    0x6E08490B
    Register Should Be        0x2000121C    0xC5FDDBAA
    Register Should Be        0x20001220    0x000022B2
    Run CCM                   1    0x20001200    0x20001300
    Register Should Be        0x4000F400    1
    Register Should Be        0x20001300    0x01001B02
    Register Should Be        0x20001304    0x05040302
    Register Should Be        0x20001318    0x19181716
    ${last}=                  Execute Command    sysbus ReadWord 0x2000131C
    Should Be Equal As Integers    ${last}    0x1B1A
    # Flip one MIC bit: the payload still decrypts but MICSTATUS reports the failure.
    Execute Command           sysbus WriteDoubleWord 0x20001220 0x000022B3
    Run CCM                   1    0x20001200    0x20001400
    Register Should Be        0x4000F400    0
    Register Should Be        0x20001404    0x05040302

AAR Resolves The Bluetooth Core Sample Address
    Configure AAR
    # A 3-byte header, then the address 0x708194_0DFBAA (prand 0x708194, hash 0x0DFBAA) LSB first.
    Write Words               0x20002100    0xAA000000    0x81940DFB    0x00000070
    Execute Command           sysbus WriteDoubleWord 0x4000F000 1
    Register Should Be        0x4000F100    1
    Register Should Be        0x4000F104    1
    Register Should Be        0x4000F108    0
    Register Should Be        0x4000F400    1

AAR Rejects An Address No IRK Resolves
    Configure AAR
    Write Words               0x20002100    0xAB000000    0x81940DFB    0x00000070
    Execute Command           sysbus WriteDoubleWord 0x4000F000 1
    Register Should Be        0x4000F100    1
    Register Should Be        0x4000F104    0
    Register Should Be        0x4000F108    1

MWU Registers Read Back
    Execute Command           sysbus WriteDoubleWord 0x40020620 0x20001000
    Execute Command           sysbus WriteDoubleWord 0x40020624 0x20001FFF
    Register Should Be        0x40020620    0x20001000
    Register Should Be        0x40020624    0x20001FFF
    Execute Command           sysbus WriteDoubleWord 0x40020514 0xFFFFFFFF
    Register Should Be        0x40020510    0x0F0000FF
    Execute Command           sysbus WriteDoubleWord 0x40020518 0x0F000000
    Register Should Be        0x40020510    0x000000FF
    Execute Command           sysbus WriteDoubleWord 0x40020324 0x3
    Register Should Be        0x40020320    0x3
    Execute Command           sysbus WriteDoubleWord 0x40020304 0x01000000
    Register Should Be        0x40020300    0x01000000
    Execute Command           sysbus WriteDoubleWord 0x400206D8 0x00FF00FF
    Register Should Be        0x400206D8    0x00FF00FF
    Register Should Be        0x400206D0    0
    Execute Command           sysbus WriteDoubleWord 0x40020160 1
    Register Should Be        0x40020160    1

PWM Plays A Sequence In Virtual Time
    Configure PWM With A One Millisecond Period
    # Four steps of one period each: 25 %, 50 %, 75 %, then 100 % high.
    Write Words               0x20003000    0x9F408FA0    0xBE80AEE0
    Execute Command           sysbus WriteDoubleWord 0x4001C304 0x10
    Start PWM Sequence        0    0x20003000    4
    Register Should Be        0x4001C108    1
    Duty Should Be            0    0.25
    Run For Milliseconds      1.5
    Duty Should Be            0    0.5
    Duty Should Be            3    0.5
    Run For Milliseconds      1.4
    Duty Should Be            0    0.75
    Register Should Be        0x4001C110    0
    # The last value is applied, and SEQEND raised, 3 ms after the start.
    Run For Milliseconds      0.2
    Register Should Be        0x4001C110    1
    Duty Should Be            0    1.0
    ${output}=                Execute Command    pwm0 Output0 IsSet
    Should Contain            ${output}    True
    ${pending}=               Execute Command    sysbus ReadDoubleWord 0xE000E200
    ${bit}=                   Evaluate    (${pending} >> 28) & 1
    Should Be Equal As Integers    ${bit}    1

PWM Inverts Polarity Without Bit 15
    Configure PWM With A One Millisecond Period
    # Individual load: 4000 without the polarity bit is 75 % high; 0x8000 | 4000 is 25 %.
    Execute Command           sysbus WriteDoubleWord 0x4001C510 2
    Write Words               0x20003000    0x8FA00FA0    0x00008000
    Start PWM Sequence        0    0x20003000    4
    Duty Should Be            0    0.75
    Duty Should Be            1    0.25
    Duty Should Be            2    0.0
    Duty Should Be            3    1.0

PWM Loops Both Sequences And Stops On LOOPSDONE
    Configure PWM With A One Millisecond Period
    Write Words               0x20003000    0x8000AEE0
    Execute Command           sysbus WriteDoubleWord 0x4001C514 2
    # LOOPSDONE_STOP.
    Execute Command           sysbus WriteDoubleWord 0x4001C200 0x10
    Execute Command           sysbus WriteDoubleWord 0x4001C540 0x20003002
    Execute Command           sysbus WriteDoubleWord 0x4001C544 1
    Execute Command           sysbus WriteDoubleWord 0x4001C548 0
    Start PWM Sequence        0    0x20003000    1
    Duty Should Be            0    0.75
    Run For Milliseconds      1.5
    Duty Should Be            0    0.0
    Register Should Be        0x4001C10C    1
    # Two loops of one 1 ms step in each sequence end after 4 ms.
    Run For Milliseconds      2.4
    Register Should Be        0x4001C11C    0
    Run For Milliseconds      0.2
    Register Should Be        0x4001C11C    1
    Register Should Be        0x4001C104    1
    ${running}=               Execute Command    pwm0 IsRunning
    Should Contain            ${running}    False

PWM Stops On The SEQEND Short
    Configure PWM With A One Millisecond Period
    Write Words               0x20003000    0x80008000
    Execute Command           sysbus WriteDoubleWord 0x4001C200 0x1
    Start PWM Sequence        0    0x20003000    2
    Register Should Be        0x4001C104    0
    Run For Milliseconds      1.1
    Register Should Be        0x4001C110    1
    Register Should Be        0x4001C104    1
    Duty Should Be            0    0.0

COMP And LPCOMP Accept The Errata Trim Write
    Register Should Be        0x40013508    4
    Execute Command           sysbus WriteDoubleWord 0x40013540 0x15
    Register Should Be        0x40013540    0x15
    Execute Command           sysbus WriteDoubleWord 0x40013500 1
    Execute Command           sysbus WriteDoubleWord 0x40013000 1
    Register Should Be        0x40013100    1
    Execute Command           comp_lpcomp SetInputAbove true
    Register Should Be        0x40013108    1
    Register Should Be        0x4001310C    1
    Register Should Be        0x40013400    1

QDEC Has Its Reset Values
    Register Should Be        0x40012540    0x10
    Register Should Be        0x4001251C    0xFFFFFFFF
    Register Should Be        0x40012524    0xFFFFFFFF
    Execute Command           sysbus WriteDoubleWord 0x40012004 1
    Register Should Be        0x40012110    1
