use crate::artwork::ArtworkCache;
use glib::variant::ToVariant;
use gtk::{gdk, gio, glib, graphene, prelude::*};
use std::{
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicI32, Ordering},
        mpsc::{self, Receiver},
    },
    thread,
    time::Duration,
};

const PATH: &str = "/org/mpris/MediaPlayer2";
const PLAYER: &str = "org.mpris.MediaPlayer2.Player";

#[derive(Clone, Debug, PartialEq)]
pub struct Player {
    pub bus_name: String,
    pub artist: String,
    pub title: String,
    pub artwork: String,
    pub cover: Option<PathBuf>,
    pub playing: bool,
    pub toggle_method: Option<&'static str>,
    pub can_next: bool,
}

pub fn start() -> (Receiver<Vec<Player>>, Arc<AtomicI32>) {
    let (tx, rx) = mpsc::channel();
    let size = Arc::new(AtomicI32::new(36));
    let artwork_size = size.clone();
    thread::spawn(move || {
        let mut previous = Vec::new();
        let mut artwork = ArtworkCache::new();
        loop {
            if let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE) {
                let names = bus
                    .call_sync(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                        "ListNames",
                        None,
                        None,
                        gio::DBusCallFlags::NONE,
                        1000,
                        gio::Cancellable::NONE,
                    )
                    .ok()
                    .and_then(|v| v.get::<(Vec<String>,)>())
                    .map(|v| v.0)
                    .unwrap_or_default();
                let mut players = Vec::new();
                artwork.begin_update();
                for name in names
                    .into_iter()
                    .filter(|n| n.starts_with("org.mpris.MediaPlayer2."))
                {
                    if let Some(mut player) = read_player(&bus, name) {
                        player.cover =
                            artwork.resolve(&player.artwork, size.load(Ordering::Relaxed));
                        players.push(player);
                    }
                }
                artwork.end_update();
                players.sort_by(|a, b| b.playing.cmp(&a.playing).then(a.bus_name.cmp(&b.bus_name)));
                if players != previous {
                    previous = players.clone();
                    if tx.send(players).is_err() {
                        return;
                    }
                }
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
    (rx, artwork_size)
}

fn read_player(bus: &gio::DBusConnection, name: String) -> Option<Player> {
    let reply = bus
        .call_sync(
            Some(&name),
            PATH,
            "org.freedesktop.DBus.Properties",
            "GetAll",
            Some(&(PLAYER,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            700,
            gio::Cancellable::NONE,
        )
        .ok()?;
    let (props,) = reply.get::<(HashMap<String, glib::Variant>,)>()?;
    player_from_properties(name, &props)
}

fn player_from_properties(name: String, props: &HashMap<String, glib::Variant>) -> Option<Player> {
    let metadata = props
        .get("Metadata")?
        .get::<HashMap<String, glib::Variant>>()?;
    let string = |props: &HashMap<String, glib::Variant>, key: &str| {
        props
            .get(key)
            .and_then(|v| v.str())
            .unwrap_or("")
            .to_owned()
    };
    let boolean = |key: &str| {
        props
            .get(key)
            .and_then(|v| v.get::<bool>())
            .unwrap_or(false)
    };
    let playing = string(props, "PlaybackStatus") == "Playing";
    let can_control = boolean("CanControl");
    let can_pause = boolean("CanPause");
    let toggle_method = if !can_control {
        None
    } else if playing {
        can_pause.then_some("PlayPause")
    } else if boolean("CanPlay") {
        Some(if can_pause { "PlayPause" } else { "Play" })
    } else {
        None
    };
    Some(Player {
        bus_name: name,
        artist: metadata
            .get("xesam:artist")
            .and_then(|v| v.get::<Vec<String>>())
            .unwrap_or_default()
            .join(", "),
        title: string(&metadata, "xesam:title"),
        artwork: string(&metadata, "mpris:artUrl"),
        cover: None,
        playing,
        toggle_method,
        can_next: can_control && boolean("CanGoNext"),
    })
}

pub fn action(bus_name: &str, method: &str) {
    let bus_name = bus_name.to_owned();
    let method = method.to_owned();
    thread::spawn(move || {
        if let Ok(bus) = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)
            && let Err(e) = bus.call_sync(
                Some(&bus_name),
                PATH,
                PLAYER,
                &method,
                None,
                None,
                gio::DBusCallFlags::NONE,
                1000,
                gio::Cancellable::NONE,
            )
        {
            eprintln!("MPRIS: {e}");
        }
    });
}

#[derive(Default)]
pub struct Textures(HashMap<PathBuf, gdk::Paintable>);

impl Textures {
    pub fn retain(&mut self, players: &[Player]) {
        self.0.retain(|path, _| {
            players
                .iter()
                .any(|player| player.cover.as_ref() == Some(path))
        });
    }

    fn get(&mut self, path: &PathBuf) -> Option<gdk::Paintable> {
        if let Some(paintable) = self.0.get(path) {
            return Some(paintable.clone());
        }
        let texture = gdk::Texture::from_filename(path).ok()?;
        let side = 36.;
        let scale = side / texture.width().min(texture.height()).max(1) as f32;
        let width = texture.width() as f32 * scale;
        let height = texture.height() as f32 * scale;
        let snapshot = gtk::Snapshot::new();
        snapshot.push_clip(&graphene::Rect::new(0., 0., side, side));
        snapshot.append_texture(
            &texture,
            &graphene::Rect::new((side - width) / 2., (side - height) / 2., width, height),
        );
        snapshot.pop();
        // A size request is only a minimum; bound the artwork's natural size too.
        let paintable = snapshot.to_paintable(Some(&graphene::Size::new(side, side)));
        let paintable = paintable?;
        self.0.insert(path.clone(), paintable.clone());
        Some(paintable)
    }
}

struct Row {
    row: gtk::Box,
    image: gtk::Picture,
    play: gtk::Button,
    next: gtk::Button,
    artist: gtk::Label,
    title: gtk::Label,
    method: Rc<Cell<Option<&'static str>>>,
    cover: Option<PathBuf>,
}

impl Row {
    fn new(player: &Player) -> Self {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        row.set_widget_name("media-player");
        let cover = gtk::Overlay::new();
        cover.set_widget_name("media-cover");
        cover.add_css_class("cover-art");
        cover.set_overflow(gtk::Overflow::Hidden);
        let image = gtk::Picture::new();
        image.set_widget_name("media-artwork");
        image.set_size_request(36, 36);
        image.set_content_fit(gtk::ContentFit::Cover);
        cover.set_child(Some(&image));
        let play = gtk::Button::new();
        play.set_widget_name("media-play-pause");
        play.add_css_class("cover-control");
        play.set_halign(gtk::Align::Center);
        play.set_valign(gtk::Align::Center);
        play.set_tooltip_text(Some("Play / pause"));
        let name = player.bus_name.clone();
        let method = Rc::new(Cell::new(player.toggle_method));
        let current_method = method.clone();
        play.connect_clicked(move |_| {
            if let Some(method) = current_method.get() {
                action(&name, method);
            }
        });
        play.set_opacity(0.);
        cover.add_overlay(&play);
        let hover = gtk::EventControllerMotion::new();
        let weak = play.downgrade();
        hover.connect_enter(move |_, _, _| {
            if let Some(play) = weak.upgrade() {
                play.set_opacity(1.);
            }
        });
        let weak = play.downgrade();
        hover.connect_leave(move |_| {
            if let Some(play) = weak.upgrade() {
                play.set_opacity(0.);
            }
        });
        cover.add_controller(hover);
        let weak = play.downgrade();
        play.connect_has_focus_notify(move |button| {
            if let Some(play) = weak.upgrade() {
                play.set_opacity(if button.has_focus() { 1. } else { 0. });
            }
        });
        row.append(&cover);
        let text = gtk::Box::new(gtk::Orientation::Vertical, 0);
        text.add_css_class("media-text");
        text.set_valign(gtk::Align::Center);
        let artist = gtk::Label::new(None);
        artist.set_widget_name("media-artist");
        artist.add_css_class("media-artist");
        artist.set_halign(gtk::Align::Start);
        artist.set_ellipsize(gtk::pango::EllipsizeMode::End);
        artist.set_max_width_chars(20);
        text.append(&artist);
        let label = gtk::Label::new(None);
        label.set_widget_name("media-title");
        label.add_css_class("media-title");
        label.set_halign(gtk::Align::Start);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_max_width_chars(16);
        text.append(&label);
        row.append(&text);
        let next = gtk::Button::from_icon_name("media-skip-forward-symbolic");
        next.set_widget_name("media-next");
        let name = player.bus_name.clone();
        next.connect_clicked(move |_| action(&name, "Next"));
        row.append(&next);
        Self {
            row,
            image,
            play,
            next,
            artist,
            title: label,
            method,
            cover: None,
        }
    }

    fn update(&mut self, player: &Player, textures: &mut Textures) {
        if self.cover != player.cover || self.image.paintable().is_none() {
            let paintable = player.cover.as_ref().and_then(|path| textures.get(path));
            self.image.set_paintable(paintable.as_ref());
            self.cover = player.cover.clone();
        }
        self.method.set(player.toggle_method);
        self.play.set_sensitive(player.toggle_method.is_some());
        self.play.set_icon_name(if player.playing {
            "media-playback-pause-symbolic"
        } else {
            "media-playback-start-symbolic"
        });
        self.next.set_visible(player.can_next);
        self.artist.set_label(&player.artist);
        self.artist.set_visible(!player.artist.is_empty());
        self.title.set_label(&player.title);
        self.title
            .set_tooltip_text(Some(&format!("{}\n{}", player.artist, player.title)));
    }
}

#[derive(Default)]
pub struct View {
    rows: HashMap<String, Row>,
}

impl View {
    pub fn render(&mut self, container: &gtk::Box, players: &[Player], textures: &mut Textures) {
        self.rows.retain(|name, row| {
            let keep = players.iter().any(|player| &player.bus_name == name);
            if !keep {
                container.remove(&row.row);
            }
            keep
        });
        let mut previous: Option<gtk::Widget> = None;
        for player in players {
            let row = self.rows.entry(player.bus_name.clone()).or_insert_with(|| {
                let row = Row::new(player);
                container.append(&row.row);
                row
            });
            row.update(player, textures);
            container.reorder_child_after(&row.row, previous.as_ref());
            previous = Some(row.row.clone().upcast());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn controls_follow_playback_state_and_mpris_capabilities() {
        let props = HashMap::from([
            (
                "Metadata".into(),
                HashMap::<String, glib::Variant>::new().to_variant(),
            ),
            ("PlaybackStatus".into(), "Playing".to_variant()),
            ("CanControl".into(), true.to_variant()),
            ("CanPlay".into(), true.to_variant()),
            ("CanPause".into(), false.to_variant()),
            ("CanGoNext".into(), true.to_variant()),
        ]);
        let player = |props: &HashMap<String, glib::Variant>| {
            player_from_properties("org.mpris.MediaPlayer2.fixture".into(), props).unwrap()
        };
        assert_eq!(player(&props).toggle_method, None);
        assert!(player(&props).can_next);

        let mut paused = props.clone();
        paused.insert("PlaybackStatus".into(), "Paused".to_variant());
        assert_eq!(player(&paused).toggle_method, Some("Play"));
        paused.insert("CanPause".into(), true.to_variant());
        assert_eq!(player(&paused).toggle_method, Some("PlayPause"));
        paused.insert("CanPlay".into(), false.to_variant());
        assert_eq!(player(&paused).toggle_method, None);

        let mut playing = paused.clone();
        playing.insert("PlaybackStatus".into(), "Playing".to_variant());
        assert_eq!(player(&playing).toggle_method, Some("PlayPause"));
        playing.insert("CanControl".into(), false.to_variant());
        assert_eq!(player(&playing).toggle_method, None);
        assert!(!player(&playing).can_next);
    }
}
