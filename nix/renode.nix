# SPDX-License-Identifier: GPL-3.0-or-later
# Renode from nixpkgs with the fixes the PineTime emulator needs.
{ renode }:

renode.overrideAttrs (old: {
  patches = (old.patches or [ ]) ++ [
    # Cortex-M4 code such as Nordic's SoftDevice uses MOV.W Rd, SP, which Renode 1.17 decodes as illegal.
    ./renode-thumb2-mov-from-sp.patch
  ];
})
