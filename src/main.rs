mod artwork;
mod icons;
mod media;
mod meter;
mod tray;
mod ui;

use astal_shell::{config::Config, geometry, ipc, model::Output};
use gtk::prelude::*;
use serde_json::json;
use std::{cell::Cell, collections::BTreeMap, process::ExitCode, rc::Rc};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("--version") => {
            println!("astal-shell {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Some("--help" | "-h") => {
            println!(
                "astal-shell [--check-config | --print-layout | --version]\n\
                Run inside a Niri Wayland session. Configuration: $XDG_CONFIG_HOME/astal-shell/."
            );
            return ExitCode::SUCCESS;
        }
        Some("--check-config") => {
            return match Config::load() {
                Ok(config) => {
                    println!(
                        "{}",
                        json!({
                            "overviewZoom": config.overview_zoom,
                            "iconTheme": config.icon_theme().unwrap_or("(inherited from GTK)"),
                            "quickSettings": config.quick_settings.len(),
                            "revealDelayMs": config.settings.reveal_delay_ms
                        })
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            };
        }
        Some("--print-layout") => {
            return match print_layout() {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            };
        }
        Some(arg) => {
            eprintln!("Unknown argument: {arg}");
            return ExitCode::FAILURE;
        }
        None => {}
    }
    if std::env::var_os("NIRI_SOCKET").is_none() {
        eprintln!("NIRI_SOCKET is not set. Start astal-shell inside your Niri session.");
        return ExitCode::FAILURE;
    }
    let app = gtk::Application::builder()
        .application_id("dev.astal.shell")
        .build();
    let started = Cell::new(false);
    let failed = Rc::new(Cell::new(false));
    let failure = failed.clone();
    app.connect_activate(move |app| {
        if started.replace(true) {
            return;
        }
        if !gtk4_layer_shell::is_supported() {
            eprintln!("The Wayland compositor does not support wlr-layer-shell.");
            failure.set(true);
            app.quit();
            return;
        }
        if !ui::start(app) {
            failure.set(true);
        }
    });
    let exit = app.run_with_args::<&str>(&[]);
    if exit == gtk::glib::ExitCode::SUCCESS && !failed.get() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn print_layout() -> Result<(), String> {
    let config = Config::load()?;
    let response = ipc::request(&json!("Outputs")).map_err(|e| e.to_string())?;
    let outputs: BTreeMap<String, Output> =
        serde_json::from_value(response["Outputs"].clone()).map_err(|e| e.to_string())?;
    let placements: BTreeMap<_, _> = outputs
        .into_iter()
        .filter_map(|(name, output)| {
            let logical = output.logical?;
            Some((
                name,
                geometry::place(
                    geometry::OutputSize {
                        width: logical.width as f64,
                        height: logical.height as f64,
                        scale: logical.scale,
                    },
                    config.overview_zoom,
                    40,
                    48,
                    20,
                    config.settings.clearance,
                ),
            ))
        })
        .collect();
    println!(
        "{}",
        serde_json::to_string_pretty(&placements).map_err(|e| e.to_string())?
    );
    Ok(())
}
