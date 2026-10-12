# SPDX-License-Identifier: GPL-3.0-or-later
# The pinetime-test command: runs Robot Framework suites against pinetime-engine, in place of renode-test.
{ pkgs, pinetime_engine }:

let
  robot_python = pkgs.python3.withPackages (packages: [ packages.robotframework ]);
  keywords = ../robot;
in
pkgs.writeShellApplication {
  name = "pinetime-test";
  runtimeInputs = [ robot_python ];
  text = ''
    # Takes renode-test's -r (results folder) and -f (one test) as well as any robot option.
    arguments=()
    while [ $# -gt 0 ]; do
      case "$1" in
        -r) arguments+=(--outputdir "$2"); shift 2 ;;
        -f) arguments+=(--test "$2"); shift 2 ;;
        *) arguments+=("$1"); shift ;;
      esac
    done
    export PINETIME_EMULATOR_ENGINE="''${PINETIME_EMULATOR_ENGINE:-${pinetime_engine}/bin/pinetime-engine}"
    exec robot --pythonpath ${keywords} \
      --variable RENODEKEYWORDS:${keywords}/pinetime.resource \
      "''${arguments[@]}"
  '';
}
