{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    pkg-config
    cmake
    cargo
    rustc
    clang
  ];

  buildInputs = with pkgs; [
    alsa-lib
    pipewire
    gtk3
    libayatana-appindicator
    openssl
    libGL
    libgbm
    libxkbcommon
    wayland
    libx11
    libxcursor
    libxrandr
    libxi
    libxinerama
    libxcb
    systemd
    xdotool
    grim
  ];

  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
  shellHook = ''
    export LD_LIBRARY_PATH="${pkgs.lib.makeLibraryPath (with pkgs; [
      alsa-lib pipewire gtk3 libayatana-appindicator openssl libGL libgbm libxkbcommon wayland libx11 libxcursor libxrandr libxi libxinerama libxcb
    ])}:$LD_LIBRARY_PATH"
  '';
}
