# SPDX-License-Identifier: GPL-3.0-or-later
# A standalone AppImage of the watch window for systems without Nix: portable Python, SDL from PyPI, a static engine and InfiniTime.
{ pkgs, infinitime_app, hci_controller, appimage_runtime }:

let
  lib = pkgs.lib;

  # ==========================================================================
  # Inputs
  # ==========================================================================

  architecture = pkgs.stdenv.hostPlatform.uname.processor;
  python_version = "3.12";
  infinitime_version = lib.getVersion infinitime_app;

  # A relocatable CPython for glibc 2.28 and newer, from the python-appimage project.
  python_appimage = {
    x86_64-linux = pkgs.fetchurl {
      url = "https://github.com/niess/python-appimage/releases/download/python3.12/python3.12.15-cp312-cp312-manylinux_2_28_x86_64.AppImage";
      hash = "sha256-kpzmm0WVW/x0r4H9ZaZcOtJAWTyZs1dEZqlOvw+Yfr8=";
    };
    aarch64-linux = pkgs.fetchurl {
      url = "https://github.com/niess/python-appimage/releases/download/python3.12/python3.12.15-cp312-cp312-manylinux_2_28_aarch64.AppImage";
      hash = "sha256-t9JxvBRjDhA02imWslHKbmi3kFiVSRezOm2qdtcVlTo=";
    };
  }.${pkgs.stdenv.hostPlatform.system};

  pysdl2_wheel = pkgs.fetchurl {
    url = "https://files.pythonhosted.org/packages/5d/e2/399ea7900e7510096aeb41439e6f1540bef0bdf7e15cc2d8464e4adb71e8/PySDL2-0.9.17-py3-none-any.whl";
    hash = "sha256-/pI9v1x7J7vB6yv1ir+nk/jxP9euiyexvC3kmSC8vUE=";
  };

  # SDL2 and SDL2_ttf built for manylinux, which load X11 or Wayland from the host at run time.
  sdl_wheel = {
    x86_64-linux = pkgs.fetchurl {
      url = "https://files.pythonhosted.org/packages/2b/1c/91fb667c3968b420de5dd76e840259b44b876b6fc84c5889151633fcc06e/pysdl2_dll-2.32.10-py2.py3-none-manylinux_2_28_x86_64.whl";
      hash = "sha256-HmE5486DLZXYN2t39Mkoi+801rEx+ETypfzIhzzo5to=";
    };
    aarch64-linux = pkgs.fetchurl {
      url = "https://files.pythonhosted.org/packages/65/fc/8e35a36010e9dce25247991762ba3fc058c2626d323848c5e293447dcb35/pysdl2_dll-2.32.10-py2.py3-none-manylinux_2_28_aarch64.whl";
      hash = "sha256-JIscbGuh9D6WTy+bxyPuTOOYkqyoQK7m3iXakMRvaF0=";
    };
  }.${pkgs.stdenv.hostPlatform.system};

  # Linked statically against musl, so it runs on any Linux.
  static_engine = import ./pinetime-engine.nix { pkgs = pkgs.pkgsStatic; };

  # The size of the ELF runtime in front of an AppImage, which is where its squashfs starts.
  squashfs_offset = pkgs.writeText "squashfs_offset.py" ''
    import struct, sys
    header = open(sys.argv[1], "rb").read(64)
    section_table, = struct.unpack_from("<Q", header, 0x28)
    entry_size, entry_count = struct.unpack_from("<HH", header, 0x3A)
    print(section_table + entry_size * entry_count)
  '';

in
# ============================================================================
# The AppImage
# ============================================================================

pkgs.runCommand "pinetime-emulator-appimage-${architecture}"
{
  nativeBuildInputs = [ pkgs.python3 pkgs.squashfsTools pkgs.unzip ];
}
  ''
    app=$PWD/AppDir
    unsquashfs -q -n -d $app -o "$(python3 ${squashfs_offset} ${python_appimage})" ${python_appimage}
    chmod -R u+w $app

    # Python's own launcher, desktop entry and icon make way for the emulator's.
    rm -f $app/AppRun $app/.DirIcon $app/*.desktop $app/*.png
    rm -rf $app/usr/share/applications $app/usr/share/icons

    site=$app/opt/python${python_version}/lib/python${python_version}/site-packages
    unzip -q -o ${pysdl2_wheel} -d $site
    unzip -q -o ${sdl_wheel} -d $site
    cp -r ${../frontend/pinetime_emulator} $site/pinetime_emulator

    parts=$app/usr/lib/pinetime-emulator
    mkdir -p $parts/infinitime
    cp ${static_engine}/bin/pinetime-engine $parts/
    cp ${infinitime_app}/pinetime-app-${infinitime_version}.out ${infinitime_app}/LOAD-ADDRESSES.txt $parts/infinitime/
    cp ${hci_controller} $parts/nrf52840-hci-uart.elf
    cp ${pkgs.dejavu_fonts}/share/fonts/truetype/DejaVuSansMono.ttf $parts/

    substitute ${./appimage/AppRun} $app/AppRun \
      --subst-var-by python_version ${python_version} \
      --subst-var-by infinitime_version ${infinitime_version}
    chmod +x $app/AppRun
    cp ${./appimage/pinetime-emulator.desktop} $app/pinetime-emulator.desktop
    cp ${./appimage/pinetime-emulator.svg} $app/pinetime-emulator.svg
    ln -s pinetime-emulator.svg $app/.DirIcon

    mkdir -p $out
    mksquashfs $app image.squashfs -quiet -no-progress -root-owned -noappend -comp zstd
    cat ${appimage_runtime} image.squashfs > $out/PineTime_Emulator-${architecture}.AppImage
    chmod +x $out/PineTime_Emulator-${architecture}.AppImage
  ''
