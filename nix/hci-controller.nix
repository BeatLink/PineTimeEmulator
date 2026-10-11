# SPDX-License-Identifier: GPL-3.0-or-later
# Zephyr's hci_uart controller for the nRF52840, as Antmicro builds it for Renode's own Bluetooth examples.
{ fetchurl }:

fetchurl {
  name = "nrf52840dk-zephyr-ble-hci-uart.elf";
  url = "https://dl.antmicro.com/projects/renode/nrf52840dk--zephyr-ble_hci_uart.elf-s_2437336-4bf9cccb70ca95fb12667896eec3f777f39f90ee";
  hash = "sha256-ZCVG86U0fedkmmBo+CplBb1mO/WxTHzcNs+Zu3lCbyY=";
}
