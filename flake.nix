# SPDX-License-Identifier: GPL-3.0-or-later
{
  description = "PineTimeEmulator: the PineTime smartwatch, emulated so its firmware runs unchanged";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # InfiniTime release the firmware packages build, with its lvgl, littlefs and arduinoFFT submodules.
    infinitime = {
      url = "git+https://github.com/InfiniTimeOrg/InfiniTime?ref=refs/tags/1.16.1&submodules=1";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, flake-utils, infinitime }:
    flake-utils.lib.eachSystem [ "x86_64-linux" "aarch64-linux" ] (system:
      let
        pkgs = import nixpkgs { inherit system; };
        infinitime_packages = import ./nix/infinitime.nix {
          inherit pkgs;
          infinitime_source = infinitime;
        };
        pinetime_engine = pkgs.callPackage ./nix/pinetime-engine.nix { };
        pinetime_test = import ./nix/pinetime-test.nix { inherit pkgs pinetime_engine; };
        pinetime_emulator = import ./nix/pinetime-emulator.nix {
          inherit pkgs pinetime_engine;
          infinitime_app = infinitime_packages.infinitime-app;
        };
      in
      {
        packages = {
          inherit (infinitime_packages) infinitime-app infinitime-mcuboot;
          pinetime-emulator = pinetime_emulator;
          pinetime-engine = pinetime_engine;
          pinetime-test = pinetime_test;
          default = pinetime_emulator;
        };

        # `nix run . -- --infinitime` opens the watch window.
        apps.default = {
          type = "app";
          program = "${pinetime_emulator}/bin/pinetime-emulator";
        };

        # `nix flake check` runs every Rust test.
        checks.engine-tests = pinetime_engine.overrideAttrs (_: {
          pname = "pinetime-engine-tests";
          doCheck = true;
          cargoTestFlags = [ "--workspace" ];
          # The CPU tests assemble their programs with the Arm toolchain.
          nativeCheckInputs = [ pkgs.gcc-arm-embedded ];
        });

        devShells.default = pkgs.mkShell {
          packages = [
            pkgs.cargo
            pkgs.rustc
            pkgs.rustfmt
            pkgs.clippy
            pkgs.gcc-arm-embedded
            pkgs.python3
            pkgs.python3Packages.pytest
            pinetime_engine
            pinetime_test
            pinetime_emulator
          ];

          shellHook = ''
            echo "PineTimeEmulator"
            echo "  cargo test                      run the emulator's tests"
            echo "  pinetime-engine                 open the emulator monitor on standard input"
            echo "  pinetime-test SUITE.robot       run Robot suites, as renode-test did"
            echo "  pinetime-emulator --infinitime  open the watch window"
            echo "  pytest tests/                   run the window tests"
          '';
        };
      });
}
