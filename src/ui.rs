use crate::{icons::AppIcons, media, meter::Meter, tray::Tray};
use astal_shell::{
    config::{Config, QuickSetting},
    geometry::{self, OutputSize},
    ipc::{self, Message},
    model::{LogicalOutput, State},
    system::{self, Metrics, Update},
    weather::WeatherDisplay,
};
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

pub(crate) struct Osd {
    pub(crate) window: gtk::ApplicationWindow,
    image: gtk::Image,
    pub(crate) progress: gtk::ProgressBar,
    pub(crate) label: gtk::Label,
    pub(crate) until: Option<Instant>,
}

impl Osd {
    pub(crate) fn new(app: &gtk::Application, monitor: &gdk::Monitor, kind: &str) -> Self {
        let window = layer_window(app, monitor, kind, Layer::Overlay, &[Edge::Right]);
        window.set_margin(Edge::Right, 16);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
        content.add_css_class("panel");
        content.add_css_class("osd");
        let progress = gtk::ProgressBar::new();
        progress.set_orientation(gtk::Orientation::Vertical);
        progress.set_inverted(true);
        progress.set_size_request(10, 100);
        progress.set_halign(gtk::Align::Center);
        let image = gtk::Image::new();
        image.set_pixel_size(24);
        let label = gtk::Label::new(None);
        content.append(&progress);
        content.append(&image);
        content.append(&label);
        window.set_child(Some(&content));
        Self {
            window,
            image,
            progress,
            label,
            until: None,
        }
    }

    pub(crate) fn show(&mut self, value: f64, icon: &str) {
        let value = if value.is_finite() { value.max(0.) } else { 0. };
        self.image.set_icon_name(Some(icon));
        self.progress.set_fraction(value.clamp(0., 1.));
        self.label.set_label(&format!("{:.0}%", value * 100.));
        self.until = Some(Instant::now() + Duration::from_secs(1));
    }

    pub(crate) fn tick(&self, safe: bool) {
        self.window
            .set_visible(safe && self.until.is_some_and(|until| until > Instant::now()));
    }

    pub(crate) fn show_volume(&mut self, volume: &system::Volume) {
        let icon = if volume.muted {
            "audio-volume-muted-symbolic"
        } else if volume.level < 0.33 {
            "audio-volume-low-symbolic"
        } else if volume.level < 0.66 {
            "audio-volume-medium-symbolic"
        } else {
            "audio-volume-high-symbolic"
        };
        self.show(volume.level, icon);
        if volume.muted {
            self.label
                .set_label(&format!("Muted\n{:.0}%", volume.level * 100.));
        }
    }
}

fn layer_window(
    app: &gtk::Application,
    monitor: &gdk::Monitor,
    kind: &str,
    layer: Layer,
    edges: &[Edge],
) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .decorated(false)
        .resizable(true)
        .build();
    window.add_css_class("astal-shell");
    window.init_layer_shell();
    window.set_namespace(Some(&format!("astal-shell-{kind}")));
    window.set_layer(layer);
    window.set_monitor(Some(monitor));
    window.set_exclusive_zone(-1);
    window.set_keyboard_mode(KeyboardMode::None);
    for edge in edges {
        window.set_anchor(*edge, true);
    }
    window
}

fn widget_bounds(
    widget: &impl IsA<gtk::Widget>,
    window: &gtk::ApplicationWindow,
    origin: (f64, f64),
) -> Option<[f64; 4]> {
    let rect = widget.as_ref().compute_bounds(window)?;
    Some([
        origin.0 + f64::from(rect.x()),
        origin.1 + f64::from(rect.y()),
        f64::from(rect.width()),
        f64::from(rect.height()),
    ])
}

fn child_bounds(
    container: &gtk::Box,
    window: &gtk::ApplicationWindow,
    origin: (f64, f64),
    ids: impl Iterator<Item = u64>,
) -> Vec<Value> {
    let mut child = container.first_child();
    ids.map_while(|id| {
        let widget = child.take()?;
        child = widget.next_sibling();
        Some(json!({
            "id": id,
            "bounds": widget_bounds(&widget, window, origin),
            "active": widget.has_css_class("active"),
            "focused": widget.has_css_class("focused"),
        }))
    })
    .collect()
}

fn named_bounds(
    widget: &gtk::Widget,
    window: &gtk::ApplicationWindow,
    origin: (f64, f64),
) -> BTreeMap<String, Option<[f64; 4]>> {
    let mut bounds = BTreeMap::new();
    let name = widget.widget_name();
    if name.starts_with("media-") {
        bounds.insert(name.to_string(), widget_bounds(widget, window, origin));
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        child = widget.next_sibling();
        bounds.extend(named_bounds(&widget, window, origin));
    }
    bounds
}

struct Bars {
    monitor: gdk::Monitor,
    top: gtk::ApplicationWindow,
    top_content: gtk::CenterBox,
    left: gtk::ApplicationWindow,
    bottom: gtk::ApplicationWindow,
    bottom_content: gtk::ScrolledWindow,
    workspaces: gtk::Box,
    workspace_buttons: BTreeMap<u64, gtk::Button>,
    dock: gtk::Box,
    dock_buttons: BTreeMap<u64, gtk::Button>,
    clock: gtk::Label,
    weather: gtk::Button,
    network: gtk::Label,
    metrics: gtk::Box,
    meters: BTreeMap<&'static str, Meter>,
    media: gtk::Box,
    tray: gtk::Box,
    actions: gtk::Box,
    volume: Osd,
    brightness: Osd,
    workspace_signature: Value,
    dock_signature: Value,
    debug_layout: Value,
    last_geometry: (i32, i32, i32, Option<LogicalOutput>),
    waiting_for_outputs: Option<u64>,
    state: Rc<RefCell<State>>,
}

impl Bars {
    fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        output: &str,
        config: &Config,
        state: Rc<RefCell<State>>,
    ) -> Self {
        let top = layer_window(
            app,
            monitor,
            "top",
            Layer::Overlay,
            &[Edge::Top, Edge::Left, Edge::Right],
        );
        let top_content = gtk::CenterBox::new();
        top_content.add_css_class("panel");
        let media = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        top_content.set_start_widget(Some(&media));
        let center = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        let clock = gtk::Label::new(None);
        clock.add_css_class("clock");
        clock.set_justify(gtk::Justification::Center);
        let date = gtk::Button::new();
        date.set_child(Some(&clock));
        date.connect_clicked(|_| system::execute(vec!["gnome-calendar".into()]));
        center.append(&date);
        let weather = gtk::Button::new();
        weather.add_css_class("weather");
        weather.set_visible(false);
        weather.connect_clicked(|_| system::execute(vec!["gnome-weather".into()]));
        center.append(&weather);
        top_content.set_center_widget(Some(&center));
        let right = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let network = gtk::Label::new(None);
        network.add_css_class("network");
        let metrics = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let hardware = gtk::GestureClick::new();
        hardware.connect_pressed(|_, _, _, _| system::execute(vec!["mission-center".into()]));
        metrics.add_controller(hardware);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let tray = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        right.append(&network);
        right.append(&metrics);
        right.append(&actions);
        right.append(&tray);
        top_content.set_end_widget(Some(&right));
        top.set_child(Some(&top_content));

        let left = layer_window(
            app,
            monitor,
            "left",
            Layer::Overlay,
            &[Edge::Left, Edge::Top, Edge::Bottom],
        );
        let workspaces = gtk::Box::new(gtk::Orientation::Vertical, 6);
        workspaces.set_halign(gtk::Align::Center);
        workspaces.set_valign(gtk::Align::Center);
        let workspace_scroll = gtk::ScrolledWindow::new();
        workspace_scroll.set_policy(gtk::PolicyType::Never, gtk::PolicyType::External);
        workspace_scroll.set_child(Some(&workspaces));
        left.set_child(Some(&workspace_scroll));

        let bottom = layer_window(
            app,
            monitor,
            "bottom",
            Layer::Overlay,
            &[Edge::Bottom, Edge::Left, Edge::Right],
        );
        let dock = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        dock.add_css_class("panel");
        dock.add_css_class("dock");
        dock.set_halign(gtk::Align::Center);
        let bottom_content = gtk::ScrolledWindow::new();
        bottom_content.set_policy(gtk::PolicyType::External, gtk::PolicyType::Never);
        bottom_content.set_propagate_natural_height(true);
        bottom_content.set_child(Some(&dock));
        bottom.set_child(Some(&bottom_content));
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        let model = state.clone();
        let output_name = output.to_owned();
        scroll.connect_scroll(move |_, _, dy| {
            let state = model.borrow();
            let windows = state.windows_on(&output_name);
            if windows.is_empty() || dy == 0. {
                return glib::Propagation::Proceed;
            }
            let active = state
                .active_on(&output_name)
                .and_then(|w| w.active_window_id);
            let current = windows
                .iter()
                .position(|w| active == Some(w.id))
                .or_else(|| windows.iter().position(|w| w.is_focused))
                .unwrap_or(0);
            let next = if dy > 0. {
                (current + 1) % windows.len()
            } else {
                (current + windows.len() - 1) % windows.len()
            };
            ipc::action(json!({"FocusWindow":{"id":windows[next].id}}));
            glib::Propagation::Stop
        });
        bottom_content.add_controller(scroll);
        let volume = Osd::new(app, monitor, "volume");
        let brightness = Osd::new(app, monitor, "brightness");
        let geometry = monitor.geometry();
        let last_geometry = (
            geometry.width(),
            geometry.height(),
            monitor.scale_factor(),
            state
                .borrow()
                .outputs
                .get(output)
                .and_then(|o| o.logical.clone()),
        );
        let output_revision = state.borrow().output_revision;
        let bars = Self {
            monitor: monitor.clone(),
            top,
            top_content,
            left,
            bottom,
            bottom_content,
            workspaces,
            workspace_buttons: BTreeMap::new(),
            dock,
            dock_buttons: BTreeMap::new(),
            clock,
            weather,
            network,
            metrics,
            meters: BTreeMap::new(),
            media,
            tray,
            actions,
            volume,
            brightness,
            workspace_signature: Value::Null,
            dock_signature: Value::Null,
            debug_layout: Value::Null,
            last_geometry,
            waiting_for_outputs: Some(output_revision),
            state,
        };
        bars.configure(app, config);
        bars
    }

    fn configure(&self, app: &gtk::Application, config: &Config) {
        while let Some(child) = self.actions.first_child() {
            self.actions.remove(&child);
        }
        for entry in &config.quick_settings {
            let button = gtk::Button::from_icon_name(&entry.icon);
            button.add_css_class("quick-action");
            button.set_tooltip_text(Some(&entry.label));
            let entry = entry.clone();
            let app = app.clone();
            let monitor = self.monitor.clone();
            button.connect_clicked(move |_| {
                if entry.confirm {
                    confirm(&app, &monitor, &entry);
                } else if let Ok(argv) = entry.command.argv() {
                    system::execute(argv);
                }
            });
            self.actions.append(&button);
        }
        if !config.settings.weather.enable {
            self.weather.set_visible(false);
        }
        for meter in self.meters.values() {
            meter.configure(config);
        }
    }

    fn update_model(&mut self, output: &str, icons: &AppIcons) {
        let state = self.state.borrow();
        let workspaces = state.workspaces_on(output);
        let signature = json!(
            workspaces
                .iter()
                .map(|w| (w.id, w.idx, &w.name, w.is_active, w.is_focused, w.is_urgent))
                .collect::<Vec<_>>()
        );
        if signature != self.workspace_signature {
            let ids: BTreeSet<_> = workspaces.iter().map(|w| w.id).collect();
            self.workspace_buttons.retain(|id, button| {
                if ids.contains(id) {
                    true
                } else {
                    self.workspaces.remove(button);
                    false
                }
            });
            let mut previous: Option<gtk::Widget> = None;
            for workspace in workspaces {
                let id = workspace.id;
                let button = self.workspace_buttons.entry(id).or_insert_with(|| {
                    let button = gtk::Button::new();
                    button.add_css_class("workspace");
                    button.set_size_request(12, 24);
                    button.connect_clicked(move |_| {
                        ipc::action(json!({"FocusWorkspace":{"reference":{"Id":id}}}))
                    });
                    self.workspaces.append(&button);
                    button
                });
                for class in ["active", "focused", "urgent"] {
                    button.remove_css_class(class);
                }
                if workspace.is_active {
                    button.add_css_class("active");
                }
                if workspace.is_focused {
                    button.add_css_class("focused");
                }
                if workspace.is_urgent {
                    button.add_css_class("urgent");
                }
                button.set_tooltip_text(Some(&format!(
                    "{}: {}",
                    workspace.idx,
                    workspace.name.as_deref().unwrap_or(output)
                )));
                self.workspaces
                    .reorder_child_after(button, previous.as_ref());
                previous = Some(button.clone().upcast());
            }
            self.workspace_signature = signature;
        }
        let windows = state.windows_on(output);
        let active = state.active_on(output).and_then(|w| w.active_window_id);
        let signature = json!(
            windows
                .iter()
                .map(|w| (
                    w.id,
                    &w.app_id,
                    &w.title,
                    w.is_focused,
                    w.is_urgent,
                    active == Some(w.id)
                ))
                .collect::<Vec<_>>()
        );
        if signature != self.dock_signature {
            let ids: BTreeSet<_> = windows.iter().map(|w| w.id).collect();
            self.dock_buttons.retain(|id, button| {
                if ids.contains(id) {
                    true
                } else {
                    self.dock.remove(button);
                    false
                }
            });
            let mut previous: Option<gtk::Widget> = None;
            for window in windows {
                let id = window.id;
                let button = self.dock_buttons.entry(id).or_insert_with(|| {
                    let button = dock_button(id);
                    self.dock.append(&button);
                    button
                });
                button.set_child(Some(
                    &icons.image(window.app_id.as_deref().unwrap_or(""), 36),
                ));
                button.set_tooltip_text(window.title.as_deref().or(window.app_id.as_deref()));
                for class in ["focused", "urgent"] {
                    button.remove_css_class(class);
                }
                if active == Some(window.id) {
                    button.add_css_class("focused");
                }
                if window.is_urgent {
                    button.add_css_class("urgent");
                }
                self.dock.reorder_child_after(button, previous.as_ref());
                previous = Some(button.clone().upcast());
            }
            self.dock_signature = signature;
        }
    }

    fn layout(&mut self, output: &str, config: &Config, shown: bool, refresh: &mpsc::Sender<()>) {
        let state = self.state.borrow();
        let geo = self.monitor.geometry();
        let logical = state.outputs.get(output).and_then(|o| o.logical.clone());
        let geometry = (
            geo.width(),
            geo.height(),
            self.monitor.scale_factor(),
            logical.clone(),
        );
        if geometry != self.last_geometry {
            self.last_geometry = geometry;
            self.waiting_for_outputs = Some(state.output_revision);
            let _ = refresh.send(());
        }
        if self
            .waiting_for_outputs
            .is_some_and(|revision| revision != state.output_revision)
        {
            self.waiting_for_outputs = None;
        }
        // Hide while output/GTK geometry settles. Never display old margins on
        // a resized or rescaled monitor while the next IPC snapshot is pending.
        let shown = shown && self.waiting_for_outputs.is_none() && logical.is_some();
        let size = state
            .outputs
            .get(output)
            .and_then(|o| o.logical.as_ref())
            .map(|l| OutputSize {
                width: l.width as f64,
                height: l.height as f64,
                scale: l.scale,
            })
            .unwrap_or(OutputSize {
                width: geo.width() as f64,
                height: geo.height() as f64,
                scale: self.monitor.scale_factor() as f64,
            });
        let width = (size.width * config.overview_zoom).floor().max(1.) as i32;
        for widget in [
            &self.media.upcast_ref::<gtk::Widget>(),
            &self.network.upcast_ref::<gtk::Widget>(),
            &self.metrics.upcast_ref::<gtk::Widget>(),
        ] {
            widget.set_visible(true);
        }
        // Remove optional content until GTK's minimum width fits the card.
        for widget in [
            &self.metrics.upcast_ref::<gtk::Widget>(),
            &self.network.upcast_ref::<gtk::Widget>(),
            &self.media.upcast_ref::<gtk::Widget>(),
        ] {
            if self.top_content.measure(gtk::Orientation::Horizontal, -1).0 <= width {
                break;
            }
            widget.set_visible(false);
        }
        let top_height = self
            .top_content
            .measure(gtk::Orientation::Vertical, width)
            .1;
        let bottom_height = self.dock.measure(gtk::Orientation::Vertical, -1).1;
        let left_width = self
            .workspaces
            .measure(gtk::Orientation::Horizontal, -1)
            .1
            .max(20);
        let placement = geometry::place(
            size,
            config.overview_zoom,
            top_height,
            bottom_height,
            left_width,
            config.settings.clearance,
        );
        for window in [&self.top, &self.bottom] {
            window.set_margin(Edge::Left, placement.horizontal);
            window.set_margin(Edge::Right, placement.horizontal);
        }
        self.top.set_margin(Edge::Top, placement.vertical);
        self.bottom
            .set_margin(Edge::Bottom, placement.bottom_vertical);
        self.bottom_content.set_max_content_width(width);
        self.left.set_margin(Edge::Left, placement.left);
        self.left.set_default_size(left_width, -1);
        self.left
            .set_margin(Edge::Top, config.settings.clearance + 2);
        self.left
            .set_margin(Edge::Bottom, config.settings.clearance + 2);
        let top_fits =
            self.top_content.measure(gtk::Orientation::Horizontal, -1).0 <= placement.card_width;
        self.top
            .set_visible(shown && placement.top_visible && top_fits);
        self.bottom
            .set_visible(shown && placement.bottom_visible && !state.windows_on(output).is_empty());
        self.left.set_visible(shown && placement.left_visible);
        let osd_width = self
            .volume
            .window
            .child()
            .map(|w| w.measure(gtk::Orientation::Horizontal, -1).1)
            .unwrap_or(64);
        let osd_safe = state.connected
            && self.waiting_for_outputs.is_none()
            && logical.is_some()
            && (!state.overview_open || placement.horizontal > osd_width + 20);
        self.volume.tick(osd_safe);
        self.brightness.tick(osd_safe);
        if std::env::var_os("ASTAL_SHELL_DEBUG_LAYOUT").is_some() {
            let top_origin = (
                f64::from(placement.horizontal),
                f64::from(placement.vertical),
            );
            let bottom_origin = (
                f64::from(placement.horizontal),
                size.height - f64::from(placement.bottom_vertical + self.bottom.height()),
            );
            let snapshot = json!({
                "output": output,
                "top": [self.top.width(), self.top.height()],
                "panel": [self.top_content.width(), self.top_content.height()],
                "bottom": [self.bottom.width(), self.bottom.height()],
                "left": [self.left.width(), self.left.height()],
                "placement": placement,
                "shown": shown,
                "connected": state.connected,
                "revealDelayMs": config.settings.reveal_delay_ms,
                "clock": self.clock.text().to_string(),
                "targets": {
                    "workspaces": child_bounds(&self.workspaces, &self.left,
                        (f64::from(placement.left), f64::from(config.settings.clearance + 2)),
                        state.workspaces_on(output).iter().map(|w| w.id)),
                    "dock": child_bounds(&self.dock, &self.bottom, bottom_origin,
                        state.windows_on(output).iter().map(|w| w.id)),
                    "media": named_bounds(self.media.upcast_ref(), &self.top, top_origin),
                    "tray": self.tray.first_child().and_then(|w| widget_bounds(&w, &self.top, top_origin)),
                },
                "volume": {
                    "visible": self.volume.window.is_visible(),
                    "label": self.volume.label.text().to_string(),
                    "fraction": self.volume.progress.fraction(),
                },
            });
            if snapshot != self.debug_layout {
                eprintln!("layout: {snapshot}");
                self.debug_layout = snapshot;
            }
        }
    }

    fn update_metrics(&mut self, m: &Metrics, config: &Config) {
        let mut values = vec![
            ("CPU", m.cpu, format!("CPU: {:.0}%", m.cpu * 100.)),
            ("RAM", m.memory, format!("RAM: {:.0}%", m.memory * 100.)),
            ("HDD", m.disk, format!("Disk: {:.0}%", m.disk * 100.)),
        ];
        if let Some((gpu, vram, temp)) = m.gpu {
            values.push((
                "GPU",
                gpu,
                format!("GPU: {:.0}%\nTemperature: {temp:.0} C", gpu * 100.),
            ));
            values.push(("VRAM", vram, format!("VRAM: {:.0}%", vram * 100.)));
        }
        if let Some((capacity, details)) = &m.battery {
            values.push(("BAT", *capacity, details.clone()));
        }
        for (name, value, tooltip) in &values {
            let meter = self.meters.entry(name).or_insert_with(|| {
                let meter = Meter::new(name, *name == "BAT", config);
                self.metrics.append(&meter.row);
                meter
            });
            meter.row.set_visible(true);
            meter.set(*value, tooltip);
        }
        for (name, meter) in &self.meters {
            meter.row.set_visible(values.iter().any(|v| &v.0 == name));
        }
        if m.upload >= 1024. || m.download >= 1024. {
            self.network.set_label(&format!(
                "\u{25b2} {}\n\u{25bc} {}",
                system::format_rate(m.upload),
                system::format_rate(m.download)
            ));
        } else {
            self.network.set_label("");
        }
    }

    fn close(self) {
        for window in [
            self.top,
            self.left,
            self.bottom,
            self.volume.window,
            self.brightness.window,
        ] {
            window.close();
        }
    }
}

fn dock_actions(actions: Vec<Value>) {
    // Focusing must complete before column operations.
    std::thread::spawn(move || {
        for action in actions {
            if let Err(e) = ipc::request(&json!({"Action": action})) {
                eprintln!("Dock action: {e}");
                break;
            }
        }
    });
}

fn dock_button(id: u64) -> gtk::Button {
    let button = gtk::Button::new();
    button.connect_clicked(move |_| {
        dock_actions(vec![
            json!({"FocusWindow":{"id":id}}),
            json!({"CenterColumn":{}}),
        ]);
    });
    for mouse_button in [1, 3] {
        let click = gtk::GestureClick::new();
        click.set_button(mouse_button);
        click.connect_pressed(move |gesture, presses, _, _| {
            let actions = match mouse_button {
                1 if presses == 2 => vec![
                    json!({"FocusWindow":{"id":id}}),
                    json!({"MaximizeColumn":{}}),
                    json!({"CenterWindow":{"id":id}}),
                ],
                1 => vec![json!({"FocusWindow":{"id":id}}), json!({"CenterColumn":{}})],
                _ => vec![json!({"CloseWindow":{"id":id}})],
            };
            dock_actions(actions);
            gesture.set_state(gtk::EventSequenceState::Claimed);
        });
        button.add_controller(click);
    }
    let long = gtk::GestureLongPress::new();
    long.connect_pressed(move |_, _, _| ipc::action(json!({"CloseWindow":{"id":id}})));
    button.add_controller(long);
    button
}

fn confirm(app: &gtk::Application, monitor: &gdk::Monitor, entry: &QuickSetting) {
    let window = layer_window(
        app,
        monitor,
        "confirm",
        Layer::Overlay,
        &[Edge::Left, Edge::Right, Edge::Top, Edge::Bottom],
    );
    window.set_keyboard_mode(KeyboardMode::OnDemand);
    let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
    container.add_css_class("panel");
    container.add_css_class("confirm");
    container.set_halign(gtk::Align::Center);
    container.set_valign(gtk::Align::Center);
    let title = gtk::Label::new(Some(&format!("{}?", entry.label)));
    title.add_css_class("confirm-title");
    container.append(&title);
    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let cancel = gtk::Button::with_label("Cancel");
    cancel.set_widget_name("confirm-cancel");
    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let accept = gtk::Button::with_label("Confirm");
    accept.set_widget_name("confirm-accept");
    accept.add_css_class("danger");
    let entry = entry.clone();
    let weak = window.downgrade();
    accept.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
        if let Ok(argv) = entry.command.argv() {
            system::execute(argv);
        }
    });
    buttons.append(&cancel);
    buttons.append(&accept);
    container.append(&buttons);
    window.set_child(Some(&container));
    let key = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(key);
    window.present();
    cancel.grab_focus();
}

fn apply_theme(provider: &gtk::CssProvider, config: &Config, reset_icon: Option<&str>) {
    provider.load_from_string(&config.css());
    if let Some(icon) = config.icon_theme().or(reset_icon)
        && let Some(settings) = gtk::Settings::default()
    {
        settings.set_gtk_icon_theme_name(Some(icon));
    }
}

fn session_icon_theme() -> Option<String> {
    let ini = glib::KeyFile::new();
    ini.load_from_file(
        astal_shell::config::config_home().join("gtk-4.0/settings.ini"),
        glib::KeyFileFlags::NONE,
    )
    .ok()?;
    ini.string("Settings", "gtk-icon-theme-name")
        .ok()
        .map(|s| s.to_string())
}

pub fn start(app: &gtk::Application) -> bool {
    let app = app.clone();
    let hold = app.hold();
    let mut config = match Config::load() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Configuration: {e}");
            app.quit();
            return false;
        }
    };
    let display = gdk::Display::default().unwrap();
    let inherited_icon_theme = gtk::Settings::default()
        .and_then(|s| s.gtk_icon_theme_name())
        .unwrap_or_else(|| "Adwaita".into())
        .to_string();
    let provider = gtk::CssProvider::new();
    gtk::style_context_add_provider_for_display(
        &display,
        &provider,
        gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
    );
    apply_theme(&provider, &config, None);
    let monitors = display.monitors();
    let model = Rc::new(RefCell::new(State::default()));
    let (ipc, refresh_outputs) = ipc::start();
    let system = system::start();
    let media = media::start();
    let tray = match Tray::new() {
        Ok(tray) => Some(tray),
        Err(e) => {
            eprintln!("Tray unavailable: {e}");
            None
        }
    };
    let (weather_tx, weather_rx) = mpsc::channel();
    let (weather_settings_tx, weather_settings_rx) = mpsc::channel();
    let _ = weather_settings_tx.send(config.settings.weather.clone());
    system::start_weather(weather_tx, weather_settings_rx);
    let mut icons = AppIcons::new();
    let icon_dirty = Rc::new(std::cell::Cell::new(false));
    let changed = icon_dirty.clone();
    gio::AppInfoMonitor::get().connect_changed(move |_| changed.set(true));
    let mut bars = BTreeMap::<String, Bars>::new();
    let mut last_config = Instant::now();
    let mut last_tray = Instant::now() - Duration::from_secs(1);
    let mut config_error = String::new();
    let mut players = Vec::new();
    let mut tray_revision = 0;
    let mut metrics = Metrics::default();
    let mut overview_since = None;
    let mut last_volume = None;
    let mut last_brightness = None;
    let mut weather_value: Option<WeatherDisplay> = None;
    glib::timeout_add_local(Duration::from_millis(50), move || {
        let _hold = &hold;
        for message in ipc.try_iter().take(1000) {
            let mut state = model.borrow_mut();
            match message {
                Message::Connection(connected) => {
                    state.connected = connected;
                    if !connected {
                        state.overview_open = false;
                        state.workspaces.clear();
                        state.windows.clear();
                        state.outputs.clear();
                    }
                }
                Message::Outputs(outputs) => {
                    state.outputs = outputs;
                    state.output_revision = state.output_revision.wrapping_add(1);
                }
                Message::Event(event) => {
                    if state.overview_open && event.get("WorkspaceActivated").is_some() {
                        overview_since = Some(Instant::now());
                    }
                    if let Err(e) = state.apply(event) {
                        eprintln!("Niri event: {e}");
                    }
                }
            }
        }
        let mut config_changed = false;
        if last_config.elapsed() >= Duration::from_secs(1) {
            match Config::load() {
                Ok(next) => {
                    config_error.clear();
                    if next != config {
                        let reset = (config.icon_theme().is_some() && next.icon_theme().is_none())
                            .then(|| {
                                session_icon_theme().unwrap_or_else(|| inherited_icon_theme.clone())
                            });
                        config = next;
                        config_changed = true;
                        apply_theme(&provider, &config, reset.as_deref());
                        let _ = weather_settings_tx.send(config.settings.weather.clone());
                    }
                }
                Err(e) => {
                    if e != config_error {
                        eprintln!("Keeping previous configuration: {e}");
                        config_error = e;
                    }
                }
            }
            last_config = Instant::now();
        }
        let mut icons_changed = false;
        if icon_dirty.replace(false) {
            icons.refresh();
            icons_changed = true;
        }
        let mut connected = BTreeSet::new();
        let mut new_bars = BTreeSet::new();
        for index in 0..monitors.n_items() {
            let Some(monitor) = monitors.item(index).and_downcast::<gdk::Monitor>() else {
                continue;
            };
            let Some(connector) = monitor.connector() else {
                continue;
            };
            let name = connector.to_string();
            connected.insert(name.clone());
            if bars.get(&name).is_some_and(|b| b.monitor != monitor)
                && let Some(old) = bars.remove(&name)
            {
                old.close();
            }
            if !bars.contains_key(&name) {
                bars.insert(
                    name.clone(),
                    Bars::new(&app, &monitor, &name, &config, model.clone()),
                );
                new_bars.insert(name);
            }
        }
        let removed: Vec<_> = bars
            .keys()
            .filter(|k| !connected.contains(*k))
            .cloned()
            .collect();
        for name in removed {
            if let Some(old) = bars.remove(&name) {
                old.close();
            }
        }
        if !new_bars.is_empty() {
            let _ = refresh_outputs.send(());
        }
        let mut media_changed = false;
        for next in media.try_iter() {
            players = next;
            media_changed = true;
        }
        if last_tray.elapsed() >= Duration::from_secs(1) {
            if let Some(tray) = &tray {
                tray.refresh();
            }
            last_tray = Instant::now();
        }
        let next_tray_revision = tray.as_ref().map(Tray::revision).unwrap_or(0);
        let tray_changed = next_tray_revision != tray_revision;
        tray_revision = next_tray_revision;
        let focused_output = model
            .borrow()
            .workspaces
            .values()
            .find(|w| w.is_focused)
            .and_then(|w| w.output.clone());
        let mut volume_event = None;
        let mut brightness_event = None;
        let mut metrics_changed = false;
        let mut weather_changed = false;
        for update in system.try_iter().chain(weather_rx.try_iter()) {
            match update {
                Update::Metrics(next) => {
                    metrics = next;
                    metrics_changed = true;
                }
                Update::Volume(next) => {
                    if last_volume.is_some() {
                        volume_event = Some(next.clone());
                    }
                    last_volume = Some(next);
                }
                Update::Brightness(next) => {
                    if last_brightness.is_some() {
                        brightness_event = Some(next);
                    }
                    last_brightness = Some(next);
                }
                Update::Weather(display) => {
                    weather_value = Some(display);
                    weather_changed = true;
                }
            }
        }
        let state = model.borrow();
        if state.overview_open && state.connected {
            if overview_since.is_none() {
                overview_since = Some(Instant::now());
            }
        } else {
            overview_since = None;
        }
        let shown = overview_since.is_some_and(|since| {
            since.elapsed() >= Duration::from_millis(config.settings.reveal_delay_ms)
        });
        drop(state);
        let clock = glib::DateTime::now_local()
            .ok()
            .and_then(|d| d.format("%a %d %b\n%H:%M:%S").ok())
            .unwrap_or_default();
        for (name, bar) in &mut bars {
            let new = new_bars.contains(name);
            if config_changed {
                bar.configure(&app, &config);
            }
            if icons_changed || config_changed {
                bar.dock_signature = Value::Null;
            }
            if media_changed || new || config_changed {
                media::render(&bar.media, &players);
            }
            if (tray_changed || new)
                && let Some(tray) = &tray
            {
                tray.render(&bar.tray);
            }
            if bar.clock.text() != clock {
                bar.clock.set_label(&clock);
            }
            if metrics_changed || new || config_changed {
                bar.update_metrics(&metrics, &config);
            }
            if (weather_changed || new || config_changed)
                && let Some(display) = &weather_value
            {
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
                if let Some(icon) = display.icon {
                    let image = gtk::Image::from_icon_name(icon);
                    image.set_pixel_size(14);
                    row.append(&image);
                }
                row.append(&gtk::Label::new(Some(&display.text)));
                bar.weather.set_child(Some(&row));
                bar.weather.set_tooltip_text(Some(&display.tooltip));
                bar.weather.set_visible(config.settings.weather.enable);
            }
            if focused_output.as_deref() == Some(name) {
                if let Some(volume) = &volume_event {
                    bar.volume.show_volume(volume);
                }
                if let Some(brightness) = brightness_event {
                    bar.brightness
                        .show(brightness, "display-brightness-symbolic");
                }
            }
            bar.update_model(name, &icons);
            bar.layout(name, &config, shown, &refresh_outputs);
        }
        glib::ControlFlow::Continue
    });
    true
}
