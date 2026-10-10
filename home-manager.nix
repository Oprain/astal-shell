self: { pkgs, lib, config, options, ... }: let
  cfg = config.services.astal-shell;
  gtkIcon = lib.attrByPath [ "gtk" "iconTheme" "name" ] null config;
  icons = lib.attrByPath [ "stylix" "icons" ] {} config;
  polarity = lib.attrByPath [ "stylix" "polarity" ] "dark" config;
  stylixIcon = if polarity == "light" then icons.light or null else icons.dark or null;
  inheritedIcon = if gtkIcon != null && gtkIcon != "" then gtkIcon else
    if icons.enable or false then stylixIcon else null;
  iconPackage = lib.attrByPath [ "gtk" "iconTheme" "package" ] null config;
  stylixPackage = icons.package or null;
  hasNiriSettings = options ? programs.niri.settings;
  theme = lib.optionalAttrs (inheritedIcon != null) { iconTheme = inheritedIcon; } // cfg.theme;
in {
  options.services.astal-shell = {
    enable = lib.mkEnableOption "the Rust Niri overview shell";
    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.default;
      description = "Native Rust shell package.";
    };
    theme = lib.mkOption {
      type = lib.types.attrsOf lib.types.anything;
      default = {};
      description = "Theme overrides. iconTheme inherits GTK/Stylix unless explicitly set.";
    };
    quickSettings = lib.mkOption {
      type = lib.types.listOf (lib.types.submodule {
        options = {
          id = lib.mkOption { type = lib.types.str; description = "Unique button ID."; };
          icon = lib.mkOption { type = lib.types.str; description = "Icon theme icon name."; };
          label = lib.mkOption { type = lib.types.str; description = "Tooltip text."; };
          command = lib.mkOption {
            type = lib.types.either lib.types.str (lib.types.listOf lib.types.str);
            description = "Executable and arguments. Strings use shell quoting without shell expansion.";
          };
          confirm = lib.mkOption { type = lib.types.bool; default = false; description = "Require confirmation."; };
        };
      });
      default = [
        { id = "shutdown"; icon = "system-shutdown-symbolic"; label = "Shutdown"; command = [ "systemctl" "poweroff" ]; confirm = true; }
        { id = "reboot"; icon = "system-reboot-symbolic"; label = "Reboot"; command = [ "systemctl" "reboot" ]; confirm = true; }
        { id = "logout"; icon = "system-log-out-symbolic"; label = "Logout"; command = [ "niri" "msg" "action" "quit" "-s" ]; confirm = true; }
      ];
      description = "Quick settings. An empty list disables all buttons.";
    };
    niri = {
      integration = lib.mkOption {
        type = lib.types.bool;
        default = true;
        description = "Add output-space layer rules when programs.niri.settings is available.";
      };
      configPath = lib.mkOption {
        type = lib.types.nullOr lib.types.str;
        default = null;
        description = "Niri KDL path. Otherwise use NIRI_CONFIG or XDG_CONFIG_HOME/niri/config.kdl.";
      };
      overviewZoom = lib.mkOption {
        type = lib.types.nullOr lib.types.float;
        default = null;
        description = "Explicit zoom, also applied to managed Niri settings. Otherwise read live KDL and includes.";
      };
      clearance = lib.mkOption {
        type = lib.types.ints.between 0 64;
        default = 3;
        description = "Minimum logical pixel clearance around overview cards.";
      };
      revealDelayMs = lib.mkOption {
        type = lib.types.ints.between 0 10000;
        default = 300;
        description = "Wait for Niri's overview opening animation before revealing the shell.";
      };
    };
    weather = {
      enable = lib.mkEnableOption "weather" // { default = true; };
      latitude = lib.mkOption { type = lib.types.nullOr lib.types.float; default = null; description = "Latitude, or IP geolocation when unset."; };
      longitude = lib.mkOption { type = lib.types.nullOr lib.types.float; default = null; description = "Longitude, or IP geolocation when unset."; };
    };
  };

  config = lib.mkIf cfg.enable (lib.mkMerge [
    {
      assertions = [
        { assertion = (cfg.weather.latitude == null) == (cfg.weather.longitude == null); message = "astal-shell weather latitude and longitude must be set together."; }
        { assertion = cfg.niri.overviewZoom == null || (cfg.niri.overviewZoom > 0 && cfg.niri.overviewZoom <= 1); message = "astal-shell overviewZoom must be in (0, 1]."; }
      ];
      home.packages = [ cfg.package ] ++ lib.filter (p: p != null) [ iconPackage stylixPackage ];
      xdg.configFile = {
        "astal-shell/theme.json".text = builtins.toJSON theme;
        "astal-shell/quickSettings.json".text = builtins.toJSON cfg.quickSettings;
        "astal-shell/config.json".text = builtins.toJSON {
          niriConfig = cfg.niri.configPath;
          overviewZoom = cfg.niri.overviewZoom;
          clearance = cfg.niri.clearance;
          revealDelayMs = cfg.niri.revealDelayMs;
          weather = cfg.weather;
        };
        "astal-shell/niri.kdl".text = ''
          layer-rule {
            match namespace="^astal-shell"
            place-within-backdrop false
          }
        '';
      };
      systemd.user.services.astal-shell = {
        Unit = {
          Description = "Astal Shell (Rust)";
          After = [ "graphical-session.target" ];
          PartOf = [ "graphical-session.target" ];
        };
        Service = {
          ExecStart = "${cfg.package}/bin/astal-shell";
          Restart = "on-failure";
          RestartSec = 3;
        };
        Install.WantedBy = [ "graphical-session.target" ];
      };
    }
    (lib.optionalAttrs hasNiriSettings (lib.mkIf cfg.niri.integration {
      programs.niri.settings = {
        layer-rules = [
          { matches = [ { namespace = "^astal-shell"; } ]; place-within-backdrop = false; }
        ];
      } // lib.optionalAttrs (cfg.niri.overviewZoom != null) {
        overview.zoom = cfg.niri.overviewZoom;
      };
    }))
  ]);
}
