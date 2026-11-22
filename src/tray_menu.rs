//! Native DBusMenu presentation; no synchronous IPC or global coordinates.
use crate::{
    menu_model::{MenuRow, RowWire},
    tray_backend::{API, PATH},
};
use glib::variant::ToVariant;
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, LayerShell};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
type Fallback = Box<dyn Fn(&str, &gtk::Button)>;
struct Active {
    id: u64,
    key: String,
    anchor: glib::WeakRef<gtk::Button>,
    panel: glib::WeakRef<gtk::ApplicationWindow>,
    previous: KeyboardMode,
    popup: gtk::Popover,
    body: gtk::Box,
    token: Cell<u64>,
    revision: Cell<u32>,
    parent: Cell<i32>,
    loading: Cell<bool>,
    error: RefCell<String>,
    rows: RefCell<Vec<MenuRow>>,
    widgets: RefCell<Vec<(MenuRow, gtk::Button)>>,
    announced: Cell<Option<i32>>,
}
#[derive(serde::Serialize)]
pub struct RowInfo {
    pub id: i32,
    pub label: String,
    pub enabled: bool,
    pub toggle_type: String,
    pub toggle_state: i32,
    pub submenu: bool,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub hovered: bool,
    pub mapped: bool,
}
#[derive(serde::Serialize)]
pub struct Info {
    pub open: bool,
    pub key: String,
    pub parent: i32,
    pub token: u64,
    pub revision: u32,
    pub loading: bool,
    pub error: String,
    pub keyboard_grabbed: bool,
    pub rows: Vec<RowInfo>,
}
pub struct Manager {
    conn: RefCell<Option<gio::DBusConnection>>,
    active: RefCell<Option<Rc<Active>>>,
    last_panel: RefCell<glib::WeakRef<gtk::ApplicationWindow>>,
    serial: Rc<Cell<u64>>,
    last_error: Rc<RefCell<String>>,
    pending: Cell<bool>,
    queued: RefCell<Option<(u64, i32, bool)>>,
    fallback: RefCell<Option<Fallback>>,
}
impl Manager {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            conn: RefCell::new(None),
            active: RefCell::new(None),
            last_panel: RefCell::new(glib::WeakRef::new()),
            serial: Rc::new(Cell::new(0)),
            last_error: Rc::new(RefCell::new(String::new())),
            pending: Cell::new(false),
            queued: RefCell::new(None),
            fallback: RefCell::new(None),
        })
    }
    pub fn connect(&self, conn: &gio::DBusConnection) {
        *self.conn.borrow_mut() = Some(conn.clone());
    }
    pub fn fallback(&self, f: impl Fn(&str, &gtk::Button) + 'static) {
        *self.fallback.borrow_mut() = Some(Box::new(f));
    }
    fn current(&self, id: u64) -> Option<Rc<Active>> {
        self.active.borrow().clone().filter(|a| a.id == id)
    }
    pub fn close(&self) {
        self.serial.set(self.serial.get() + 1);
        self.queued.borrow_mut().take();
        let active = self.active.borrow_mut().take();
        if let Some(a) = active {
            if let Some(parent) = a.announced.get() {
                if parent != 0 {
                    self.event(&a, a.token.get(), parent, "closed");
                }
                self.event(&a, a.token.get(), 0, "closed");
            }
            if let Some(panel) = a.panel.upgrade() {
                panel.set_keyboard_mode(a.previous);
            }
            a.popup.popdown();
            a.popup.unparent();
        }
    }
    pub fn close_anchor(&self, button: &gtk::Button) {
        let matches = self
            .active
            .borrow()
            .as_ref()
            .and_then(|active| active.anchor.upgrade())
            .is_some_and(|anchor| anchor == *button);
        if matches {
            self.close();
        }
    }
    pub fn close_container(&self, container: &gtk::Box) {
        let matches = self
            .active
            .borrow()
            .as_ref()
            .and_then(|a| a.anchor.upgrade())
            .is_some_and(|b| b.is_ancestor(container));
        if matches {
            self.close();
        }
    }
    pub fn sync_visible(&self, keys: &[String]) {
        if self.active.borrow().as_ref().is_some_and(|a| {
            !keys.contains(&a.key) || a.anchor.upgrade().is_none_or(|b| !b.is_mapped())
        }) {
            self.close();
        }
    }
    pub fn changed(self: &Rc<Self>, key: &str) {
        let active = self.active.borrow().clone();
        if let Some(a) = active
            && a.key == key
        {
            self.load(a.id, a.parent.get(), false);
        }
    }
    pub fn open(self: &Rc<Self>, key: &str, anchor: &gtk::Button) {
        self.close();
        self.last_error.borrow_mut().clear();
        let Some(panel) = anchor.root().and_downcast::<gtk::ApplicationWindow>() else {
            return;
        };
        if !anchor.is_mapped() {
            return;
        }
        let popup = gtk::Popover::new();
        popup.add_css_class("mosaic-popup");
        popup.set_position(gtk::PositionType::Bottom);
        popup.set_autohide(true);
        popup.set_focusable(true);
        popup.set_parent(anchor);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let height = gtk::prelude::WidgetExt::display(&panel)
            .monitor_at_surface(&panel.surface().expect("mapped panel surface"))
            .map(|m| (m.geometry().height() - 48).clamp(80, 640))
            .unwrap_or(480);
        let width = gtk::prelude::WidgetExt::display(&panel)
            .monitor_at_surface(&panel.surface().expect("mapped panel surface"))
            .map(|m| (m.geometry().width() - 24).clamp(80, 280))
            .unwrap_or(280);
        let scroll = gtk::ScrolledWindow::builder()
            .width_request(width)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .min_content_width(240)
            .max_content_height(height)
            .propagate_natural_height(true)
            .child(&body)
            .build();
        popup.set_child(Some(&scroll));
        let id = self.serial.get();
        let previous = panel.keyboard_mode();
        *self.last_panel.borrow_mut() = panel.downgrade();
        let active = Rc::new(Active {
            id,
            key: key.into(),
            anchor: anchor.downgrade(),
            panel: panel.downgrade(),
            previous,
            popup: popup.clone(),
            body,
            token: Cell::new(0),
            revision: Cell::new(0),
            parent: Cell::new(0),
            loading: Cell::new(true),
            error: RefCell::new(String::new()),
            rows: RefCell::new(Vec::new()),
            widgets: RefCell::new(Vec::new()),
            announced: Cell::new(None),
        });
        *self.active.borrow_mut() = Some(active.clone());
        let weak = Rc::downgrade(self);
        popup.connect_closed(move |_| {
            if let Some(m) = weak.upgrade()
                && m.current(id).is_some()
            {
                m.close();
            }
        });
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, key, _, _| {
            let Some(m) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            m.key(key)
        });
        popup.add_controller(keys);
        self.message(&active, "Загрузка меню…");
        self.load(id, 0, true);
    }
    fn key(self: &Rc<Self>, key: gdk::Key) -> glib::Propagation {
        let Some(a) = self.active.borrow().clone() else {
            return glib::Propagation::Proceed;
        };
        match key {
            gdk::Key::Escape => {
                self.close();
            }
            gdk::Key::Left if a.parent.get() != 0 => {
                let parent = a
                    .rows
                    .borrow()
                    .iter()
                    .find(|r| r.id == a.parent.get())
                    .map(|r| r.parent)
                    .unwrap_or(0);
                a.parent.set(parent);
                if self.pending.get() {
                    self.load(a.id, parent, false);
                }
                self.render(&a);
            }
            gdk::Key::Up | gdk::Key::Down => {
                let widgets: Vec<_> = a
                    .widgets
                    .borrow()
                    .iter()
                    .filter(|(_, b)| b.is_sensitive())
                    .map(|(_, b)| b.clone())
                    .collect();
                if !widgets.is_empty() {
                    let focused = widgets.iter().position(|b| b.has_focus());
                    let next = match focused {
                        Some(i) if key == gdk::Key::Down => (i + 1) % widgets.len(),
                        Some(i) => (i + widgets.len() - 1) % widgets.len(),
                        None => 0,
                    };
                    widgets[next].grab_focus();
                }
            }
            gdk::Key::Right => {
                let selected = a
                    .widgets
                    .borrow()
                    .iter()
                    .find(|(r, b)| r.submenu && b.has_focus())
                    .map(|(r, _)| r.id);
                if let Some(id) = selected {
                    a.parent.set(id);
                    self.load(a.id, id, true);
                }
            }
            _ => return glib::Propagation::Proceed,
        }
        glib::Propagation::Stop
    }
    fn message(&self, a: &Active, text: &str) {
        while let Some(child) = a.body.first_child() {
            a.body.remove(&child);
        }
        a.widgets.borrow_mut().clear();
        let label = gtk::Label::new(Some(text));
        label.set_margin_top(10);
        label.set_margin_bottom(10);
        a.body.append(&label);
    }
    fn load(self: &Rc<Self>, id: u64, parent: i32, prepare: bool) {
        let Some(a) = self.active.borrow().clone().filter(|a| a.id == id) else {
            return;
        };
        a.loading.set(true);
        if self.pending.replace(true) {
            let mut queued = self.queued.borrow_mut();
            let prepare = prepare
                || queued
                    .as_ref()
                    .is_some_and(|(old_id, old_parent, old_prepare)| {
                        *old_id == id && *old_parent == parent && *old_prepare
                    });
            *queued = Some((id, parent, prepare));
            return;
        }
        let Some(conn) = self.conn.borrow().clone() else {
            self.pending.set(false);
            return;
        };
        let weak = Rc::downgrade(self);
        let key = a.key.clone();
        glib::MainContext::default().spawn_local(async move {
            let result = conn
                .call_future(
                    Some(API),
                    PATH,
                    API,
                    "GetMenu",
                    Some(&(key, parent, prepare).to_variant()),
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    4000,
                )
                .await;
            let Some(m) = weak.upgrade() else { return };
            m.pending.set(false);
            let current = m.current(id);
            let queued = m.queued.borrow_mut().take();
            if let Some(a) = current
                && queued.is_none()
            {
                a.loading.set(false);
                if let Ok(value) = result
                    && let Some((available, token, revision, rows)) =
                        value.get::<(bool, u64, u32, Vec<RowWire>)>()
                {
                    if !available {
                        let anchor = a.anchor.upgrade();
                        let key = a.key.clone();
                        m.close();
                        if let Some(anchor) = anchor
                            && anchor.is_mapped()
                            && let Some(f) = m.fallback.borrow().as_ref()
                        {
                            f(&key, &anchor);
                        }
                    } else {
                        a.token.set(token);
                        a.revision.set(revision);
                        *a.rows.borrow_mut() = rows.into_iter().map(MenuRow::from_wire).collect();
                        a.error.borrow_mut().clear();
                        m.render(&a);
                    }
                } else {
                    *a.error.borrow_mut() =
                        "Меню недоступно или изменилось во время загрузки".into();
                    a.rows.borrow_mut().clear();
                    m.message(&a, "Не удалось загрузить меню");
                    if a.anchor.upgrade().is_some_and(|b| b.is_mapped()) {
                        if let Some(panel) = a.panel.upgrade() {
                            panel.set_keyboard_mode(KeyboardMode::Exclusive);
                        }
                        a.popup.popup();
                        a.popup.grab_focus();
                    } else {
                        m.close();
                    }
                }
            }
            if let Some((id, parent, prepare)) = queued {
                m.load(id, parent, prepare);
            }
        });
    }
    fn render(self: &Rc<Self>, a: &Rc<Active>) {
        if a.anchor.upgrade().is_none_or(|b| !b.is_mapped()) {
            self.close();
            return;
        }
        if a.parent.get() != 0
            && !a
                .rows
                .borrow()
                .iter()
                .any(|r| r.id == a.parent.get() && r.enabled && r.visible && r.submenu)
        {
            a.parent.set(0);
        }
        if !a.popup.is_visible() {
            if let Some(panel) = a.panel.upgrade() {
                panel.set_keyboard_mode(KeyboardMode::Exclusive);
            }
            a.popup.popup();
        }
        let focus = a
            .widgets
            .borrow()
            .iter()
            .find(|(_, b)| b.has_focus())
            .map(|(r, _)| r.id);
        while let Some(child) = a.body.first_child() {
            a.body.remove(&child);
        }
        a.widgets.borrow_mut().clear();
        let parent = a.parent.get();
        if parent != 0 {
            let back = gtk::Button::with_label("‹ Назад");
            let weak = Rc::downgrade(self);
            let id = a.id;
            back.connect_clicked(move |_| {
                if let Some(m) = weak.upgrade()
                    && let Some(a) = m.current(id)
                {
                    let parent = a
                        .rows
                        .borrow()
                        .iter()
                        .find(|r| r.id == a.parent.get())
                        .map(|r| r.parent)
                        .unwrap_or(0);
                    a.parent.set(parent);
                    if m.pending.get() {
                        m.load(a.id, parent, false);
                    }
                    m.render(&a);
                }
            });
            a.body.append(&back);
        }
        for row in a
            .rows
            .borrow()
            .iter()
            .filter(|r| r.id != 0 && r.parent == parent && r.visible)
        {
            if row.kind == "separator" {
                a.body
                    .append(&gtk::Separator::new(gtk::Orientation::Horizontal));
                continue;
            }
            let role = match row.toggle_type.as_str() {
                "checkmark" => gtk::AccessibleRole::MenuItemCheckbox,
                "radio" => gtk::AccessibleRole::MenuItemRadio,
                _ => gtk::AccessibleRole::MenuItem,
            };
            let button = gtk::Button::builder()
                .accessible_role(role)
                .sensitive(row.enabled)
                .build();
            button.set_tooltip_text(Some(&row.label));
            button.update_property(&[gtk::accessible::Property::Label(&row.label)]);
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            if !row.toggle_type.is_empty() {
                let mark = if row.toggle_state == 1 {
                    if row.toggle_type == "radio" {
                        "●"
                    } else {
                        "✓"
                    }
                } else if row.toggle_state == 0 {
                    " "
                } else {
                    "−"
                };
                content.append(&gtk::Label::new(Some(mark)));
                button.update_state(&[gtk::accessible::State::Checked(match row.toggle_state {
                    1 => gtk::AccessibleTristate::True,
                    0 => gtk::AccessibleTristate::False,
                    _ => gtk::AccessibleTristate::Mixed,
                })]);
            }
            if !row.icon_name.is_empty()
                && gtk::IconTheme::for_display(&button.display()).has_icon(&row.icon_name)
            {
                let image = gtk::Image::from_icon_name(&row.icon_name);
                image.set_pixel_size(16);
                content.append(&image);
            }
            let label = gtk::Label::new(Some(&row.label));
            label.set_xalign(0.);
            label.set_hexpand(true);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(36);
            content.append(&label);
            if row.submenu {
                content.append(&gtk::Label::new(Some("›")));
            }
            button.set_child(Some(&content));
            let weak = Rc::downgrade(self);
            let id = a.id;
            let row = row.clone();
            let token = a.token.get();
            let r = row.clone();
            button.connect_clicked(move |_| {
                if let Some(m) = weak.upgrade()
                    && let Some(a) = m.current(id)
                {
                    if r.submenu {
                        if a.token.get() == token {
                            a.parent.set(r.id);
                            m.load(id, r.id, true);
                        }
                    } else {
                        m.event(&a, token, r.id, "clicked");
                        m.close();
                    }
                }
            });
            a.body.append(&button);
            a.widgets.borrow_mut().push((row, button));
        }
        if a.widgets.borrow().is_empty() {
            a.body
                .append(&gtk::Label::new(Some("Нет доступных пунктов")));
        }
        let previous = a.announced.replace(Some(parent));
        if previous != Some(parent) {
            if let Some(old) = previous
                && old != 0
            {
                self.event(a, a.token.get(), old, "closed");
            }
            if previous.is_none() {
                self.event(a, a.token.get(), 0, "opened");
            }
            if parent != 0 {
                self.event(a, a.token.get(), parent, "opened");
            }
        }
        let target = {
            let widgets = a.widgets.borrow();
            widgets
                .iter()
                .find(|(r, b)| Some(r.id) == focus && b.is_sensitive())
                .or_else(|| widgets.iter().find(|(_, b)| b.is_sensitive()))
                .map(|(_, b)| b.downgrade())
        };
        if let Some(target) = target {
            glib::idle_add_local_once(move || {
                if let Some(b) = target.upgrade()
                    && b.is_mapped()
                {
                    b.grab_focus();
                }
            });
        }
    }
    fn event(&self, a: &Active, token: u64, id: i32, event: &str) {
        if token == 0 {
            return;
        }
        let Some(conn) = self.conn.borrow().clone() else {
            return;
        };
        let args = (
            a.key.clone(),
            token,
            id,
            event,
            (glib::monotonic_time() / 1000) as u32,
        )
            .to_variant();
        let clicked = event == "clicked";
        let error = self.last_error.clone();
        let serial = self.serial.clone();
        let expected = a.id + 1;
        let anchor = a.anchor.clone();
        glib::MainContext::default().spawn_local(async move {
            if conn
                .call_future(
                    Some(API),
                    PATH,
                    API,
                    "MenuEvent",
                    Some(&args),
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    2500,
                )
                .await
                .is_err()
                && clicked
                && serial.get() == expected
            {
                *error.borrow_mut() = "Не удалось выполнить пункт меню".into();
                if let Some(b) = anchor.upgrade() {
                    b.set_tooltip_text(Some("Не удалось выполнить пункт меню"));
                }
            }
        });
    }
    pub fn info(&self) -> Info {
        let keyboard_grabbed = self
            .last_panel
            .borrow()
            .upgrade()
            .is_some_and(|w| w.keyboard_mode() != KeyboardMode::None);
        let mut info = Info {
            open: false,
            key: String::new(),
            parent: 0,
            token: 0,
            revision: 0,
            loading: false,
            error: self.last_error.borrow().clone(),
            keyboard_grabbed,
            rows: Vec::new(),
        };
        if let Some(a) = self.active.borrow().as_ref() {
            info.open = a.popup.is_visible();
            info.key = a.key.clone();
            info.parent = a.parent.get();
            info.token = a.token.get();
            info.revision = a.revision.get();
            info.loading = a.loading.get();
            info.error = a.error.borrow().clone();
            if let Some(surface) = a.popup.surface().and_downcast::<gdk::Popup>() {
                for (r, b) in a.widgets.borrow().iter() {
                    if let Some(rect) = b.compute_bounds(&a.popup) {
                        info.rows.push(RowInfo {
                            id: r.id,
                            label: r.label.clone(),
                            enabled: r.enabled,
                            toggle_type: r.toggle_type.clone(),
                            toggle_state: r.toggle_state,
                            submenu: r.submenu,
                            x: surface.position_x() as f32 + rect.x(),
                            y: surface.position_y() as f32 + rect.y(),
                            width: rect.width(),
                            height: rect.height(),
                            hovered: b.state_flags().contains(gtk::StateFlags::PRELIGHT),
                            mapped: b.is_mapped(),
                        });
                    }
                }
            }
        }
        info
    }
}
