use glib::variant::ToVariant;
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, HashMap},
    rc::Rc,
};

const WATCHER: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const ITEM: &str = "org.kde.StatusNotifierItem";
const XML: &str = r#"<node><interface name="org.kde.StatusNotifierWatcher">
 <method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
 <method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
 <property name="RegisteredStatusNotifierItems" type="as" access="read"/>
 <property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
 <property name="ProtocolVersion" type="i" access="read"/>
 <signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
 <signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal>
 <signal name="StatusNotifierHostRegistered"/>
</interface></node>"#;

#[derive(Clone)]
pub struct TrayItem {
    pub address: String,
    pub props: HashMap<String, glib::Variant>,
}

struct ItemView {
    button: gtk::Button,
    item: Rc<RefCell<TrayItem>>,
}

#[derive(Default)]
pub struct View {
    buttons: BTreeMap<String, ItemView>,
}

impl TrayItem {
    fn string(&self, key: &str) -> &str {
        self.props.get(key).and_then(|v| v.str()).unwrap_or("")
    }
}

struct Inner {
    bus: gio::DBusConnection,
    items: RefCell<BTreeMap<String, TrayItem>>,
    pending: RefCell<BTreeSet<String>>,
    revision: Cell<u64>,
    own_watcher: Cell<bool>,
}

pub struct Tray {
    inner: Rc<Inner>,
    _owner: gio::OwnerId,
    _registration: gio::RegistrationId,
    _subscriptions: Vec<gio::SignalSubscription>,
}

fn address_parts(address: &str) -> (&str, &str) {
    address
        .split_once('/')
        .map(|(name, _)| (name, &address[name.len()..]))
        .unwrap_or((address, "/StatusNotifierItem"))
}

impl Tray {
    pub fn new() -> Result<Self, glib::Error> {
        let bus = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
        let inner = Rc::new(Inner {
            bus: bus.clone(),
            items: RefCell::new(BTreeMap::new()),
            pending: RefCell::new(BTreeSet::new()),
            revision: Cell::new(0),
            own_watcher: Cell::new(false),
        });
        let info = gio::DBusNodeInfo::for_xml(XML)?
            .lookup_interface(WATCHER)
            .unwrap();
        let state = inner.clone();
        let props = inner.clone();
        let registration = bus
            .register_object(WATCHER_PATH, &info)
            .method_call(move |bus, sender, _, _, method, params, invocation| {
                match method {
                    "RegisterStatusNotifierItem" => {
                        if let Some((service,)) = params.get::<(String,)>() {
                            let address = if service.starts_with('/') {
                                format!("{}{}", sender.unwrap_or(""), service)
                            } else {
                                format!("{service}/StatusNotifierItem")
                            };
                            add_item(&state, &address);
                            let _ = bus.emit_signal(
                                None,
                                WATCHER_PATH,
                                WATCHER,
                                "StatusNotifierItemRegistered",
                                Some(&(address,).to_variant()),
                            );
                        }
                    }
                    "RegisterStatusNotifierHost" => {
                        let _ = bus.emit_signal(
                            None,
                            WATCHER_PATH,
                            WATCHER,
                            "StatusNotifierHostRegistered",
                            None,
                        );
                    }
                    _ => {
                        invocation.return_dbus_error(
                            "org.freedesktop.DBus.Error.UnknownMethod",
                            "Unknown watcher method",
                        );
                        return;
                    }
                }
                invocation.return_value(Some(&().to_variant()));
            })
            .property(move |_, _, _, _, key| match key {
                "RegisteredStatusNotifierItems" => props
                    .items
                    .borrow()
                    .keys()
                    .cloned()
                    .collect::<Vec<_>>()
                    .to_variant(),
                "IsStatusNotifierHostRegistered" => true.to_variant(),
                _ => 0_i32.to_variant(),
            })
            .build()?;
        let acquired = inner.clone();
        let lost = inner.clone();
        let owner = gio::bus_own_name_on_connection(
            &bus,
            WATCHER,
            gio::BusNameOwnerFlags::NONE,
            move |_, _| {
                acquired.own_watcher.set(true);
                let _ = acquired.bus.emit_signal(
                    None,
                    WATCHER_PATH,
                    WATCHER,
                    "StatusNotifierHostRegistered",
                    None,
                );
            },
            move |_, _| {
                lost.own_watcher.set(false);
                register_external_host(&lost);
            },
        );
        let state = inner.clone();
        let owners = bus.subscribe_to_signal(
            Some("org.freedesktop.DBus"),
            Some("org.freedesktop.DBus"),
            Some("NameOwnerChanged"),
            Some("/org/freedesktop/DBus"),
            None,
            gio::DBusSignalFlags::NONE,
            move |signal| {
                if let Some((name, old, new)) = signal.parameters.get::<(String, String, String)>()
                    && !old.is_empty()
                    && new.is_empty()
                {
                    let addresses: Vec<_> = state
                        .items
                        .borrow()
                        .keys()
                        .filter(|a| address_parts(a).0 == name)
                        .cloned()
                        .collect();
                    for address in addresses {
                        state.items.borrow_mut().remove(&address);
                        state.revision.set(state.revision.get().wrapping_add(1));
                        if state.own_watcher.get() {
                            let _ = state.bus.emit_signal(
                                None,
                                WATCHER_PATH,
                                WATCHER,
                                "StatusNotifierItemUnregistered",
                                Some(&(address,).to_variant()),
                            );
                        }
                    }
                }
            },
        );
        let state = inner.clone();
        let watcher = bus.subscribe_to_signal(
            Some(WATCHER),
            Some(WATCHER),
            None,
            Some(WATCHER_PATH),
            None,
            gio::DBusSignalFlags::NONE,
            move |event| {
                if !state.own_watcher.get()
                    && let Some((address,)) = event.parameters.get::<(String,)>()
                {
                    if event.signal_name == "StatusNotifierItemRegistered" {
                        add_item(&state, &address);
                    }
                    if event.signal_name == "StatusNotifierItemUnregistered" {
                        state.items.borrow_mut().remove(&address);
                        state.revision.set(state.revision.get().wrapping_add(1));
                    }
                }
            },
        );
        Ok(Self {
            inner,
            _owner: owner,
            _registration: registration,
            _subscriptions: vec![owners, watcher],
        })
    }

    pub fn revision(&self) -> u64 {
        self.inner.revision.get()
    }

    pub fn refresh(&self) {
        let addresses: Vec<_> = self.inner.items.borrow().keys().cloned().collect();
        for address in addresses {
            refresh_item(&self.inner, address);
        }
        if !self.inner.own_watcher.get() {
            register_external_host(&self.inner);
        }
    }

    pub fn render(&self, container: &gtk::Box, view: &mut View) {
        let items = self.inner.items.borrow();
        view.buttons.retain(|address, view| {
            let keep = items
                .get(address)
                .is_some_and(|item| item.string("Status") != "Passive");
            if !keep {
                container.remove(&view.button);
            }
            keep
        });
        let mut previous: Option<gtk::Widget> = None;
        for item in items.values() {
            if item.string("Status") == "Passive" {
                continue;
            }
            if let Some(view) = view.buttons.get(&item.address) {
                update_button(&view.button, item, Some(&view.item.borrow()));
                *view.item.borrow_mut() = item.clone();
                container.reorder_child_after(&view.button, previous.as_ref());
                previous = Some(view.button.clone().upcast());
                continue;
            }
            let button = gtk::Button::new();
            update_button(&button, item, None);
            let current_item = Rc::new(RefCell::new(item.clone()));
            let bus = self.inner.bus.clone();
            let item_copy = current_item.clone();
            button.connect_clicked(move |button| {
                let item_copy = item_copy.borrow();
                if item_copy
                    .props
                    .get("ItemIsMenu")
                    .and_then(|v| v.get::<bool>())
                    .unwrap_or(false)
                {
                    show_menu(&bus, &item_copy, button);
                } else {
                    item_call(
                        &bus,
                        &item_copy.address,
                        "Activate",
                        &(0_i32, 0_i32).to_variant(),
                    );
                }
            });
            for mouse_button in [2, 3] {
                let click = gtk::GestureClick::new();
                click.set_button(mouse_button);
                let bus = self.inner.bus.clone();
                let item_copy = current_item.clone();
                let weak_button = button.downgrade();
                click.connect_pressed(move |gesture, _, _, _| {
                    let item_copy = item_copy.borrow();
                    if mouse_button == 2 {
                        item_call(
                            &bus,
                            &item_copy.address,
                            "SecondaryActivate",
                            &(0_i32, 0_i32).to_variant(),
                        );
                    } else if let Some(button) = weak_button.upgrade() {
                        show_menu(&bus, &item_copy, &button);
                    }
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                });
                button.add_controller(click);
            }
            let scroll =
                gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
            let bus = self.inner.bus.clone();
            let address = item.address.clone();
            scroll.connect_scroll(move |_, dx, dy| {
                let (amount, orientation) = if dy.abs() >= dx.abs() {
                    (-dy * 120., "vertical")
                } else {
                    (-dx * 120., "horizontal")
                };
                item_call(
                    &bus,
                    &address,
                    "Scroll",
                    &(amount as i32, orientation).to_variant(),
                );
                glib::Propagation::Stop
            });
            button.add_controller(scroll);
            button.connect_unmap(|button| {
                let mut child = button.first_child();
                while let Some(widget) = child {
                    child = widget.next_sibling();
                    if let Ok(popover) = widget.downcast::<gtk::Popover>() {
                        popover.popdown();
                        if popover.parent().is_some() {
                            popover.unparent();
                        }
                    }
                }
            });
            container.append(&button);
            container.reorder_child_after(&button, previous.as_ref());
            previous = Some(button.clone().upcast());
            view.buttons.insert(
                item.address.clone(),
                ItemView {
                    button,
                    item: current_item,
                },
            );
        }
    }
}

fn update_button(button: &gtk::Button, item: &TrayItem, previous: Option<&TrayItem>) {
    let icon_changed = previous.is_none_or(|previous| {
        [
            "Status",
            "IconName",
            "IconPixmap",
            "AttentionIconName",
            "AttentionIconPixmap",
            "IconThemePath",
        ]
        .iter()
        .any(|key| previous.props.get(*key) != item.props.get(*key))
    });
    if icon_changed {
        button.set_child(Some(&item_image(item)));
    }
    let title = if let Some(tooltip) = item.props.get("ToolTip")
        && tooltip.is_container()
        && tooltip.n_children() == 4
    {
        format!(
            "{}\n{}",
            tooltip.child_value(2).str().unwrap_or(""),
            tooltip.child_value(3).str().unwrap_or("")
        )
    } else {
        item.string("Title").into()
    };
    button.set_tooltip_text(Some(&title));
}

fn add_item(state: &Rc<Inner>, address: &str) {
    if !state.items.borrow().contains_key(address) {
        state.items.borrow_mut().insert(
            address.into(),
            TrayItem {
                address: address.into(),
                props: HashMap::new(),
            },
        );
        state.revision.set(state.revision.get().wrapping_add(1));
    }
    refresh_item(state, address.into());
}

fn refresh_item(state: &Rc<Inner>, address: String) {
    if !state.pending.borrow_mut().insert(address.clone()) {
        return;
    }
    let (name, path) = address_parts(&address);
    let name = name.to_owned();
    let path = path.to_owned();
    let state_copy = state.clone();
    state.bus.call(
        Some(&name),
        &path,
        "org.freedesktop.DBus.Properties",
        "GetAll",
        Some(&(ITEM,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
        move |result| {
            state_copy.pending.borrow_mut().remove(&address);
            if let Ok(reply) = result
                && let Some((props,)) = reply.get::<(HashMap<String, glib::Variant>,)>()
                && let Some(item) = state_copy.items.borrow_mut().get_mut(&address)
                && item.props != props
            {
                item.props = props;
                state_copy
                    .revision
                    .set(state_copy.revision.get().wrapping_add(1));
            }
        },
    );
}

fn register_external_host(state: &Rc<Inner>) {
    if let Some(name) = state.bus.unique_name() {
        state.bus.call(
            Some(WATCHER),
            WATCHER_PATH,
            WATCHER,
            "RegisterStatusNotifierHost",
            Some(&(name.as_str(),).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
            |_| {},
        );
    }
    let state_copy = state.clone();
    state.bus.call(
        Some(WATCHER),
        WATCHER_PATH,
        "org.freedesktop.DBus.Properties",
        "Get",
        Some(&(WATCHER, "RegisteredStatusNotifierItems").to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
        move |result| {
            if let Ok(reply) = result
                && let Some((value,)) = reply.get::<(glib::Variant,)>()
            {
                for address in value.get::<Vec<String>>().unwrap_or_default() {
                    add_item(&state_copy, &address);
                }
            }
        },
    );
}

fn item_call(bus: &gio::DBusConnection, address: &str, method: &str, params: &glib::Variant) {
    let (name, path) = address_parts(address);
    bus.call(
        Some(name),
        path,
        ITEM,
        method,
        Some(params),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
        |result| {
            if let Err(e) = result {
                eprintln!("Tray action: {e}");
            }
        },
    );
}

fn item_image(item: &TrayItem) -> gtk::Image {
    let attention = item.string("Status") == "NeedsAttention";
    let name = item.string(if attention {
        "AttentionIconName"
    } else {
        "IconName"
    });
    let image = gtk::Image::new();
    if !name.is_empty() {
        if std::path::Path::new(name).is_absolute() {
            image.set_from_file(Some(name));
        } else {
            let display = gtk::gdk::Display::default().unwrap();
            let theme = gtk::IconTheme::for_display(&display);
            let extra = item.string("IconThemePath");
            if !extra.is_empty()
                && !theme
                    .search_path()
                    .iter()
                    .any(|path| path == std::path::Path::new(extra))
            {
                theme.add_search_path(extra);
            }
            image.set_icon_name(Some(name));
        }
    } else if let Some(pixmaps) = item
        .props
        .get(if attention {
            "AttentionIconPixmap"
        } else {
            "IconPixmap"
        })
        .and_then(|v| v.get::<Vec<(i32, i32, Vec<u8>)>>())
    {
        if let Some((w, h, bytes)) = pixmaps
            .into_iter()
            .filter(|(w, h, bytes)| {
                *w > 0
                    && *h > 0
                    && *w <= 1024
                    && *h <= 1024
                    && bytes.len() == (*w as usize * *h as usize * 4)
            })
            .min_by_key(|(w, _, _)| (*w - 24).abs())
        {
            let rgba: Vec<u8> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[1], p[2], p[3], p[0]])
                .collect();
            let texture = gtk::gdk::MemoryTexture::new(
                w,
                h,
                gtk::gdk::MemoryFormat::R8g8b8a8,
                &glib::Bytes::from_owned(rgba),
                w as usize * 4,
            );
            image.set_paintable(Some(&texture));
        }
    } else {
        image.set_icon_name(Some("application-x-executable"));
    }
    image.set_pixel_size(24);
    image
}

fn show_menu(bus: &gio::DBusConnection, item: &TrayItem, button: &gtk::Button) {
    let path = item.string("Menu");
    if path.is_empty() || path == "/" {
        item_call(
            bus,
            &item.address,
            "ContextMenu",
            &(0_i32, 0_i32).to_variant(),
        );
        return;
    }
    let (name, _) = address_parts(&item.address);
    let bus_copy = bus.clone();
    let name_copy = name.to_owned();
    let path_copy = path.to_owned();
    let weak_button = button.downgrade();
    bus.call(
        Some(name),
        path,
        "com.canonical.dbusmenu",
        "AboutToShow",
        Some(&(0_i32,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        700,
        gio::Cancellable::NONE,
        move |_| load_menu(&bus_copy, &name_copy, &path_copy, weak_button),
    );
}

fn load_menu(
    bus: &gio::DBusConnection,
    name: &str,
    path: &str,
    weak_button: glib::WeakRef<gtk::Button>,
) {
    let bus_copy = bus.clone();
    let name_copy = name.to_owned();
    let path_copy = path.to_owned();
    bus.call(
        Some(name),
        path,
        "com.canonical.dbusmenu",
        "GetLayout",
        Some(&(0_i32, -1_i32, Vec::<String>::new()).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        1000,
        gio::Cancellable::NONE,
        move |result| {
            let Some(button) = weak_button.upgrade() else {
                return;
            };
            if !button.is_mapped() {
                return;
            }
            let Ok(reply) = result else {
                return;
            };
            if reply.n_children() != 2 {
                return;
            }
            let popover = gtk::Popover::new();
            popover.set_parent(&button);
            let rows = menu_rows(&bus_copy, &name_copy, &path_copy, &reply.child_value(1), 0);
            popover.set_child(Some(&rows));
            popover.connect_closed(|p| {
                if p.parent().is_some() {
                    p.unparent();
                }
            });
            popover.popup();
        },
    );
}

fn menu_rows(
    bus: &gio::DBusConnection,
    name: &str,
    path: &str,
    node: &glib::Variant,
    depth: u32,
) -> gtk::Box {
    let rows = gtk::Box::new(gtk::Orientation::Vertical, 2);
    if depth > 12 || node.n_children() != 3 {
        return rows;
    }
    let children = node.child_value(2);
    for index in 0..children.n_children().min(200) {
        let Some(child) = children.child_value(index).as_variant() else {
            continue;
        };
        if child.n_children() != 3 {
            continue;
        }
        let id = child.child_value(0).get::<i32>().unwrap_or(0);
        let props = child
            .child_value(1)
            .get::<HashMap<String, glib::Variant>>()
            .unwrap_or_default();
        if props.get("visible").and_then(|v| v.get::<bool>()) == Some(false) {
            continue;
        }
        if props.get("type").and_then(|v| v.str()) == Some("separator") {
            rows.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            continue;
        }
        let label = props
            .get("label")
            .and_then(|v| v.str())
            .unwrap_or("")
            .replace('_', "");
        if child.child_value(2).n_children() > 0 {
            let expander = gtk::Expander::new(Some(&label));
            expander.set_sensitive(
                props
                    .get("enabled")
                    .and_then(|v| v.get::<bool>())
                    .unwrap_or(true),
            );
            expander.set_child(Some(&menu_rows(bus, name, path, &child, depth + 1)));
            rows.append(&expander);
        } else {
            let label = if props.get("toggle-state").and_then(|v| v.get::<i32>()) == Some(1) {
                format!("[x] {label}")
            } else {
                label
            };
            let button = gtk::Button::with_label(&label);
            button.set_sensitive(
                props
                    .get("enabled")
                    .and_then(|v| v.get::<bool>())
                    .unwrap_or(true),
            );
            let bus = bus.clone();
            let name = name.to_owned();
            let path = path.to_owned();
            button.connect_clicked(move |button| {
                bus.call(
                    Some(&name),
                    &path,
                    "com.canonical.dbusmenu",
                    "Event",
                    Some(&(id, "clicked", 0_i32.to_variant(), 0_u32).to_variant()),
                    None,
                    gio::DBusCallFlags::NONE,
                    1000,
                    gio::Cancellable::NONE,
                    |_| {},
                );
                if let Some(popover) = button
                    .ancestor(gtk::Popover::static_type())
                    .and_downcast::<gtk::Popover>()
                {
                    popover.popdown();
                }
            });
            rows.append(&button);
        }
    }
    rows
}
