# SPDX-License-Identifier: GPL-3.0-or-later
# The pinetime-emulator command: the frontend/ window, wrapped with Renode, SDL2, a font and the emulator tree.
{ pkgs, renode, infinitime_app }:

let
  lib = pkgs.lib;
  python_packages = pkgs.python3Packages;

  # The Renode side of the emulator that the window starts: board scripts, platform files and C# models.
  emulator_tree = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../peripherals
      ../platform
      ../scripts
    ];
  };
in
python_packages.buildPythonApplication {
  pname = "pinetime-emulator";
  version = "0.1.0";
  pyproject = true;
  src = ../frontend;

  build-system = [ python_packages.setuptools ];
  dependencies = [ python_packages.pysdl2 ];

  makeWrapperArgs = [
    "--set-default" "PINETIME_EMULATOR_RENODE" "${renode}/bin/renode"
    "--set-default" "PINETIME_EMULATOR_ROOT" "${emulator_tree}"
    "--set-default" "PINETIME_EMULATOR_INFINITIME" "${infinitime_app}/pinetime-app-${lib.getVersion infinitime_app}.out"
    "--set-default" "PINETIME_EMULATOR_FONT" "${pkgs.dejavu_fonts}/share/fonts/truetype/DejaVuSansMono.ttf"
    "--set-default" "PYSDL2_DLL_PATH" (lib.makeLibraryPath [ pkgs.SDL2 pkgs.SDL2_ttf ])
  ];

  pythonImportsCheck = [ "pinetime_emulator.main" ];

  meta = {
    description = "A desktop window onto the Renode PineTime emulator";
    mainProgram = "pinetime-emulator";
    license = lib.licenses.gpl3Plus;
  };
}
