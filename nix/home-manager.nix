flake: { config, lib, pkgs, ... }:
let
  cfg = config.programs.mosaic-shell;
  toml = pkgs.formats.toml { };
  settings = cfg.settings // { mode = cfg.configurationMode; schema_version = 1; };
in {
  options.programs.mosaic-shell = {
    enable = lib.mkEnableOption "experimental Mosaic shell";
    package = lib.mkOption { type = lib.types.package; default = flake.packages.${pkgs.stdenv.hostPlatform.system}.default; };
    settings = lib.mkOption { type = toml.type; default = { }; description = "Initial supported schema: theme, reduced_motion, dock, panel."; };
    configurationMode = lib.mkOption { type = lib.types.enum [ "strict" "defaults" ]; default = "strict"; };
    enableLabwcBindings = lib.mkOption { type = lib.types.bool; default = false; description = "Install example bindings, without replacing labwc rc.xml."; };
    notifications.enable = lib.mkOption { type = lib.types.bool; default = false; description = "Opt into the experimental notification server; disable other servers first."; };
  };
  config = lib.mkIf cfg.enable {
    home.packages = [ cfg.package pkgs.labwc ];
    xdg.configFile."mosaic-shell/config.toml".source = toml.generate "mosaic-shell-config" settings;
    xdg.configFile."mosaic-shell/labwc-keybindings.xml" = lib.mkIf cfg.enableLabwcBindings {
      text = ''
        <!-- Copy inside your existing <keyboard>; this file is not loaded automatically. -->
        <keybind key="W-space"><action name="Execute" command="${cfg.package}/bin/shellctl launcher" /></keybind>
        <keybind key="W-a"><action name="Execute" command="${cfg.package}/bin/shellctl applications" /></keybind>
        <keybind key="W-comma"><action name="Execute" command="${cfg.package}/bin/shellctl settings" /></keybind>
        <keybind key="W-Escape"><action name="Execute" command="${cfg.package}/bin/shellctl dock show" /></keybind>
        <keybind key="W-n"><action name="Execute" command="${cfg.package}/bin/shellctl notifications" /></keybind>
      '';
    };
    systemd.user.targets.mosaic-session.Unit = { Description = "Mosaic Labwc session"; StopWhenUnneeded = false; };
    systemd.user.services.mosaic-shell = {
      Unit = { Description = "Mosaic desktop UI"; PartOf = [ "mosaic-session.target" ]; After = [ "mosaic-services.service" ]; StartLimitBurst = 3; StartLimitIntervalSec = 30; };
      Service = { ExecStart = "${cfg.package}/bin/mosaic-shell"; Restart = "on-failure"; RestartSec = 1; };
      Install.WantedBy = [ "mosaic-session.target" ];
    };
    systemd.user.services.mosaic-services = lib.mkIf cfg.notifications.enable {
      Unit = { Description = "Mosaic experimental notification service"; PartOf = [ "mosaic-session.target" ]; StartLimitBurst = 3; StartLimitIntervalSec = 30; };
      Service = { ExecStart = "${cfg.package}/bin/mosaic-services"; Restart = "on-failure"; RestartSec = 1; };
      Install.WantedBy = [ "mosaic-session.target" ];
    };
    # Starts the target only after Labwc sets WAYLAND_DISPLAY. No automatic start outside a session.
    xdg.configFile."mosaic-shell/start-target" = {
      executable = true;
      text = ''
        #!${pkgs.runtimeShell}
        set -eu
        test -n "''${WAYLAND_DISPLAY:-}"
        export DISPLAY="''${DISPLAY:-}"
        export XDG_CURRENT_DESKTOP="''${XDG_CURRENT_DESKTOP:-labwc}"
        ${pkgs.systemd}/bin/systemctl --user unset-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP
        ${pkgs.systemd}/bin/systemctl --user import-environment WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP
        ${pkgs.dbus}/bin/dbus-update-activation-environment --systemd WAYLAND_DISPLAY DISPLAY XDG_CURRENT_DESKTOP
        ${pkgs.systemd}/bin/systemctl --user start mosaic-session.target
      '';
    };
  };
}
