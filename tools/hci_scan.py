#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""Scans for Bluetooth LE advertisers through an H4 HCI serial port, or connects to one and reads its name over GATT."""

import argparse
import os
import select
import termios
import time
import tty

# ============================================================================
# HCI packets
# ============================================================================

COMMAND_PACKET = 0x01
ACL_PACKET = 0x02
EVENT_PACKET = 0x04
DISCONNECTION_COMPLETE = 0x05
COMMAND_STATUS = 0x0F
COMMAND_COMPLETE = 0x0E
LE_META_EVENT = 0x3E
LE_ADVERTISING_REPORT = 0x02
LE_CONNECTION_COMPLETE = 0x01
LE_CREATE_CONNECTION = 0x200D

ATTRIBUTE_CHANNEL = 0x0004
READ_BY_TYPE_REQUEST = 0x08
READ_BY_TYPE_RESPONSE = 0x09
ERROR_RESPONSE = 0x01
DEVICE_NAME_UUID = 0x2A00

SET_EVENT_MASK = 0x0C01
RESET = 0x0C03
READ_BD_ADDRESS = 0x1009
LE_SET_SCAN_PARAMETERS = 0x200B
LE_SET_SCAN_ENABLE = 0x200C


def command(opcode, parameters=b""):
    return bytes([COMMAND_PACKET, opcode & 0xFF, opcode >> 8, len(parameters)]) + parameters


def read_packets(port, seconds):
    """Yields (packet type, first header field, payload) for every event or ACL packet within the time given."""
    buffer = b""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        ready, _, _ = select.select([port], [], [], 0.1)
        if ready:
            buffer += os.read(port, 4096)
        while buffer:
            if buffer[0] == EVENT_PACKET and len(buffer) >= 3:
                length = buffer[2]
                if len(buffer) < 3 + length:
                    break
                yield EVENT_PACKET, buffer[1], buffer[3:3 + length]
                buffer = buffer[3 + length:]
            elif buffer[0] == ACL_PACKET and len(buffer) >= 5:
                length = int.from_bytes(buffer[3:5], "little")
                if len(buffer) < 5 + length:
                    break
                yield ACL_PACKET, int.from_bytes(buffer[1:3], "little") & 0x0FFF, buffer[5:5 + length]
                buffer = buffer[5 + length:]
            elif buffer[0] in (EVENT_PACKET, ACL_PACKET):
                break
            else:
                buffer = buffer[1:]


def read_events(port, seconds):
    """Yields (event code, parameters) for every HCI event that arrives within the time given."""
    for kind, code, payload in read_packets(port, seconds):
        if kind == EVENT_PACKET:
            yield code, payload


def wait_for_completion(port, opcode, seconds=10):
    for event, parameters in read_events(port, seconds):
        if event == COMMAND_COMPLETE and int.from_bytes(parameters[1:3], "little") == opcode:
            return parameters[3:]
    raise TimeoutError(f"no Command Complete for 0x{opcode:04X}")


def reset_controller(port, seconds=60):
    """Sends HCI Reset until it is answered, since the controller ignores its UART while it boots."""
    end = time.monotonic() + seconds
    while time.monotonic() < end:
        os.write(port, command(RESET))
        try:
            return wait_for_completion(port, RESET, seconds=2)
        except TimeoutError:
            continue
    raise TimeoutError("the controller never answered HCI Reset")


# ============================================================================
# Advertising reports
# ============================================================================

def advertised_name(data):
    while len(data) >= 2:
        length, kind = data[0], data[1]
        if kind in (0x08, 0x09):
            return data[2:1 + length].decode(errors="replace")
        data = data[1 + length:]
    return None


def advertisers(parameters):
    count = parameters[1]
    offset = 2
    for _ in range(count):
        address = ":".join(f"{byte:02X}" for byte in reversed(parameters[offset + 2:offset + 8]))
        length = parameters[offset + 8]
        data = parameters[offset + 9:offset + 9 + length]
        rssi = parameters[offset + 9 + length] - 256
        offset += 10 + length
        yield address, advertised_name(data), rssi


# ============================================================================
# Connecting
# ============================================================================

def connect_and_read_name(port, address, seconds):
    peer = bytes(int(part, 16) for part in reversed(address.split(":")))
    # Scan 60 ms in 60 ms, random peer address, 30 ms interval, no latency, 720 ms supervision timeout.
    parameters = (0x60).to_bytes(2, "little") * 2 + bytes([0, 1]) + peer + bytes([0])
    parameters += (0x18).to_bytes(2, "little") * 2 + (0).to_bytes(2, "little") + (0x48).to_bytes(2, "little")
    parameters += (0).to_bytes(2, "little") * 2
    os.write(port, command(LE_CREATE_CONNECTION, parameters))

    handle = None
    for event, payload in read_events(port, seconds):
        if event == LE_META_EVENT and payload[0] == LE_CONNECTION_COMPLETE:
            if payload[1] != 0:
                raise RuntimeError(f"connection failed, status 0x{payload[1]:02X}")
            handle = int.from_bytes(payload[2:4], "little")
            print("connected, handle", handle, flush=True)
            break
    if handle is None:
        raise TimeoutError("no connection")

    request = bytes([READ_BY_TYPE_REQUEST]) + (1).to_bytes(2, "little") + (0xFFFF).to_bytes(2, "little")
    request += DEVICE_NAME_UUID.to_bytes(2, "little")
    # Host-to-controller ACL on LE uses packet boundary flag 00; controllers drop 10.
    l2cap = len(request).to_bytes(2, "little") + ATTRIBUTE_CHANNEL.to_bytes(2, "little") + request
    os.write(port, bytes([ACL_PACKET]) + handle.to_bytes(2, "little") + len(l2cap).to_bytes(2, "little") + l2cap)

    for kind, code, payload in read_packets(port, seconds):
        if kind == EVENT_PACKET and code == DISCONNECTION_COMPLETE:
            raise RuntimeError(f"disconnected, reason 0x{payload[3]:02X}")
        if kind == ACL_PACKET and int.from_bytes(payload[2:4], "little") == ATTRIBUTE_CHANNEL:
            attribute = payload[4:]
            if attribute[0] == READ_BY_TYPE_RESPONSE:
                print("device name", attribute[4:].decode(errors="replace"), flush=True)
                return
            if attribute[0] == ERROR_RESPONSE:
                raise RuntimeError(f"ATT error 0x{attribute[4]:02X}")
    raise TimeoutError("no answer to the GATT read")


# ============================================================================
# Main
# ============================================================================

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("port", help="the HCI serial port, such as /tmp/pinetime-hci")
    parser.add_argument("--seconds", type=float, default=20)
    parser.add_argument("--connect", metavar="ADDRESS", help="connect to this random address and read its name")
    arguments = parser.parse_args()

    port = os.open(arguments.port, os.O_RDWR | os.O_NOCTTY)
    tty.setraw(port)
    termios.tcflush(port, termios.TCIOFLUSH)

    reset_controller(port)
    os.write(port, command(READ_BD_ADDRESS))
    controller_address = wait_for_completion(port, READ_BD_ADDRESS)[1:7]
    print("controller", ":".join(f"{byte:02X}" for byte in reversed(controller_address)))

    # The default event mask leaves out LE Meta events, which carry the advertising reports.
    os.write(port, command(SET_EVENT_MASK, (0x20001FFFFFFFFFFF).to_bytes(8, "little")))
    wait_for_completion(port, SET_EVENT_MASK)

    if arguments.connect:
        connect_and_read_name(port, arguments.connect, arguments.seconds)
        return

    # Active scan, 10 ms interval and window, public address, accept all.
    os.write(port, command(LE_SET_SCAN_PARAMETERS, bytes([1, 0x10, 0, 0x10, 0, 0, 0])))
    wait_for_completion(port, LE_SET_SCAN_PARAMETERS)
    os.write(port, command(LE_SET_SCAN_ENABLE, bytes([1, 0])))
    wait_for_completion(port, LE_SET_SCAN_ENABLE)

    seen = set()
    for event, parameters in read_events(port, arguments.seconds):
        if event == LE_META_EVENT and parameters[0] == LE_ADVERTISING_REPORT:
            for address, name, rssi in advertisers(parameters):
                if (address, name) not in seen:
                    seen.add((address, name))
                    print("advertiser", address, name or "(no name)", f"{rssi} dBm", flush=True)


if __name__ == "__main__":
    main()
