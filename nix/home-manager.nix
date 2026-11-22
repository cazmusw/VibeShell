{ config, lib, pkgs, defaultPackage }:
let
  cfg = config.programs.mosaic-shell;
  format = pkgs.formats.toml {};
  unit = {
    PartOf = [ "mosaic-session.target" ];
    BindsTo = [ "mosaic-session.target" ];
    After = [ "graphical-session.target" ];
    StartLimitIntervalSec = 30;
    StartLimitBurst = 3;
  };
in {
  options.programs.mosaic-shell = {
    enable = lib.mkEnableOption "Mosaic shell vertical slice for a UWSM graphical session";
    package = lib.mkOption { type = lib.types.package; default = defaultPackage; };
    bluetoothPairing.enable = lib.mkEnableOption "the Mosaic Bluetooth pairing agent";
    settings = lib.mkOption {
      type = lib.types.nullOr (lib.types.attrsOf lib.types.anything);
      default = null;
      description = "Schema 2: pins, theme, dock, general, panel. Declared configuration is read-only; null allows editing in the GUI.";
    };
  };
  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package ];
    xdg.configFile = lib.mkIf (cfg.settings != null) {
      "mosaic-shell/config.toml".source = format.generate "mosaic-config.toml"
        ({ schema_version = 2; revision = 0; pins = []; } // cfg.settings);
    };
    systemd.user.targets.mosaic-session = {
      Unit = {
        Description = "Mosaic shell graphical session";
        BindsTo = [ "graphical-session.target" ];
        After = [ "graphical-session.target" ];
      };
      Install.WantedBy = [ "graphical-session.target" ];
    };
    systemd.user.services.mosaic-services = {
      Unit = unit // { Description = "Mosaic shell backend"; };
      Service = {
        ExecStart = "${cfg.package}/bin/mosaic-services";
        Restart = "on-failure";
        RestartSec = 1;
        Environment = lib.optional cfg.bluetoothPairing.enable "MOSAIC_BLUETOOTH_PAIRING=1";
      };
      Install.WantedBy = [ "mosaic-session.target" ];
    };
    systemd.user.services.mosaic-shell = {
      Unit = unit // { Description = "Mosaic panel and Dock"; Wants = [ "mosaic-services.service" ]; After = unit.After ++ [ "mosaic-services.service" ]; };
      Service = { ExecStart = "${cfg.package}/bin/mosaic-shell"; Restart = "on-failure"; RestartSec = 1; };
      Install.WantedBy = [ "mosaic-session.target" ];
    };
  };
}
