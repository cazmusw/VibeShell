use crate::{
    tray_backend::{self, API, PATH},
    tray_model::{TrayItem, Wire},
};
use glib::variant::ToVariant;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, LayerShell};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
struct Target {
    container: glib::WeakRef<gtk::Box>,
    inline: glib::WeakRef<gtk::Box>,
    hidden: glib::WeakRef<gtk::Box>,
    more: glib::WeakRef<gtk::MenuButton>,
    popup: glib::WeakRef<gtk::Popover>,
    monitor: gdk::Monitor,
    buttons: RefCell<Vec<Rendered>>,
    geometry_handler: Option<glib::SignalHandlerId>,
}
impl Drop for Target {
    fn drop(&mut self) {
        if let Some(handler) = self.geometry_handler.take() {
            self.monitor.disconnect(handler);
        }
    }
}
struct Rendered {
    button: gtk::Button,
    image: gtk::Image,
    label: gtk::Label,
    key: String,
    has_icon: bool,
    attention: bool,
    tooltip: String,
}
#[derive(serde::Serialize)]
pub struct ButtonInfo {
    connector: String,
    key: String,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    has_icon: bool,
    attention: bool,
    tooltip: String,
    hovered: bool,
    mapped: bool,
    overflow: bool,
}
#[derive(serde::Serialize)]
struct OverflowInfo {
    connector: String,
    hidden_count: usize,
    open: bool,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
}
#[derive(serde::Serialize)]
pub struct Info {
    available: bool,
    revision: u64,
    error: String,
    buttons: Vec<ButtonInfo>,
    overflow: Vec<OverflowInfo>,
    menu: crate::tray_menu::Info,
}
pub struct Tray {
    pub menus: Rc<crate::tray_menu::Manager>,
    conn: RefCell<Option<gio::DBusConnection>>,
    subscriptions: RefCell<Vec<gio::SignalSubscription>>,
    targets: RefCell<Vec<Target>>,
    items: RefCell<Vec<TrayItem>>,
    revision: Cell<u64>,
    generation: Cell<u64>,
    available: Cell<bool>,
    pending: Cell<bool>,
    again: Cell<bool>,
    error: RefCell<String>,
}
impl Tray {
    pub fn new() -> Rc<Self> {
        let tray = Rc::new(Self {
            menus: crate::tray_menu::Manager::new(),
            conn: RefCell::new(None),
            subscriptions: RefCell::new(Vec::new()),
            targets: RefCell::new(Vec::new()),
            items: RefCell::new(Vec::new()),
            revision: Cell::new(0),
            generation: Cell::new(0),
            available: Cell::new(false),
            pending: Cell::new(false),
            again: Cell::new(false),
            error: RefCell::new(String::new()),
        });
        let fallback = Rc::downgrade(&tray);
        tray.menus.fallback(move |key, button| {
            if let Some(t) = fallback.upgrade() {
                t.action(key, "context", 0, "", button);
            }
        });
        let weak = Rc::downgrade(&tray);
        glib::MainContext::default().spawn_local(async move {
            let Ok(conn) = gio::bus_get_future(gio::BusType::Session).await else {
                return;
            };
            let Some(tray) = weak.upgrade() else { return };
            let weak = Rc::downgrade(&tray);
            let changed = conn.subscribe_to_signal(
                Some(API),
                Some(API),
                Some("Changed"),
                Some(PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |_| {
                    if let Some(t) = weak.upgrade() {
                        t.refresh();
                    }
                },
            );
            let weak = Rc::downgrade(&tray);
            let owner = conn.subscribe_to_signal(
                Some("org.freedesktop.DBus"),
                Some("org.freedesktop.DBus"),
                Some("NameOwnerChanged"),
                Some("/org/freedesktop/DBus"),
                Some(API),
                gio::DBusSignalFlags::NONE,
                move |_| {
                    if let Some(t) = weak.upgrade() {
                        t.menus.close();
                        t.generation.set(t.generation.get() + 1);
                        t.items.borrow_mut().clear();
                        t.available.set(false);
                        t.render();
                        t.refresh();
                    }
                },
            );
            let weak = Rc::downgrade(&tray);
            let menus = conn.subscribe_to_signal(
                Some(API),
                Some(API),
                Some("MenuChanged"),
                Some(PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |signal| {
                    if let Some(t) = weak.upgrade()
                        && let Some((key,)) = signal.parameters.get::<(String,)>()
                    {
                        t.menus.changed(&key);
                    }
                },
            );
            tray.menus.connect(&conn);
            *tray.subscriptions.borrow_mut() = vec![changed, owner, menus];
            *tray.conn.borrow_mut() = Some(conn);
            tray.refresh();
        });
        tray
    }
    pub fn close_popovers(&self) {
        self.menus.close();
        let popovers: Vec<_> = self
            .targets
            .borrow()
            .iter()
            .filter_map(|target| target.popup.upgrade())
            .collect();
        for popover in popovers {
            popover.popdown();
        }
    }
    pub fn attach(self: &Rc<Self>, container: &gtk::Box, monitor: &gdk::Monitor) {
        let inline = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let hidden = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let scroll = gtk::ScrolledWindow::builder()
            .width_request(220)
            .max_content_height((monitor.geometry().height() - 64).clamp(96, 360))
            .propagate_natural_height(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&hidden)
            .build();
        let popup = gtk::Popover::builder()
            .position(gtk::PositionType::Bottom)
            .autohide(true)
            .child(&scroll)
            .build();
        popup.add_css_class("mosaic-popup");
        let more = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text("Ещё значки")
            .popover(&popup)
            .visible(false)
            .build();
        more.add_css_class("panel-tray-more");
        more.update_property(&[gtk::accessible::Property::Label("Ещё значки")]);
        popup.connect_map(|popup| {
            if let Some(panel) = popup.root().and_downcast::<gtk::ApplicationWindow>() {
                panel.set_keyboard_mode(KeyboardMode::Exclusive);
            }
        });
        let weak = Rc::downgrade(&self.menus);
        let hidden_ref = hidden.downgrade();
        popup.connect_unmap(move |popup| {
            if let Some(m) = weak.upgrade()
                && let Some(hidden) = hidden_ref.upgrade()
            {
                m.close_container(&hidden);
            }
            if let Some(panel) = popup.root().and_downcast::<gtk::ApplicationWindow>() {
                panel.set_keyboard_mode(KeyboardMode::None);
            }
        });
        let weak = Rc::downgrade(&self.menus);
        container.connect_unmap(move |container| {
            if let Some(m) = weak.upgrade() {
                m.close_container(container);
            }
        });
        container.append(&inline);
        container.append(&more);
        self.targets.borrow_mut().push(Target {
            container: container.downgrade(),
            inline: inline.downgrade(),
            hidden: hidden.downgrade(),
            more: more.downgrade(),
            popup: popup.downgrade(),
            monitor: monitor.clone(),
            buttons: RefCell::new(Vec::new()),
            geometry_handler: None,
        });
        let weak = Rc::downgrade(self);
        let scroll = scroll.downgrade();
        let handler = monitor.connect_geometry_notify(move |monitor| {
            if let Some(scroll) = scroll.upgrade() {
                scroll.set_max_content_height((monitor.geometry().height() - 64).clamp(96, 360));
                if let Some(tray) = weak.upgrade() {
                    tray.render();
                }
            }
        });
        if let Some(target) = self.targets.borrow_mut().last_mut() {
            target.geometry_handler = Some(handler);
        }
        self.render();
    }
    fn refresh(self: &Rc<Self>) {
        if self.pending.replace(true) {
            self.again.set(true);
            return;
        }
        let Some(conn) = self.conn.borrow().clone() else {
            self.pending.set(false);
            return;
        };
        let weak = Rc::downgrade(self);
        let generation = self.generation.get();
        glib::MainContext::default().spawn_local(async move {
            let result =
                tray_backend::call(&conn, API, PATH, API, "GetSnapshot", ().to_variant()).await;
            let Some(tray) = weak.upgrade() else { return };
            tray.pending.set(false);
            if generation == tray.generation.get() {
                if let Ok(value) = result
                    && value.size() <= 34_000_000
                    && let Some((revision, items)) = value.get::<(u64, Vec<Wire>)>()
                    && items.len() <= 128
                {
                    tray.available.set(true);
                    tray.revision.set(revision);
                    *tray.items.borrow_mut() = items.into_iter().map(TrayItem::from_wire).collect();
                    tray.error.borrow_mut().clear();
                } else {
                    tray.available.set(false);
                    tray.items.borrow_mut().clear();
                    *tray.error.borrow_mut() = "Трей недоступен".into();
                }
                tray.render();
            }
            if tray.again.replace(false) {
                tray.refresh();
            }
        });
    }
    fn action(
        self: &Rc<Self>,
        key: &str,
        action: &str,
        delta: i32,
        orientation: &str,
        button: &gtk::Button,
    ) {
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        let weak = Rc::downgrade(self);
        let button = button.downgrade();
        let args = (key, action, delta, orientation).to_variant();
        glib::MainContext::default().spawn_local(async move {
            if tray_backend::call(&conn, API, PATH, API, "Action", args)
                .await
                .is_err()
            {
                if let Some(t) = weak.upgrade() {
                    *t.error.borrow_mut() = "Действие трея не выполнено".into();
                }
                if let Some(b) = button.upgrade() {
                    b.set_tooltip_text(Some("Действие недоступно или приложение не отвечает"));
                }
            }
        });
    }
    fn create(self: &Rc<Self>, key: &str) -> Rendered {
        let button = gtk::Button::new();
        button.add_css_class("panel-button");
        button.add_css_class("tray-item");
        let weak = Rc::downgrade(&self.menus);
        button.connect_unmap(move |button| {
            if let Some(menus) = weak.upgrade() {
                menus.close_anchor(button);
            }
        });
        let image = gtk::Image::new();
        image.set_pixel_size(20);
        let label = gtk::Label::new(None);
        label.set_xalign(0.);
        label.set_max_width_chars(22);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        label.set_hexpand(true);
        label.set_visible(false);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        content.append(&image);
        content.append(&label);
        button.set_child(Some(&content));
        let weak = Rc::downgrade(self);
        let id = key.to_owned();
        button.connect_clicked(move |b| {
            if let Some(t) = weak.upgrade() {
                let menu = t
                    .items
                    .borrow()
                    .iter()
                    .find(|i| i.key == id)
                    .is_some_and(|i| i.item_is_menu);
                if menu {
                    t.menus.open(&id, b);
                } else {
                    t.action(&id, "activate", 0, "", b);
                }
            }
        });
        let gesture = gtk::GestureClick::new();
        gesture.set_button(0);
        gesture.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        let id = key.to_owned();
        let b = button.downgrade();
        gesture.connect_pressed(move |g, _, _, _| {
            let action = match g.current_button() {
                2 => "secondary",
                3 => "context",
                _ => return,
            };
            g.set_state(gtk::EventSequenceState::Claimed);
            if let Some(t) = weak.upgrade()
                && let Some(b) = b.upgrade()
            {
                if action == "context" {
                    t.menus.open(&id, &b);
                } else {
                    t.action(&id, action, 0, "", &b);
                }
            }
        });
        button.add_controller(gesture);
        let scroll = gtk::EventControllerScroll::new(
            gtk::EventControllerScrollFlags::BOTH_AXES | gtk::EventControllerScrollFlags::DISCRETE,
        );
        let weak = Rc::downgrade(self);
        let id = key.to_owned();
        let b = button.downgrade();
        scroll.connect_scroll(move |_, x, y| {
            let (v, axis) = if y.abs() >= x.abs() {
                (y, "vertical")
            } else {
                (x, "horizontal")
            };
            if let Some(t) = weak.upgrade()
                && let Some(b) = b.upgrade()
            {
                t.action(
                    &id,
                    "scroll",
                    (-v * 120.).round().clamp(-32768., 32768.) as i32,
                    axis,
                    &b,
                );
            }
            glib::Propagation::Stop
        });
        button.add_controller(scroll);
        Rendered {
            button,
            image,
            label,
            key: key.into(),
            has_icon: false,
            attention: false,
            tooltip: String::new(),
        }
    }
    fn render(self: &Rc<Self>) {
        let items: Vec<_> = self
            .items
            .borrow()
            .iter()
            .filter(|i| self.available.get() && i.status != "Passive")
            .cloned()
            .collect();
        let keys: Vec<_> = items.iter().map(|i| i.key.clone()).collect();
        self.menus.sync_visible(&keys);
        self.targets
            .borrow_mut()
            .retain(|t| t.container.upgrade().is_some());
        for target in self.targets.borrow().iter() {
            let (Some(inline), Some(hidden), Some(more), Some(popup)) = (
                target.inline.upgrade(),
                target.hidden.upgrade(),
                target.more.upgrade(),
                target.popup.upgrade(),
            ) else {
                continue;
            };
            let mut old: std::collections::HashMap<_, _> =
                std::mem::take(&mut *target.buttons.borrow_mut())
                    .into_iter()
                    .map(|r| (r.key.clone(), r))
                    .collect();
            // Remove disappeared items before replacing metadata on survivors.
            let removed: Vec<_> = old.keys().filter(|k| !keys.contains(k)).cloned().collect();
            for key in removed {
                if let Some(r) = old.remove(&key) {
                    remove_button(&r.button);
                }
            }
            if let Some(note) = inline
                .first_child()
                .filter(|widget| widget.is::<gtk::Label>())
            {
                inline.remove(&note);
            }
            if !self.available.get() {
                popup.popdown();
                more.set_visible(false);
                let note = gtk::Label::new(Some("?"));
                note.set_tooltip_text(Some("Системный трей недоступен"));
                inline.append(&note);
                continue;
            }
            let mut rendered = Vec::with_capacity(items.len());
            for item in &items {
                let mut r = old
                    .remove(&item.key)
                    .unwrap_or_else(|| self.create(&item.key));
                r.attention = item.status == "NeedsAttention";
                if r.attention {
                    r.button.add_css_class("attention");
                } else {
                    r.button.remove_css_class("attention");
                }
                r.tooltip = item.tooltip.clone();
                r.label.set_text(if item.title.is_empty() {
                    &item.tooltip
                } else {
                    &item.title
                });
                r.button.set_tooltip_text(Some(&item.tooltip));
                r.button
                    .update_property(&[gtk::accessible::Property::Label(&item.title)]);
                r.has_icon = false;
                if let Some(display) = gdk::Display::default()
                    && !item.icon_name.is_empty()
                    && gtk::IconTheme::for_display(&display).has_icon(&item.icon_name)
                {
                    r.image.set_icon_name(Some(&item.icon_name));
                    r.has_icon = true;
                } else if (1..=256).contains(&item.width)
                    && (1..=256).contains(&item.height)
                    && item.pixels.len() == item.width as usize * item.height as usize * 4
                {
                    let bytes = glib::Bytes::from_owned(item.pixels.clone());
                    let texture = gdk::MemoryTexture::new(
                        item.width,
                        item.height,
                        gdk::MemoryFormat::R8g8b8a8,
                        &bytes,
                        item.width as usize * 4,
                    );
                    r.image.set_paintable(Some(&texture));
                    r.has_icon = true;
                } else {
                    r.image
                        .set_icon_name(Some("application-x-executable-symbolic"));
                }
                rendered.push(r);
            }
            // Leave room for the shell menu, clocks, settings and panel spacing.
            let budget = (target.monitor.geometry().width() - 320).max(40);
            let widths: Vec<_> = rendered
                .iter()
                .map(|r| (r.image.measure(gtk::Orientation::Horizontal, -1).1 + 12).max(34) + 2)
                .collect();
            let visible = if widths.iter().sum::<i32>() <= budget {
                widths.len()
            } else {
                let mut remaining = budget - 40;
                widths
                    .iter()
                    .take_while(|width| {
                        remaining -= **width;
                        remaining >= 0
                    })
                    .count()
            };
            let mut after_inline: Option<gtk::Button> = None;
            let mut after_hidden: Option<gtk::Button> = None;
            for (index, row) in rendered.iter().enumerate() {
                let (parent, after) = if index < visible {
                    (&inline, &mut after_inline)
                } else {
                    (&hidden, &mut after_hidden)
                };
                row.label.set_visible(index >= visible);
                if row.button.parent().as_ref() != Some(parent.upcast_ref()) {
                    remove_button(&row.button);
                    parent.append(&row.button);
                }
                parent.reorder_child_after(&row.button, after.as_ref());
                *after = Some(row.button.clone());
            }
            if visible == rendered.len() {
                popup.popdown();
            }
            more.set_visible(visible < rendered.len());
            *target.buttons.borrow_mut() = rendered;
        }
    }
    pub fn info(&self) -> Info {
        let mut buttons = Vec::new();
        let mut overflow = Vec::new();
        for t in self.targets.borrow().iter() {
            let Some(hidden) = t.hidden.upgrade() else {
                continue;
            };
            let connector = t
                .monitor
                .connector()
                .map(|v| v.to_string())
                .unwrap_or_default();
            if let Some(more) = t.more.upgrade()
                && let Some(root) = more.root().and_downcast::<gtk::ApplicationWindow>()
                && let Some(rect) = more.compute_bounds(&root)
            {
                overflow.push(OverflowInfo {
                    connector: connector.clone(),
                    hidden_count: t
                        .buttons
                        .borrow()
                        .iter()
                        .filter(|b| b.button.is_ancestor(&hidden))
                        .count(),
                    open: t.popup.upgrade().is_some_and(|popup| popup.is_mapped()),
                    x: rect.x(),
                    y: rect.y(),
                    width: rect.width(),
                    height: rect.height(),
                });
            }
            for b in t.buttons.borrow().iter() {
                let in_overflow = b.button.is_ancestor(&hidden);
                let geometry = if in_overflow {
                    t.popup.upgrade().and_then(|popup| {
                        let surface = popup.surface().and_downcast::<gdk::Popup>()?;
                        let rect = b.button.compute_bounds(&popup)?;
                        Some((
                            rect.x() + surface.position_x() as f32,
                            rect.y() + surface.position_y() as f32,
                            rect.width(),
                            rect.height(),
                        ))
                    })
                } else {
                    b.button
                        .root()
                        .and_downcast::<gtk::ApplicationWindow>()
                        .and_then(|root| b.button.compute_bounds(&root))
                        .map(|rect| (rect.x(), rect.y(), rect.width(), rect.height()))
                };
                if let Some((x, y, width, height)) = geometry {
                    buttons.push(ButtonInfo {
                        connector: connector.clone(),
                        key: b.key.clone(),
                        x,
                        y,
                        width,
                        height,
                        overflow: in_overflow,
                        has_icon: b.has_icon,
                        attention: b.attention,
                        tooltip: b.tooltip.clone(),
                        hovered: b.button.state_flags().contains(gtk::StateFlags::PRELIGHT),
                        mapped: b.button.is_mapped(),
                    });
                }
            }
        }
        Info {
            available: self.available.get(),
            revision: self.revision.get(),
            error: self.error.borrow().clone(),
            buttons,
            overflow,
            menu: self.menus.info(),
        }
    }
}

fn remove_button(button: &gtk::Button) {
    if let Some(parent) = button.parent().and_downcast::<gtk::Box>() {
        parent.remove(button);
    }
}
