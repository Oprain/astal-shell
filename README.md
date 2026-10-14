# Astal Shell

A native Rust / GTK4 overview shell for Niri. The application no longer uses AGS, GJS, Bun, or Astal libraries. It retains the `astal-shell` executable and `services.astal-shell` Home Manager namespace.

This fork is maintained under [Oprain/astal-shell](https://github.com/Oprain/astal-shell) and derives from [knoopx/astal-shell](https://github.com/knoopx/astal-shell). The original Git history and GPL-3.0 license are retained.

## Run and develop

Run these commands on Linux inside your Niri session:

```bash
nix run path:.
nix develop path:.
cargo run
```

The flake provides reproducible Rust, GTK4, layer-shell, and runtime dependencies for `x86_64-linux` and `aarch64-linux`. `NIRI_SOCKET` must be present when starting the GUI. Commands that do not require a display:

```bash
cargo run -- --check-config
cargo run -- --version
```

`astal-shell --print-layout` prints estimated per-output margins using Niri's actual logical output dimensions; the GUI additionally measures its widgets before placing them.

## Home Manager and Stylix

Add the fork to your flake inputs:

```nix
inputs.astal-shell.url = "github:Oprain/astal-shell";
```

```nix
{ inputs, pkgs, ... }: {
  imports = [ inputs.astal-shell.homeManagerModules.default ];
  services.astal-shell.enable = true;

  # The shell inherits gtk.iconTheme, including Stylix's selected theme.
  # Set an override only if the shell should use a different icon theme:
  # services.astal-shell.theme.iconTheme = "Papirus-Dark";

  # Optional: use known coordinates instead of IP geolocation.
  services.astal-shell.weather = {
    latitude = 31.23;
    longitude = 121.47;
  };
}
```

The module installs the selected icon package and writes the inherited GTK/Stylix icon name into the shell configuration. Explicit `theme.iconTheme` overrides win. Without a module or explicit override, the application inherits GTK's session settings. Desktop application icons use GIO desktop files and `StartupWMClass`; themed names, absolute icon files, and status notifier pixmaps are supported. The package preserves the session's `XDG_DATA_DIRS` and adds Adwaita only as a fallback.

The default package comes from the flake; adding the overlay is optional. `services.astal-shell.package` still supports an explicit package override.

## Workspaces and overview positioning

Each GDK monitor is keyed by its output connector, matching Niri's `Workspace.output` and `Outputs` names. Its left indicators and bottom dock show only that output's workspaces and active workspace windows. Workspace clicks use stable workspace IDs, so duplicate indices on different displays are unambiguous. The active marker slides between buttons over 200 ms and changes direction smoothly during rapid switches. Disabling GTK animations makes the marker update immediately. Active, focused, and urgent workspaces have separate states. Hotplugging a monitor closes its windows and rebuilds them if the connector returns.

Margins follow Niri's overview zoom, workspace gap, output logical dimensions, and fractional pixel rounding. Bars are measured after GTK theme styling. If the gap cannot contain a bar and the requested clearance, the top and bottom bars hide; the left indicators remain when the side margin has room. Optional top-bar content collapses when the card width is too narrow. The dock scrolls rather than exceeding the card width. No per-display margins are needed.

The Niri KDL parser supports v1/v2 syntax, comments, includes, relative include paths, and `~/` paths. Configuration files, including changed include files and Home Manager symlink replacements, are read again every second. Malformed JSON/KDL updates retain the previous valid configuration and log the error. Relative includes keep their original configuration directory when the root file is a Nix store symlink. Niri outputs are refreshed independently; monitor geometry is checked each UI update. During resolution or scale changes, surfaces hide while a fresh output snapshot is requested, then reappear with the new margins.

With a `programs.niri.settings` module available, the Home Manager module adds:

```nix
programs.niri.settings.layer-rules = [
  { matches = [ { namespace = "^astal-shell"; } ]; place-within-backdrop = false; }
];
```

For an unmanaged `config.kdl`, include the generated fragment:

```kdl
include "../astal-shell/niri.kdl"

overview {
    zoom 0.5
}
```

The layer rule keeps these surfaces in output coordinates. Custom rules that subsequently enable `place-within-backdrop` for the shell change that geometry and should be removed or narrowed.

Niri's opening animation progress is not exposed over IPC. The shell waits 300 ms when opening the overview and hides immediately on close. Switching workspaces does not restart this delay, so the bars remain mapped during vertical navigation. An empty workspace clears the dock's icons and background while preserving its surface and height. Increase `niri.revealDelayMs` if your opening animation is longer. The no-overlap calculation applies to settled overview cards; custom animations and dragging between workspaces need visual acceptance testing on the target compositor.

```nix
services.astal-shell.niri = {
  integration = true;
  configPath = null;       # use NIRI_CONFIG, then $XDG_CONFIG_HOME/niri/config.kdl
  overviewZoom = null;     # parse Niri's actual KDL; optional explicit override
  clearance = 3;           # logical pixels
  revealDelayMs = 300;
};
```

An explicit `overviewZoom` is also applied to managed Niri settings, keeping both sides aligned.

## Features and configuration

The top bar provides date/time, weather, MPRIS playback controls, network rates, CPU/RAM/disk meters, optional NVIDIA and battery status, configurable quick actions, and a StatusNotifier tray with D-Bus menus. Media shows separate artist/title lines and a cover with a play/pause control revealed on hover or keyboard focus. Local files and HTTP/HTTPS covers are supported; remote images download in the background to `$XDG_CACHE_HOME/astal-shell/artwork`, with image validation, a size limit and retry delay.

Battery tooltips include charging/discharging time estimates, health, energy, power, voltage, temperature and device details when available. Multiple batteries use energy-weighted aggregation, with charge-based sysfs data supported. Nighttime weather shows the approximate lunar phase and retains the weather description. The bottom dock supports focus/center, double-click maximize, right-click or long-press close, and wheel focus cycling on that output. Volume and brightness OSDs appear on the focused output, with a resettable timeout and clamped progress; amplified volume retains its percentage label.

Workspace and dock buttons retain their identity when focus, urgency or titles change, so double-click gestures survive a focus refresh. Wheel cycling follows the output's active window even when overview has no ordinary window keyboard focus. IPC disconnection hides the bars and active OSDs; reconnection restores the current state.

Configuration is read from `$XDG_CONFIG_HOME/astal-shell`, falling back to `~/.config/astal-shell`. Files are never created or overwritten by the application, so Nix store symlinks remain read-only:

- `theme.json`: existing colors, font sizes/weights, spacing, opacity, border radii, focused/unfocused dock colors and optional `iconTheme`.
- `quickSettings.json`: quick action entries; `[]` disables all actions.
- `config.json`: Niri layout and weather settings.

```nix
services.astal-shell.quickSettings = [
  {
    id = "lock";
    icon = "system-lock-screen-symbolic";
    label = "Lock";
    command = [ "swaylock" "-c" "000000" ];
    confirm = false;
  }
];
```

Command arrays are recommended. Command strings respect shell quoting but do not perform expansion or run through a shell. To intentionally use shell syntax, pass `[ "bash" "-lc" "your command" ]`. Confirmed actions run only after the dialog's Confirm button is pressed.

Weather uses HTTPS IP geolocation and Open-Meteo unless coordinates are configured. Disable it with `services.astal-shell.weather.enable = false`. Launching calendar/weather/system-monitor apps requires those apps in the user's environment. NVIDIA statistics use the system driver's `nvidia-smi`; unsupported hardware is omitted. Volume is sampled using `wpctl`, and brightness is read from Linux backlight sysfs.

## Verify

```bash
nix develop path:. -c cargo fmt --check
nix develop path:. -c cargo clippy --all-targets --locked -- -D warnings
nix develop path:. -c cargo test --locked
nix flake check path:.
```

CI runs formatting, Clippy, source unit tests and the Nix package build on
x86_64 and ARM64. Unit tests cover configuration, monitor-scoped state,
overview geometry, Niri IPC, system counters, batteries, weather and media
control permissions.

Check physical display hotplug, hardware controls and overview gestures in
your target Niri session. `ASTAL_SHELL_DEBUG_LAYOUT=1` enables layout and OSD
diagnostics.
