{
  description = "Mosaic shell: panel, Dock and application launcher for Labwc";
  inputs.nixpkgs.url = "github:NixOS/nixpkgs/f1e5c356f35d07e7b71a6b5e6c9338e0b029e0c0";
  inputs.home-manager.url = "github:nix-community/home-manager/release-26.05";
  inputs.home-manager.inputs.nixpkgs.follows = "nixpkgs";
  outputs = { self, nixpkgs, home-manager }:
    let
      system = "x86_64-linux";
      pkgs = import nixpkgs { inherit system; };
      inherit (pkgs) lib;
      src = lib.cleanSourceWith {
        src = ./.;
        filter = path: type: !(lib.elem (baseNameOf path) [ "target" ".git" "result" "flake.nix" "flake.lock" "docs" "nix" ]);
      };
      tools = pkgs.rustPlatform.buildRustPackage {
        pname = "mosaic-shell-tools";
        version = "0.4.10";
        # Python integration scripts are check inputs, not Rust compilation inputs.
        src = lib.cleanSourceWith {
          inherit src;
          filter = path: type: !(lib.hasInfix "/tests/platform" path);
        };
        cargoLock.lockFile = ./Cargo.lock;
        nativeBuildInputs = [ pkgs.rustfmt pkgs.clippy pkgs.pkg-config ];
        buildInputs = [ pkgs.gtk4 pkgs.gtk4-layer-shell ];
        MOSAIC_PACTL = "${pkgs.pulseaudio}/bin/pactl";
        MOSAIC_SETPRIV = "${pkgs.util-linux}/bin/setpriv";
        MOSAIC_GTK_ICON_DIR = "${pkgs.adwaita-icon-theme}/share/icons";
        MOSAIC_GTK_SCHEMA_DIR = "${pkgs.gtk4}/share/gsettings-schemas/${pkgs.gtk4.name}/glib-2.0/schemas";
        MOSAIC_GRIM = "${pkgs.grim}/bin/grim";
        buildFeatures = [ "desktop" ];
        checkFeatures = [ "desktop" ];
        postBuild = ''
          cargo build --offline --features desktop --example tray-fixture --example desktop-drop-fixture --example wifi-fixture --example bluetooth-fixture --example brightness-fixture --example power-fixture --release --target ${pkgs.stdenv.hostPlatform.rust.rustcTarget}
        '';
        postInstall = ''
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/tray-fixture $out/libexec/tray-fixture
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/desktop-drop-fixture $out/libexec/desktop-drop-fixture
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/wifi-fixture $out/libexec/wifi-fixture
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/bluetooth-fixture $out/libexec/bluetooth-fixture
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/brightness-fixture $out/libexec/brightness-fixture
          install -Dm755 target/${pkgs.stdenv.hostPlatform.rust.rustcTarget}/release/examples/power-fixture $out/libexec/power-fixture
          install -m755 scripts/mosaic-session $out/bin/mosaic-session
          substituteInPlace $out/bin/mosaic-session \
            --replace-fail 'setsid=setsid' 'setsid=${pkgs.util-linux}/bin/setsid'
          install -Dm755 scripts/mosaic-alt-tab $out/bin/mosaic-alt-tab
          substituteInPlace $out/bin/mosaic-alt-tab \
            --replace-fail '/usr/bin/env python3' '${pkgs.python3}/bin/python3'
          install -Dm644 assets/org.mosaic.Files.desktop $out/share/applications/org.mosaic.Files.desktop
          substituteInPlace $out/share/applications/org.mosaic.Files.desktop \
            --replace-fail 'Exec=mosaic-files' "Exec=$out/bin/mosaic-files"
          test -x $out/bin/mosaic-shell
          test -x $out/bin/mosaic-services
          test -x $out/bin/shellctl
          test -x $out/bin/mosaic-files
        '';
        postCheck = ''
          cargo fmt --check
          cargo clippy --offline --all-targets --features desktop -- -D warnings
        '';
        meta.platforms = [ system ];
      };
      layerProbe = pkgs.stdenv.mkDerivation {
        pname = "mosaic-layer-smoke";
        version = "0.0.1";
        inherit src;
        nativeBuildInputs = [ pkgs.rustc ];
        buildInputs = [ pkgs.gtk4 pkgs.gtk4-layer-shell pkgs.glib ];
        buildPhase = ''
          rustc --edition=2024 -O probes/layer-smoke.rs -o layer-smoke \
            -L native=${pkgs.gtk4}/lib \
            -L native=${pkgs.gtk4-layer-shell}/lib \
            -L native=${pkgs.glib}/lib \
            -C link-arg=-Wl,-rpath,${lib.makeLibraryPath [ pkgs.gtk4 pkgs.gtk4-layer-shell pkgs.glib ]}
        '';
        installPhase = ''
          mkdir -p $out/bin
          cp layer-smoke $out/bin/
        '';
        meta.platforms = [ system ];
      };
      headlessLabwc = pkgs.labwc.overrideAttrs (_: {
        mesonFlags = [ "-Dxwayland=disabled" ];
      });
      scaleCheck = pkgs.runCommand "mosaic-scale-smoke" {
        nativeBuildInputs = [ pkgs.python3 ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/scales.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --randr ${pkgs.wlr-randr}/bin/wlr-randr \
          --probe ${layerProbe}/bin/layer-smoke \
          --report $out/scales.json
      '';
      verticalCheck = pkgs.runCommand "mosaic-vertical-smoke" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell \
          --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session \
          --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke \
          --grim ${pkgs.grim}/bin/grim \
          --screenshot $out/preview.png \
          --launcher-check \
          --report $out/vertical.json
      '';
      menuChecks = lib.genAttrs [ "100" "125" "150" "200" ] (scale:
        pkgs.runCommand "mosaic-menu-${scale}" {
          nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
          FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        } ''
          mkdir -p $out
          python3 ${src}/tests/platform/vertical.py \
            --labwc ${headlessLabwc}/bin/labwc \
            --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
            --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
            --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
            --dbusmenu --tray-fixture ${tools}/libexec/tray-fixture \
            --randr ${pkgs.wlr-randr}/bin/wlr-randr --wtype ${pkgs.wtype}/bin/wtype \
            --scale ${ { "100" = "1"; "125" = "1.25"; "150" = "1.5"; "200" = "2"; }.${scale}} \
            --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
        '');
      overflowCheck = pkgs.runCommand "mosaic-panel-overflow" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --overflow --randr ${pkgs.wlr-randr}/bin/wlr-randr --tray-fixture ${tools}/libexec/tray-fixture --wtype ${pkgs.wtype}/bin/wtype --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      calendarCheck = pkgs.runCommand "mosaic-panel-calendar" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --calendar --wtype ${pkgs.wtype}/bin/wtype --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      controlCenterCheck = pkgs.runCommand "mosaic-control-center" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --control-center --wtype ${pkgs.wtype}/bin/wtype --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      audioCheck = pkgs.runCommand "mosaic-audio-and-interrupt" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --wtype ${pkgs.wtype}/bin/wtype \
          --pulseaudio ${pkgs.pulseaudio}/bin/pulseaudio --pactl ${pkgs.pulseaudio}/bin/pactl \
          --pacat ${pkgs.pulseaudio}/bin/pacat \
          --interrupt-session --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      systemCheck = kind: pkgs.runCommand "mosaic-${kind}" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --wtype ${pkgs.wtype}/bin/wtype \
          --${kind}-fixture ${tools}/libexec/${kind}-fixture --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      wifiCheck = systemCheck "wifi";
      bluetoothCheck = systemCheck "bluetooth";
      brightnessCheck = systemCheck "brightness";
      powerCheck = systemCheck "power";
      altTabCheck = pkgs.runCommand "mosaic-alt-tab" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/test_alt_tab_config.py
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --wtype ${pkgs.wtype}/bin/wtype \
          --alt-tab ${tools}/bin/mosaic-alt-tab --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      filesCheck = pkgs.runCommand "mosaic-files" {
        nativeBuildInputs = [
          (pkgs.python3.withPackages (ps: [ ps.pygobject3 ]))
          pkgs.gobject-introspection pkgs.dbus pkgs.systemd
        ];
        buildInputs = [ pkgs.gtk4 ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --ctl ${tools}/bin/shellctl --session ${tools}/bin/mosaic-session \
          --probe ${layerProbe}/bin/layer-smoke --wtype ${pkgs.wtype}/bin/wtype \
          --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --files ${tools}/bin/mosaic-files --mime-data ${pkgs.shared-mime-info}/share \
          --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      cardsCheck = pkgs.runCommand "mosaic-dock-cards" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --cards --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
      '';
      edgeCheck = pkgs.runCommand "mosaic-dock-edges" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        for scale in 1 1.5; do
          mkdir -p "$out/$scale"
          python3 ${src}/tests/platform/vertical.py \
            --labwc ${headlessLabwc}/bin/labwc \
            --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
            --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
            --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
            --dock-edges --grim ${pkgs.grim}/bin/grim --randr ${pkgs.wlr-randr}/bin/wlr-randr \
            --scale "$scale" --report "$out/$scale/vertical.json"
        done
      '';
      dropCheck = pkgs.runCommand "mosaic-desktop-drop" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --drop-fixture ${tools}/libexec/desktop-drop-fixture --report $out/vertical.json
      '';
      trayRegistry = pkgs.runCommand "mosaic-tray-registry" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/tray_registry.py \
          --services ${tools}/bin/mosaic-services --ctl ${tools}/bin/shellctl \
          --fixture ${tools}/libexec/tray-fixture --report $out/registry.json
      '';
      trayChecks = lib.genAttrs [ "owned" "external" ] (mode:
        pkgs.runCommand "mosaic-tray-${mode}" {
          nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
          FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        } ''
          mkdir -p $out
          python3 ${src}/tests/platform/vertical.py \
            --labwc ${headlessLabwc}/bin/labwc \
            --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
            --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
            --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
            --tray --tray-fixture ${tools}/libexec/tray-fixture \
            ${lib.optionalString (mode == "external") "--tray-external"} \
            --grim ${pkgs.grim}/bin/grim --report $out/vertical.json
        '');
      monitorCheck = pkgs.runCommand "mosaic-monitors" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --randr ${pkgs.wlr-randr}/bin/wlr-randr \
          --pointer ${pointerFixture}/bin/mosaic-pointer-fixture --wtype ${pkgs.wtype}/bin/wtype \
          --grim ${pkgs.grim}/bin/grim --monitors --report $out/vertical.json
      '';
      trashCheck = pkgs.runCommand "mosaic-dock-trash" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        GIO_EXTRA_MODULES = "${pkgs.gvfs}/lib/gio/modules";
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc \
          --shell ${tools}/bin/mosaic-shell --services ${tools}/bin/mosaic-services \
          --session ${tools}/bin/mosaic-session --ctl ${tools}/bin/shellctl \
          --probe ${layerProbe}/bin/layer-smoke --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
          --wtype ${pkgs.wtype}/bin/wtype --gvfsd ${pkgs.gvfs}/libexec/gvfsd \
          --gio ${pkgs.glib.bin}/bin/gio --confirm-trash-delete --report $out/vertical.json
      '';
      pointerFixture = pkgs.rustPlatform.buildRustPackage {
        pname = "mosaic-pointer-fixture";
        version = "0.1.0";
        src = lib.cleanSourceWith {
          src = ./probes/pointer;
          filter = path: type: baseNameOf path != "target";
        };
        cargoLock.lockFile = ./probes/pointer/Cargo.lock;
        doCheck = false; # Exercised by real GTK/Wayland integration checks below.
      };
      dockChecks = lib.genAttrs [ "100" "125" "150" "200" ] (scale:
        pkgs.runCommand "mosaic-dock-${scale}" {
          nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
          FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
        } ''
          mkdir -p $out
          python3 ${src}/tests/platform/vertical.py \
            --labwc ${headlessLabwc}/bin/labwc \
            --shell ${tools}/bin/mosaic-shell \
            --services ${tools}/bin/mosaic-services \
            --session ${tools}/bin/mosaic-session \
            --ctl ${tools}/bin/shellctl \
            --probe ${layerProbe}/bin/layer-smoke \
            --pointer ${pointerFixture}/bin/mosaic-pointer-fixture \
            --randr ${pkgs.wlr-randr}/bin/wlr-randr \
            --scale ${ { "100" = "1"; "125" = "1.25"; "150" = "1.5"; "200" = "2"; }.${scale}} \
            --dock-behavior --grim ${pkgs.grim}/bin/grim \
            ${lib.optionalString (scale == "100") "--swaybg ${pkgs.swaybg}/bin/swaybg"} \
            --screenshot $out/preview-dark.png --report $out/vertical.json
        '');
      temperaturesCheck = pkgs.runCommand "mosaic-temperatures" {
        nativeBuildInputs = [ pkgs.python3 pkgs.dbus pkgs.systemd ];
        FONTCONFIG_FILE = pkgs.makeFontsConf { fontDirectories = [ pkgs.dejavu_fonts ]; };
      } ''
        mkdir -p $out
        python3 ${src}/tests/platform/vertical.py \
          --labwc ${headlessLabwc}/bin/labwc --shell ${tools}/bin/mosaic-shell \
          --services ${tools}/bin/mosaic-services --session ${tools}/bin/mosaic-session \
          --ctl ${tools}/bin/shellctl --probe ${layerProbe}/bin/layer-smoke \
          --pointer ${pointerFixture}/bin/mosaic-pointer-fixture --grim ${pkgs.grim}/bin/grim \
          --temperatures --report $out/vertical.json
      '';
      homeConfig = home-manager.lib.homeManagerConfiguration {
        inherit pkgs;
        modules = [ self.homeManagerModules.default {
          home.username = "mosaic-fixture";
          home.homeDirectory = "/home/mosaic-fixture";
          home.stateVersion = "26.05";
          programs.mosaic-shell.enable = true;
          programs.mosaic-shell.settings.pins = [ "firefox.desktop" ];
        } ];
      };
    in {
      packages.${system} = { default = tools; shellctl = tools; layer-smoke = layerProbe; labwc-headless = headlessLabwc; };
      checks.${system} = { inherit tools; layer-build = layerProbe; scales = scaleCheck; vertical = verticalCheck; monitors = monitorCheck; dock-cards = cardsCheck; dock-edges = edgeCheck; desktop-drop = dropCheck; dock-trash = trashCheck; panel-calendar = calendarCheck; control-center = controlCenterCheck; audio = audioCheck; wifi = wifiCheck; bluetooth = bluetoothCheck; brightness = brightnessCheck; power = powerCheck; temperatures = temperaturesCheck; alt-tab = altTabCheck; files = filesCheck; panel-overflow = overflowCheck; tray-registry = trayRegistry; home-manager = homeConfig.activationPackage; }
        // lib.mapAttrs' (scale: check: lib.nameValuePair "dock-${scale}" check) dockChecks
        // lib.mapAttrs' (mode: check: lib.nameValuePair "tray-${mode}" check) trayChecks
        // lib.mapAttrs' (scale: check: lib.nameValuePair "menu-${scale}" check) menuChecks;
      homeManagerModules.default = { config, lib, pkgs, ... }: import ./nix/home-manager.nix { inherit config lib pkgs; defaultPackage = tools; };
      devShells.${system}.default = pkgs.mkShell {
        packages = [ pkgs.rustc pkgs.cargo pkgs.rustfmt pkgs.clippy pkgs.pkg-config pkgs.wayland-utils ];
        buildInputs = [ pkgs.gtk4 pkgs.gtk4-layer-shell pkgs.glib ];
      };
    };
}
