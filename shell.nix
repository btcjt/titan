{ pkgs ? import <nixpkgs> {} }:

pkgs.mkShell {
  nativeBuildInputs = with pkgs; [
    # Rust toolchain
    rustc
    cargo
    rustfmt
    clippy

    # Tauri build deps
    pkg-config
    gobject-introspection

    # Tauri CLI
    cargo-tauri
  ];

  buildInputs = with pkgs; [
    # Tauri 2 / WebKitGTK (Linux)
    webkitgtk_4_1
    gtk3
    glib
    glib-networking
    libsoup_3
    cairo
    pango
    harfbuzz
    gdk-pixbuf
    atk

    # System libs
    openssl
    librsvg
    libappindicator-gtk3
    patchelf
    dbus
  ];

  # Point pkg-config and linker at the right libraries
  shellHook = ''
    export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules"
  '';
}
