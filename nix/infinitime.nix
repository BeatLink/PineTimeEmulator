# InfiniTime firmware images for the PineTime, built from a pinned release.
{ pkgs, infinitime_source }:

let
  # ==========================================================================
  # Inputs
  # ==========================================================================

  version = "1.16.1";

  # Nordic moved its downloads here; the old developer.nordicsemi.com links return 403.
  nrf5_sdk = pkgs.fetchzip {
    name = "nRF5_SDK_15.3.0_59ac345";
    url = "https://nsscprodmedia.blob.core.windows.net/prod/software-and-other-downloads/sdks/nrf5/binaries/nrf5sdk153059ac345.zip";
    hash = "sha256-pfmhbpgVv5x2ju489XcivguwpnofHbgVA7bFUJRTj08=";
  };

  # The MCUboot bootloader InfiniTime runs behind, from pinetime-mcuboot-bootloader releases.
  bootloader_version = "1.0.2";
  bootloader_hex = pkgs.fetchurl {
    url = "https://github.com/InfiniTimeOrg/pinetime-mcuboot-bootloader/releases/download/${bootloader_version}/bootloader-${bootloader_version}.hex";
    hash = "sha256-InibIi57YYMeTNtAka+XJlnGwFIvOM/nvpATPgtvOWg=";
  };

  # imgtool.py needs cryptography, intelhex, click and cbor; lv_img_conv.py needs pillow.
  python = pkgs.python3.withPackages (python_packages: with python_packages; [
    cbor
    click
    cryptography
    intelhex
    pillow
  ]);

  # ==========================================================================
  # Build
  # ==========================================================================

  # Builds both the plain app and the MCUboot app in one pass, since they share every object.
  infinitime_build = pkgs.stdenv.mkDerivation {
    pname = "infinitime-build";
    inherit version;
    src = infinitime_source;

    nativeBuildInputs = [
      pkgs.cmake
      pkgs.gcc-arm-embedded-13
      pkgs.lv_font_conv
      pkgs.gnupatch
      python
    ];

    # The firmware is cross-compiled by the toolchain file, so the host compiler hooks stay out.
    dontFixup = true;

    postPatch = ''
      patchShebangs tools src/resources src/displayapp/fonts
      # The font scripts call /usr/bin/env, which the build sandbox does not have.
      substituteInPlace src/displayapp/fonts/generate.py src/resources/generate-fonts.py \
        --replace-fail "'/usr/bin/env', 'patch'" "'patch'"
      # The source has no .git, so the version screen gets the pinned commit instead.
      substituteInPlace CMakeLists.txt \
        --replace-fail "COMMAND git rev-parse --short HEAD" "COMMAND echo ${infinitime_source.shortRev or "release"}"
    '';

    cmakeFlags = [
      "-DCMAKE_BUILD_TYPE=Release"
      "-DARM_NONE_EABI_TOOLCHAIN_PATH=${pkgs.gcc-arm-embedded-13}"
      "-DNRF5_SDK_PATH=${nrf5_sdk}"
      "-DBUILD_RESOURCES=1"
    ];

    buildPhase = ''
      runHook preBuild
      cmake --build . -j$NIX_BUILD_CORES --target pinetime-app pinetime-mcuboot-app
      runHook postBuild
    '';

    installPhase = ''
      runHook preInstall
      mkdir -p $out
      cp src/pinetime-app-${version}.{out,hex,bin,map} $out/
      cp src/pinetime-mcuboot-app-${version}.{out,hex,bin,map} $out/
      cp src/pinetime-mcuboot-app-image-${version}.{hex,bin} $out/
      cp src/resources/infinitime-resources-${version}.zip $out/
      runHook postInstall
    '';
  };

in
{
  # ==========================================================================
  # Packages
  # ==========================================================================

  # The app linked at 0x0 with no bootloader, as InfiniEmu runs it.
  infinitime-app = pkgs.runCommand "infinitime-app-${version}" { } ''
    mkdir -p $out
    cp ${infinitime_build}/pinetime-app-${version}.{out,hex,bin,map} $out/
    cp ${infinitime_build}/infinitime-resources-${version}.zip $out/
    cat > $out/LOAD-ADDRESSES.txt <<EOF
    InfiniTime ${version}, plain app with no bootloader.
    pinetime-app-${version}.bin   load at 0x00000000 (vector table at 0x0)
    pinetime-app-${version}.hex   carries its own addresses (from 0x00000000)
    pinetime-app-${version}.out   ELF, loads at its own addresses
    infinitime-resources-${version}.zip   files for the littlefs on SPI flash, not flash images
    EOF
  '';

  # The MCUboot bootloader plus the app image it boots from slot 0.
  infinitime-mcuboot = pkgs.runCommand "infinitime-mcuboot-${version}" { } ''
    mkdir -p $out
    cp ${bootloader_hex} $out/bootloader-${bootloader_version}.hex
    cp ${infinitime_build}/pinetime-mcuboot-app-image-${version}.{bin,hex} $out/
    cp ${infinitime_build}/pinetime-mcuboot-app-${version}.{out,map} $out/
    cp ${infinitime_build}/infinitime-resources-${version}.zip $out/
    cat > $out/LOAD-ADDRESSES.txt <<EOF
    InfiniTime ${version} behind MCUboot bootloader ${bootloader_version}.
    bootloader-${bootloader_version}.hex   carries its own addresses (from 0x00000000)
    pinetime-mcuboot-app-image-${version}.bin   load at 0x00008000 (32-byte MCUboot header, vector table at 0x00008020)
    pinetime-mcuboot-app-image-${version}.hex   carries its own addresses (from 0x00008000)
    pinetime-mcuboot-app-${version}.out   ELF of the app alone, for symbols; linked at 0x00008020
    The bootloader draws its boot logo from SPI flash 0x0; with an erased SPI flash it shows garbage.
    EOF
  '';
}
