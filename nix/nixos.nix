{ config, lib, ... }:
let cfg = config.services.mosaic-shell;
in {
  options.services.mosaic-shell = {
    enable = lib.mkEnableOption "system prerequisites for Mosaic";
    enableNetworkManager = lib.mkEnableOption "NetworkManager (explicit opt-in)";
    enableBluetooth = lib.mkEnableOption "BlueZ (explicit opt-in)";
  };
  config = lib.mkIf cfg.enable {
    programs.labwc.enable = true;
    security.polkit.enable = true;
    networking.networkmanager.enable = lib.mkIf cfg.enableNetworkManager true;
    hardware.bluetooth.enable = lib.mkIf cfg.enableBluetooth true;
  };
}
