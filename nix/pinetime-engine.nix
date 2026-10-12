# SPDX-License-Identifier: GPL-3.0-or-later
# The pinetime-engine command: the Rust emulator of the PineTime that the window and the tests drive.
{ pkgs }:

let
  lib = pkgs.lib;
in
pkgs.rustPlatform.buildRustPackage {
  pname = "pinetime-engine";
  version = "0.1.0";

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../crates
    ];
  };
  cargoLock.lockFile = ../Cargo.lock;

  cargoBuildFlags = [ "-p" "pinetime-emulator" ];
  # The tests run in the development shell and in `nix flake check`, not on every package build.
  doCheck = false;

  meta = {
    description = "An emulator of the PineTime smartwatch that runs its firmware images unchanged";
    mainProgram = "pinetime-engine";
    license = lib.licenses.gpl3Only;
  };
}
