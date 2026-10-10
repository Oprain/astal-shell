use gtk::{gio, prelude::*};
use std::collections::HashMap;

pub struct AppIcons {
    entries: HashMap<String, gio::Icon>,
    _monitor: gio::AppInfoMonitor,
}

impl AppIcons {
    pub fn new() -> Self {
        let mut this = Self {
            entries: HashMap::new(),
            _monitor: gio::AppInfoMonitor::get(),
        };
        this.refresh();
        this
    }

    pub fn refresh(&mut self) {
        self.entries.clear();
        for app in gio::AppInfo::all() {
            let Some(icon) = app.icon() else {
                continue;
            };
            if let Some(id) = app.id() {
                let key = id
                    .strip_suffix(".desktop")
                    .unwrap_or(id.as_str())
                    .to_ascii_lowercase();
                self.entries.insert(key, icon.clone());
                if let Some(desktop) = gio::DesktopAppInfo::new(&id)
                    && let Some(class) = desktop.startup_wm_class()
                {
                    self.entries
                        .entry(class.to_ascii_lowercase())
                        .or_insert_with(|| icon.clone());
                }
            }
        }
    }

    pub fn image(&self, app_id: &str, size: i32) -> gtk::Image {
        let key = app_id
            .strip_suffix(".desktop")
            .unwrap_or(app_id)
            .to_ascii_lowercase();
        let image = if let Some(icon) = self.entries.get(&key) {
            gtk::Image::from_gicon(icon)
        } else {
            let theme = gtk::IconTheme::for_display(&gtk::gdk::Display::default().unwrap());
            if theme.has_icon(app_id) {
                gtk::Image::from_icon_name(app_id)
            } else {
                gtk::Image::from_icon_name("application-x-executable")
            }
        };
        image.set_pixel_size(size);
        image
    }
}
