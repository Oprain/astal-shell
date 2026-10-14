use kdl::{KdlDocument, KdlNode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    env, fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(untagged)]
pub enum Command {
    Argv(Vec<String>),
    Text(String),
}

impl Command {
    pub fn argv(&self) -> Result<Vec<String>, String> {
        let argv = match self {
            Self::Argv(args) => args.clone(),
            Self::Text(text) => shell_words::split(text).map_err(|e| e.to_string())?,
        };
        if argv.first().is_none_or(|v| v.is_empty()) {
            return Err("command must contain an executable".into());
        }
        Ok(argv)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
pub struct QuickSetting {
    pub id: String,
    pub icon: String,
    pub label: String,
    pub command: Command,
    #[serde(default)]
    pub confirm: bool,
}

pub fn default_quick_settings() -> Vec<QuickSetting> {
    [
        (
            "shutdown",
            "system-shutdown-symbolic",
            "Shutdown",
            vec!["systemctl", "poweroff"],
        ),
        (
            "reboot",
            "system-reboot-symbolic",
            "Reboot",
            vec!["systemctl", "reboot"],
        ),
        (
            "logout",
            "system-log-out-symbolic",
            "Logout",
            vec!["niri", "msg", "action", "quit", "-s"],
        ),
    ]
    .into_iter()
    .map(|(id, icon, label, argv)| QuickSetting {
        id: id.into(),
        icon: icon.into(),
        label: label.into(),
        command: Command::Argv(argv.into_iter().map(String::from).collect()),
        confirm: true,
    })
    .collect()
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct Settings {
    pub niri_config: Option<PathBuf>,
    pub overview_zoom: Option<f64>,
    pub clearance: i32,
    pub reveal_delay_ms: u64,
    pub weather: WeatherSettings,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            niri_config: None,
            overview_zoom: None,
            clearance: 3,
            reveal_delay_ms: 300,
            weather: WeatherSettings::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(default)]
pub struct WeatherSettings {
    pub enable: bool,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            enable: true,
            latitude: None,
            longitude: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub settings: Settings,
    pub theme: Value,
    pub quick_settings: Vec<QuickSetting>,
    pub overview_zoom: f64,
}

pub fn config_home() -> PathBuf {
    env::var_os("XDG_CONFIG_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(".config"))
}

fn read_optional<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, String> {
    match fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map(Some)
            .map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

impl Config {
    pub fn load() -> Result<Self, String> {
        Self::from_directory(&config_home().join("astal-shell"), None)
    }

    pub fn from_directory(directory: &Path, niri_path: Option<&Path>) -> Result<Self, String> {
        let settings: Settings = read_optional(&directory.join("config.json"))?.unwrap_or_default();
        if !(0..=64).contains(&settings.clearance) || settings.reveal_delay_ms > 10_000 {
            return Err("clearance must be 0..64 and revealDelayMs must be 0..10000".into());
        }
        if let Some(z) = settings.overview_zoom
            && (!z.is_finite() || z <= 0. || z > 1.)
        {
            return Err("overviewZoom must be in (0, 1]".into());
        }
        let weather = &settings.weather;
        if weather.latitude.is_some() != weather.longitude.is_some()
            || weather
                .latitude
                .is_some_and(|v| !v.is_finite() || !(-90. ..=90.).contains(&v))
            || weather
                .longitude
                .is_some_and(|v| !v.is_finite() || !(-180. ..=180.).contains(&v))
        {
            return Err("weather coordinates must be a valid latitude/longitude pair".into());
        }
        let theme: Value =
            read_optional(&directory.join("theme.json"))?.unwrap_or_else(|| json!({}));
        if !theme.is_object() {
            return Err("theme.json must contain an object".into());
        }
        for key in ["high", "medium", "low"] {
            if let Some(value) = theme.get("opacity").and_then(|v| v.get(key))
                && value.as_f64().is_none_or(|v| !(0. ..=1.).contains(&v))
            {
                return Err(format!("theme opacity.{key} must be a number in [0, 1]"));
            }
        }
        let quick_settings: Vec<QuickSetting> =
            read_optional(&directory.join("quickSettings.json"))?
                .unwrap_or_else(default_quick_settings);
        let mut ids = BTreeSet::new();
        for entry in &quick_settings {
            entry.command.argv()?;
            if entry.id.is_empty() || !ids.insert(&entry.id) {
                return Err("quick setting IDs must be nonempty and unique".into());
            }
        }
        let path = niri_path
            .map(PathBuf::from)
            .or_else(|| settings.niri_config.clone())
            .or_else(|| env::var_os("NIRI_CONFIG").map(PathBuf::from))
            .unwrap_or_else(|| config_home().join("niri/config.kdl"));
        let zoom = match settings.overview_zoom {
            Some(zoom) => zoom,
            None => read_overview_zoom(&path)?,
        };
        Ok(Self {
            settings,
            theme,
            quick_settings,
            overview_zoom: zoom.clamp(0.0001, 0.75),
        })
    }

    pub fn icon_theme(&self) -> Option<&str> {
        self.theme["iconTheme"].as_str().filter(|s| !s.is_empty())
    }

    pub fn theme_string(&self, path: &[&str], fallback: &str) -> String {
        path.iter()
            .fold(&self.theme, |value, key| &value[*key])
            .as_str()
            .filter(|v| !v.is_empty())
            .unwrap_or(fallback)
            .into()
    }

    pub fn status_colors(&self) -> [gtk::gdk::RGBA; 3] {
        [
            ("success", "#26a269"),
            ("warning", "#e5a50a"),
            ("error", "#c01c28"),
        ]
        .map(|(key, fallback)| {
            gtk::gdk::RGBA::parse(self.theme_string(&["status", key], fallback))
                .unwrap_or_else(|_| gtk::gdk::RGBA::parse(fallback).unwrap())
        })
    }

    pub fn css(&self) -> String {
        let value = |path: &[&str], fallback: &str| self.theme_string(path, fallback);
        let opacity = |key, fallback| self.theme["opacity"][key].as_f64().unwrap_or(fallback);
        format!(
            "window.astal-shell {{ background: transparent; color: {fg}; font-size: {normal}; font-weight: {weight}; }}
             .panel {{ background: {bg}; border-radius: {medium_radius}; padding: 2px {small_space}; }}
             .panel button {{ min-width: 24px; min-height: 24px; padding: 2px; background: transparent; border: none; box-shadow: none; }}
             .panel button:hover {{ background: {secondary_bg}; }}
             .panel .quick-action {{ border-radius: {large_radius}; }}
             .workspace {{ min-width: 8px; min-height: 24px; padding: 0; border-radius: {small_radius}; background: {secondary_accent}; border: none; box-shadow: none; outline: none; }}
             .workspace.active {{ background: {accent}; }}
             .workspace.urgent {{ background: {warning}; }}
             .dock button {{ padding: {small_space}; border-radius: {medium_radius}; opacity: {low}; color: {unfocused}; }}
             .dock button.focused {{ opacity: {high}; color: {focused}; background: {secondary_bg}; }}
             .clock, .weather, .network {{ font-size: {small}; font-weight: {weight}; opacity: {medium}; }}
             .meter-label {{ font-size: 0.5em; font-weight: {bold}; }}
             .media-artist {{ font-weight: {bold}; }}
             .media-title, .muted {{ font-size: {small}; color: {secondary_fg}; opacity: {medium}; }}
             .media-text {{ margin-left: {small_space}; margin-right: {small_space}; }}
             .cover-art {{ border-radius: {medium_radius}; border: 2px solid {border}; }}
             .panel .cover-control {{ padding: 0; border-radius: {large_radius}; background: {overlay}; }}
             .osd {{ padding: {medium_space}; font-size: {large}; border-radius: {large_radius}; }}
             .confirm {{ padding: {large_space}; }}
             .confirm-title {{ font-size: {large}; font-weight: {bold}; }}
             .confirm button {{ margin-top: {large_space}; padding: {medium_space} {large_space}; font-weight: {bold}; }}
             .danger {{ background: {error}; color: {fg}; }}
             progressbar trough {{ min-width: 6px; min-height: 4px; }}
             progressbar progress {{ background: {success}; }}",
            fg = value(&["text", "primary"], "@theme_fg_color"),
            bg = value(&["background", "primary"], "alpha(@theme_bg_color, 0.90)"),
            secondary_bg = value(&["background", "secondary"], "alpha(@theme_fg_color, 0.12)"),
            secondary_fg = value(&["text", "secondary"], "alpha(@theme_fg_color, 0.7)"),
            focused = value(&["text", "focused"], "@theme_fg_color"),
            unfocused = value(&["text", "unfocused"], "alpha(@theme_fg_color, 0.7)"),
            accent = value(&["accent", "primary"], "@theme_selected_bg_color"),
            secondary_accent = value(&["accent", "secondary"], "alpha(@theme_fg_color, 0.35)"),
            border = value(&["accent", "border"], "alpha(@theme_fg_color, 0.2)"),
            overlay = value(&["accent", "overlay"], "rgba(0, 0, 0, 0.6)"),
            success = value(&["status", "success"], "#26a269"),
            warning = value(&["status", "warning"], "#e5a50a"),
            error = value(&["status", "error"], "#c01c28"),
            small = value(&["font", "size", "small"], "0.8em"),
            normal = value(&["font", "size", "normal"], "1em"),
            large = value(&["font", "size", "large"], "1.2em"),
            weight = value(&["font", "weight", "normal"], "normal"),
            bold = value(&["font", "weight", "bold"], "bold"),
            small_space = value(&["spacing", "small"], "4px"),
            medium_space = value(&["spacing", "medium"], "8px"),
            large_space = value(&["spacing", "large"], "16px"),
            small_radius = value(&["borderRadius", "small"], "2px"),
            medium_radius = value(&["borderRadius", "medium"], "4px"),
            large_radius = value(&["borderRadius", "large"], "9999px"),
            high = opacity("high", 1.),
            medium = opacity("medium", 0.8),
            low = opacity("low", 0.6),
        )
    }
}

pub fn read_overview_zoom(path: &Path) -> Result<f64, String> {
    let mut stack = BTreeSet::new();
    let mut zoom = 0.5;
    if path.exists() {
        visit_config(path, &mut stack, &mut zoom)?;
    }
    Ok(zoom)
}

fn number(node: &KdlNode) -> Option<f64> {
    node.get(0)
        .and_then(|v| v.as_float().or_else(|| v.as_integer().map(|n| n as f64)))
}

fn visit_config(path: &Path, stack: &mut BTreeSet<PathBuf>, zoom: &mut f64) -> Result<(), String> {
    // Canonicalize only for cycle detection. Includes remain relative to the
    // config's lexical location, including Home Manager's store symlinks.
    let canonical = fs::canonicalize(path).map_err(|e| format!("{}: {e}", path.display()))?;
    if stack.len() >= 10 || !stack.insert(canonical.clone()) {
        return Err(format!("recursive Niri include: {}", path.display()));
    }
    let text = fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let doc: KdlDocument = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for node in doc.nodes() {
        match node.name().value() {
            "overview" => {
                if let Some(node) = node.children().and_then(|d| d.get("zoom")) {
                    let value = number(node).ok_or("overview zoom must be a number")?;
                    if !value.is_finite() || value <= 0. {
                        return Err("overview zoom must be positive and finite".into());
                    }
                    *zoom = value;
                }
            }
            "include" => {
                if let Some(name) = node.get(0).and_then(|v| v.as_string()) {
                    let include = if let Some(rest) = name.strip_prefix("~/") {
                        PathBuf::from(env::var_os("HOME").unwrap_or_default()).join(rest)
                    } else {
                        path.parent().unwrap_or(Path::new(".")).join(name)
                    };
                    let optional = node
                        .get("optional")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if include.exists() || !optional {
                        visit_config(&include, stack, zoom)?;
                    }
                }
            }
            _ => {}
        }
    }
    stack.remove(&canonical);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture(name: &str) -> PathBuf {
        let p = env::temp_dir().join(format!("astal-shell-{}-{name}", std::process::id()));
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn parses_includes_ignores_comments_and_reloads_changes() {
        let p = fixture("includes");
        fs::write(
            p.join("config.kdl"),
            "// overview { zoom 0.1; }\ninclude \"overview.kdl\"\n",
        )
        .unwrap();
        fs::write(p.join("overview.kdl"), "overview {\n zoom 0.6\n}\n").unwrap();
        assert_eq!(read_overview_zoom(&p.join("config.kdl")).unwrap(), 0.6);
        fs::write(p.join("overview.kdl"), "overview {\n zoom 0.4\n}\n").unwrap();
        assert_eq!(read_overview_zoom(&p.join("config.kdl")).unwrap(), 0.4);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn nix_store_symlinks_keep_relative_includes_at_the_config_location() {
        let p = fixture("symlink");
        fs::create_dir_all(p.join("store")).unwrap();
        fs::create_dir_all(p.join("home")).unwrap();
        fs::write(p.join("store/config.kdl"), "include \"overview.kdl\"\n").unwrap();
        fs::write(p.join("home/overview.kdl"), "overview {\n zoom 0.6\n}\n").unwrap();
        std::os::unix::fs::symlink(p.join("store/config.kdl"), p.join("home/config.kdl")).unwrap();
        assert_eq!(read_overview_zoom(&p.join("home/config.kdl")).unwrap(), 0.6);
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn refuses_recursive_includes_and_invalid_zoom() {
        let p = fixture("cycle");
        fs::write(p.join("config.kdl"), "include \"config.kdl\"\n").unwrap();
        assert!(read_overview_zoom(&p.join("config.kdl")).is_err());
        fs::write(p.join("config.kdl"), "overview {\n zoom -0.5\n}\n").unwrap();
        assert!(read_overview_zoom(&p.join("config.kdl")).is_err());
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn inherits_icon_theme_and_preserves_explicit_empty_quick_settings() {
        let p = fixture("json");
        fs::write(p.join("quickSettings.json"), "[]").unwrap();
        let config = Config::from_directory(&p, Some(&p.join("absent.kdl"))).unwrap();
        assert_eq!(config.icon_theme(), None);
        assert!(config.quick_settings.is_empty());
        assert!(!p.join("theme.json").exists());
        fs::remove_dir_all(p).unwrap();
    }

    #[test]
    fn command_strings_respect_quoted_arguments_without_running_a_shell() {
        assert_eq!(
            Command::Text("notify-send 'hello world' '$HOME'".into())
                .argv()
                .unwrap(),
            vec!["notify-send", "hello world", "$HOME"]
        );
        assert!(Command::Argv(vec![]).argv().is_err());
    }

    #[test]
    fn legacy_theme_settings_affect_generated_styles() {
        let p = fixture("legacy-theme");
        fs::write(p.join("theme.json"), json!({
            "font": {"size": {"small": "11px", "normal": "14px", "large": "19px"}, "weight": {"bold": "600"}},
            "spacing": {"small": "7px", "medium": "9px", "large": "21px"},
            "borderRadius": {"small": "3px", "medium": "5px", "large": "33px"},
            "opacity": {"high": 0.9, "medium": 0.7, "low": 0.4},
            "text": {"focused": "#123456", "unfocused": "#654321"},
            "accent": {"border": "#112233", "overlay": "#445566"}
        }).to_string()).unwrap();
        let config = Config::from_directory(&p, Some(&p.join("absent.kdl"))).unwrap();
        let css = config.css();
        for expected in [
            "11px",
            "14px",
            "19px",
            "600",
            "7px",
            "9px",
            "21px",
            "3px",
            "5px",
            "33px",
            "opacity: 0.9",
            "opacity: 0.7",
            "opacity: 0.4",
            "#123456",
            "#654321",
            "#112233",
            "#445566",
        ] {
            assert!(css.contains(expected), "{expected}");
        }
        fs::write(p.join("theme.json"), "{\"opacity\":{\"low\":2}}").unwrap();
        assert!(Config::from_directory(&p, Some(&p.join("absent.kdl"))).is_err());
        fs::remove_dir_all(p).unwrap();
    }
}
