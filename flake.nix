# SPDX-License-Identifier: GPL-3.0-or-later
{
  description = "PineTimeEmulator: the PineTime smartwatch emulated on Renode";

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
        # Renode with the CPU fixes the PineTime firmware needs; see nix/renode.nix.
        renode = pkgs.callPackage ./nix/renode.nix { };
        infinitime_packages = import ./nix/infinitime.nix {
          inherit pkgs;
          infinitime_source = infinitime;
        };
        pinetime_emulator = import ./nix/pinetime-emulator.nix {
          inherit pkgs renode;
          infinitime_app = infinitime_packages.infinitime-app;
          hci_controller = pkgs.callPackage ./nix/hci-controller.nix { };
        };
      in
      {
        packages = {
          inherit (infinitime_packages) infinitime-app infinitime-mcuboot;
          pinetime-emulator = pinetime_emulator;
          default = pinetime_emulator;
        };

        # `nix run . -- --infinitime` opens the watch window.
        apps.default = {
          type = "app";
          program = "${pinetime_emulator}/bin/pinetime-emulator";
        };

        devShells.default = pkgs.mkShell {
          packages = [
            renode
            pkgs.gcc-arm-embedded
            pkgs.python3
            pkgs.python3Packages.pytest
            pinetime_emulator
          ];

          shellHook = ''
            echo "PineTimeEmulator"
            echo "  renode-test tests/          run every model test"
            echo "  renode scripts/pinetime.resc   open the emulator monitor"
            echo "  pinetime-emulator --infinitime  open the watch window"
            echo "  pytest tests/                   run the window tests"
          '';
        };
      });
}
