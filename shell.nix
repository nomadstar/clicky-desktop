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
    openssl
    libGL
    libxkbcommon
    wayland
    libx11
    libxcursor
    libxrandr
    libxi
    libxinerama
    systemd
    xdotool
    grim
  ];

  LIBCLANG_PATH = "${pkgs.libclang.lib}/lib";
}
