{ lib, rustPlatform, pkg-config, wrapGAppsHook4, gtk4, gtk4-layer-shell, wayland, makeWrapper, systemd, dbus, labwc }:
rustPlatform.buildRustPackage {
  pname = "mosaic-shell";
  version = "0.2.0";
  src = lib.cleanSource ../.;
  # Tests paused at the user's explicit request; build validation remains enabled.
  doCheck = false;
  cargoLock.lockFile = ../Cargo.lock;
  nativeBuildInputs = [ pkg-config wrapGAppsHook4 makeWrapper ];
  buildInputs = [ gtk4 gtk4-layer-shell wayland ];
  postInstall = ''
    install -Dm755 scripts/mosaic-session "$out/bin/mosaic-session"
    patchShebangs "$out/bin/mosaic-session"
    wrapProgram "$out/bin/mosaic-session" --prefix PATH : "$out/bin:${lib.makeBinPath [ systemd dbus labwc ]}"
  '';
  meta = {
    description = "Experimental Rust desktop shell for Labwc";
    license = lib.licenses.mit;
    platforms = [ "x86_64-linux" "aarch64-linux" ];
    mainProgram = "mosaic-shell";
  };
}
