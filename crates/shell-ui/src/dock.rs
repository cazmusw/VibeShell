use crate::{app::Shell, catalog, dock_icon::DockIcon};
use gtk::{glib, prelude::*, Orientation};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use shell_core::{DockConfig, Window};
use shell_wayland::WindowCommand;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::{Rc, Weak},
    time::Duration,
};

pub struct Dock {
    shell: Weak<Shell>,
    needs_rebuild: Cell<bool>,
    window: gtk::ApplicationWindow,
    sensor: gtk::ApplicationWindow,
    monitor: gtk::gdk::Monitor,
    row: gtk::Box,
    viewport: gtk::ScrolledWindow,
    config: RefCell<DockConfig>,
    reduced_motion: Cell<bool>,
    hovered: Cell<bool>,
    sensor_hovered: Cell<bool>,
    keyboard_session: Cell<bool>,
    visible: Cell<bool>,
    pending: RefCell<Option<glib::SourceId>>,
    menus: RefCell<Vec<gtk::Popover>>,
    icons: RefCell<Vec<DockIcon>>,
}
impl Dock {
    pub fn new(shell: &Rc<Shell>, monitor: &gtk::gdk::Monitor) -> Rc<Self> {
        let window = gtk::ApplicationWindow::builder()
            .application(&shell.app)
            .title("Mosaic Dock")
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("mosaic-dock"));
        window.set_layer(Layer::Top);
        window.set_monitor(Some(monitor));
        window.set_anchor(Edge::Bottom, true);
        window.set_keyboard_mode(KeyboardMode::None);
        window.add_css_class("dock-window");
        let sensor = gtk::ApplicationWindow::builder()
            .application(&shell.app)
            .title("Mosaic Dock edge")
            .build();
        sensor.init_layer_shell();
        sensor.set_namespace(Some("mosaic-dock-edge"));
        sensor.set_layer(Layer::Top);
        sensor.set_monitor(Some(monitor));
        sensor.set_anchor(Edge::Bottom, true);
        sensor.set_keyboard_mode(KeyboardMode::None);
        sensor.set_exclusive_zone(-1);
        let sensor_area = gtk::Box::new(Orientation::Horizontal, 0);
        sensor_area.set_height_request(2);
        sensor.set_child(Some(&sensor_area));
        let row = gtk::Box::new(Orientation::Horizontal, 4);
        let viewport = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::External)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .propagate_natural_width(true)
            .propagate_natural_height(true)
            .child(&row)
            .build();
        let frame = gtk::Box::new(Orientation::Horizontal, 2);
        frame.add_css_class("dock");
        let left = gtk::Button::from_icon_name("pan-start-symbolic");
        let right = gtk::Button::from_icon_name("pan-end-symbolic");
        left.set_tooltip_text(Some("Прокрутить Dock влево"));
        right.set_tooltip_text(Some("Прокрутить Dock вправо"));
        left.add_css_class("dock-scroll");
        right.add_css_class("dock-scroll");
        frame.append(&left);
        frame.append(&viewport);
        frame.append(&right);
        window.set_child(Some(&frame));
        let this = Rc::new(Self {
            shell: Rc::downgrade(shell),
            needs_rebuild: Cell::new(false),
            window,
            sensor,
            monitor: monitor.clone(),
            row,
            viewport,
            config: RefCell::new(shell.config.borrow().dock.clone()),
            reduced_motion: Cell::new(shell.config.borrow().reduced_motion),
            hovered: Cell::new(false),
            sensor_hovered: Cell::new(false),
            keyboard_session: Cell::new(false),
            visible: Cell::new(true),
            pending: RefCell::new(None),
            menus: RefCell::new(vec![]),
            icons: RefCell::new(vec![]),
        });
        for (button, direction) in [(&left, -1.0), (&right, 1.0)] {
            let weak = Rc::downgrade(&this);
            button.connect_clicked(move |_| {
                if let Some(d) = weak.upgrade() {
                    d.scroll(direction * d.viewport.hadjustment().page_size().max(100.0) * 0.7);
                }
            });
        }
        let adjustment = this.viewport.hadjustment();
        let update = Rc::new(move |a: &gtk::Adjustment| {
            let overflow = a.upper() > a.page_size() + 1.0;
            left.set_visible(overflow);
            right.set_visible(overflow);
            left.set_sensitive(a.value() > a.lower() + 0.5);
            right.set_sensitive(a.value() < a.upper() - a.page_size() - 0.5);
        });
        for name in ["upper", "page-size", "value"] {
            let update = update.clone();
            adjustment.connect_notify_local(Some(name), move |a, _| update(a));
        }
        update(&adjustment);
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::BOTH_AXES);
        let weak = Rc::downgrade(&this);
        scroll.connect_scroll(move |_, dx, dy| {
            if let Some(d) = weak.upgrade() {
                let a = d.viewport.hadjustment();
                if a.upper() > a.page_size() + 1.0 {
                    d.scroll(if dx.abs() > dy.abs() { dx } else { dy } * 48.0);
                    return glib::Propagation::Stop;
                }
            }
            glib::Propagation::Proceed
        });
        this.viewport.add_controller(scroll);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&this);
        motion.connect_enter(move |_, _, _| {
            if let Some(d) = weak.upgrade() {
                d.hovered.set(true);
                d.show_now();
            }
        });
        let weak = Rc::downgrade(&this);
        motion.connect_leave(move |_| {
            if let Some(d) = weak.upgrade() {
                d.hovered.set(false);
                d.reset_icons();
                d.schedule_hide();
            }
        });
        this.window.add_controller(motion);
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&this);
        motion.connect_motion(move |_, x, _| {
            if let Some(d) = weak.upgrade() {
                d.magnify(x);
            }
        });
        this.row.add_controller(motion);
        let edge = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(&this);
        edge.connect_enter(move |_, _, _| {
            if let Some(d) = weak.upgrade() {
                d.sensor_hovered.set(true);
                d.schedule_show();
            }
        });
        let weak = Rc::downgrade(&this);
        edge.connect_leave(move |_| {
            if let Some(d) = weak.upgrade() {
                d.sensor_hovered.set(false);
                d.cancel_pending();
                d.schedule_hide();
            }
        });
        this.sensor.add_controller(edge);
        let key = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(&this);
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                if let Some(d) = weak.upgrade() {
                    d.finish_keyboard();
                    d.schedule_hide();
                }
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        this.window.add_controller(key);
        this.rebuild(shell, &[]);
        this.window.present();
        this.schedule_hide();
        this
    }
    fn cancel_pending(&self) {
        if let Some(source) = self.pending.borrow_mut().take() {
            source.remove();
        }
    }
    fn show_now(&self) {
        self.cancel_pending();
        self.visible.set(true);
        self.window.set_visible(true);
    }
    fn pinned_open(&self) -> bool {
        self.hovered.get()
            || self.sensor_hovered.get()
            || self.keyboard_session.get()
            || !self.menus.borrow().is_empty()
    }
    fn schedule_show(self: &Rc<Self>) {
        self.cancel_pending();
        if self.visible.get() {
            return;
        }
        let weak = Rc::downgrade(self);
        let delay = self.config.borrow().show_delay_ms;
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(delay),
            move || {
                if let Some(d) = weak.upgrade() {
                    d.pending.borrow_mut().take();
                    if d.sensor_hovered.get() {
                        d.show_now();
                    }
                }
            },
        ));
    }
    fn schedule_hide(self: &Rc<Self>) {
        self.cancel_pending();
        if !self.config.borrow().autohide || !self.visible.get() || self.pinned_open() {
            return;
        }
        let weak = Rc::downgrade(self);
        let delay = self.config.borrow().hide_delay_ms;
        *self.pending.borrow_mut() = Some(glib::timeout_add_local_once(
            Duration::from_millis(delay),
            move || {
                if let Some(d) = weak.upgrade() {
                    d.pending.borrow_mut().take();
                    if d.config.borrow().autohide && !d.pinned_open() {
                        d.visible.set(false);
                        d.window.set_visible(false);
                    }
                }
            },
        ));
    }
    pub fn summon(&self) {
        self.show_now();
        self.keyboard_session.set(true);
        self.window.set_layer(Layer::Overlay);
        self.window.set_keyboard_mode(KeyboardMode::Exclusive);
        if let Some(first) = self.row.first_child() {
            first.grab_focus();
        }
    }
    fn finish_keyboard(&self) {
        self.keyboard_session.set(false);
        self.window.set_keyboard_mode(KeyboardMode::None);
        self.window.set_layer(Layer::Top);
    }
    fn scroll(&self, delta: f64) {
        let a = self.viewport.hadjustment();
        a.set_value(
            (a.value() + delta).clamp(a.lower(), (a.upper() - a.page_size()).max(a.lower())),
        );
    }
    fn reset_icons(&self) {
        for icon in self.icons.borrow().iter() {
            icon.animate_to(1.0, self.reduced_motion.get());
        }
    }
    fn magnify(&self, x: f64) {
        let config = self.config.borrow();
        let enabled = config.magnification && !self.reduced_motion.get();
        let radius = config.icon_size as f64 * 2.5;
        for icon in self.icons.borrow().iter() {
            let Some(bounds) = icon.compute_bounds(&self.row) else {
                continue;
            };
            let distance = (x - bounds.center().x() as f64).abs();
            let influence = if enabled && distance < radius {
                (distance / radius * std::f64::consts::FRAC_PI_2)
                    .cos()
                    .powi(2)
            } else {
                0.0
            };
            icon.animate_to(
                1.0 + (config.magnification_factor - 1.0) * influence,
                !enabled,
            );
        }
    }
    pub fn reconfigure(self: &Rc<Self>, shell: &Rc<Shell>) {
        self.cancel_pending();
        *self.config.borrow_mut() = shell.config.borrow().dock.clone();
        self.reduced_motion
            .set(shell.config.borrow().reduced_motion);
        self.rebuild(shell, &shell.wayland.snapshot().windows);
        if !self.config.borrow().autohide {
            self.show_now();
        } else {
            self.schedule_hide();
        }
    }
    pub fn rebuild(self: &Rc<Self>, shell: &Rc<Shell>, windows: &[Window]) {
        // Keep an open menu stable while Wayland updates window titles/states.
        // Commands remain protected by epoch/id in the backend.
        if !self.menus.borrow().is_empty() {
            self.needs_rebuild.set(true);
            return;
        }
        self.needs_rebuild.set(false);
        self.icons.borrow_mut().clear();
        while let Some(child) = self.row.first_child() {
            self.row.remove(&child);
        }
        let config = self.config.borrow().clone();
        let maximum = if config.magnification && !self.reduced_motion.get() {
            config.magnification_factor
        } else {
            1.0
        };
        let launcher = gtk::Button::new();
        launcher.add_css_class("dock-icon");
        launcher.set_tooltip_text(Some("Приложения"));
        launcher.update_property(&[gtk::accessible::Property::Label("Приложения")]);
        let image = gtk::Image::from_icon_name("view-app-grid-symbolic");
        image.set_pixel_size(config.icon_size);
        let icon = DockIcon::new(image, config.icon_size, maximum);
        self.icons.borrow_mut().push(icon.clone());
        launcher.set_child(Some(&icon));
        let weak = Rc::downgrade(self);
        let s = shell.clone();
        launcher.connect_clicked(move |_| {
            if let Some(d) = weak.upgrade() {
                d.finish_keyboard();
            }
            s.show_launcher(true);
            if let Some(d) = weak.upgrade() {
                d.schedule_hide();
            }
        });
        self.row.append(&launcher);
        self.row.append(&gtk::Separator::new(Orientation::Vertical));
        let mut ids = config.pinned.clone();
        let mut seen: BTreeSet<_> = ids.iter().cloned().collect();
        for w in windows {
            let id = shell
                .catalog
                .match_window(&w.app_id)
                .map(|a| a.id.clone())
                .unwrap_or_else(|| format!("window:{}", w.app_id));
            if seen.insert(id.clone()) {
                ids.push(id);
            }
        }
        let item_count = ids.len() + 1;
        for id in ids {
            let app = shell.catalog.get(&id);
            let matches: Vec<_> = windows
                .iter()
                .filter(|w| {
                    shell
                        .catalog
                        .match_window(&w.app_id)
                        .map(|a| a.id == id)
                        .unwrap_or_else(|| id == format!("window:{}", w.app_id))
                })
                .cloned()
                .collect();
            let title = app
                .map(|a| a.name.clone())
                .or_else(|| matches.first().map(|w| w.app_id.clone()))
                .unwrap_or_else(|| id.clone());
            let button = gtk::Button::new();
            button.add_css_class("dock-icon");
            button.set_tooltip_text(Some(&title));
            button.update_property(&[gtk::accessible::Property::Label(&title)]);
            let content = gtk::Box::new(Orientation::Vertical, 2);
            let icon = DockIcon::new(
                catalog::icon(app, config.icon_size),
                config.icon_size,
                maximum,
            );
            self.icons.borrow_mut().push(icon.clone());
            content.append(&icon);
            let indicator = gtk::Label::new(Some(if matches.is_empty() { " " } else { "•" }));
            indicator.add_css_class("indicator");
            content.append(&indicator);
            button.set_child(Some(&content));
            if matches.iter().any(|w| w.active) {
                button.add_css_class("active");
            }
            let s = shell.clone();
            let launch_id = id.clone();
            let click_windows = matches.clone();
            let weak = Rc::downgrade(self);
            button.connect_clicked(move |b| {
                let Some(d) = weak.upgrade() else { return };
                match click_windows.as_slice() {
                    [] => {
                        d.finish_keyboard();
                        if let Err(e) = s.catalog.launch(&launch_id) {
                            s.message(&e.to_string());
                        }
                    }
                    [w] => {
                        d.finish_keyboard();
                        s.window_action(WindowCommand::Activate {
                            epoch: w.epoch,
                            id: w.id,
                        });
                    }
                    _ => show_menu(&s, &d, b, &launch_id, &click_windows),
                }
                d.schedule_hide();
            });
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3);
            let weak_button = button.downgrade();
            let s = shell.clone();
            let weak = Rc::downgrade(self);
            gesture.connect_pressed(move |_, _, _, _| {
                if let (Some(d), Some(button)) = (weak.upgrade(), weak_button.upgrade()) {
                    show_menu(&s, &d, &button, &id, &matches);
                }
            });
            button.add_controller(gesture);
            self.row.append(&button);
        }
        let cap = ((self.monitor.geometry().width() as f64 * config.max_width_ratio) as i32 - 100)
            .max(64);
        self.viewport.set_max_content_width(cap);
        self.viewport.set_min_content_width(cap.min(64));
        let width = (((config.icon_size as f64 * maximum).ceil() as i32 + 20) * item_count as i32
            + 30)
            .min(cap + 100);
        self.sensor.set_default_size(width, 2);
        self.window.set_margin(Edge::Bottom, config.margin);
        let height = (config.icon_size as f64 * maximum).ceil() as i32 + 40;
        self.viewport.set_min_content_height(height);
        self.window.set_exclusive_zone(if config.autohide {
            0
        } else {
            height + config.margin + 20
        });
        self.sensor.set_visible(config.autohide);
    }
    fn register_menu(self: &Rc<Self>, pop: &gtk::Popover) {
        self.cancel_pending();
        self.menus.borrow_mut().push(pop.clone());
        let weak: Weak<Self> = Rc::downgrade(self);
        pop.connect_closed(move |pop| {
            // Detach before rebuilding: the rebuild can dispose the parent button.
            pop.set_child(None::<&gtk::Widget>);
            if pop.parent().is_some() {
                pop.unparent();
            }
            if let Some(d) = weak.upgrade() {
                d.menus.borrow_mut().retain(|p| p != pop);
                d.finish_keyboard();
                if d.menus.borrow().is_empty() && d.needs_rebuild.replace(false) {
                    if let Some(shell) = d.shell.upgrade() {
                        d.rebuild(&shell, &shell.wayland.snapshot().windows);
                    }
                }
                d.schedule_hide();
            }
        });
    }
}

fn show_menu(
    shell: &Rc<Shell>,
    dock: &Rc<Dock>,
    button: &gtk::Button,
    id: &str,
    windows: &[Window],
) {
    let pop = gtk::Popover::new();
    pop.set_parent(button);
    let list = gtk::Box::new(Orientation::Vertical, 4);
    for w in windows {
        let b = gtk::Button::with_label(if w.title.is_empty() {
            &w.app_id
        } else {
            &w.title
        });
        let s = shell.clone();
        let w = w.clone();
        let p = pop.clone();
        b.connect_clicked(move |_| {
            s.window_action(WindowCommand::Activate {
                epoch: w.epoch,
                id: w.id,
            });
            p.popdown()
        });
        list.append(&b);
    }
    if shell.catalog.get(id).is_some() {
        let b = gtk::Button::with_label("Запустить ещё раз");
        let s = shell.clone();
        let launch_id = id.to_string();
        let p = pop.clone();
        b.connect_clicked(move |_| {
            if let Err(e) = s.catalog.launch(&launch_id) {
                s.message(&e.to_string())
            }
            p.popdown()
        });
        list.append(&b);
        let pinned = shell.config.borrow().dock.pinned.iter().any(|p| p == id);
        let b = gtk::Button::with_label(if pinned {
            "Открепить"
        } else {
            "Закрепить"
        });
        b.set_sensitive(shell.config.borrow().mode != shell_core::ConfigMode::Strict);
        let s = shell.clone();
        let id = id.to_string();
        let p = pop.clone();
        b.connect_clicked(move |_| {
            s.toggle_pin(&id);
            p.popdown()
        });
        list.append(&b);
    }
    for w in windows {
        for (label, command) in [
            (
                "Свернуть",
                WindowCommand::Minimize {
                    epoch: w.epoch,
                    id: w.id,
                },
            ),
            (
                "Закрыть окно",
                WindowCommand::Close {
                    epoch: w.epoch,
                    id: w.id,
                },
            ),
        ] {
            let b = gtk::Button::with_label(&format!("{label}: {}", w.title));
            let s = shell.clone();
            let p = pop.clone();
            b.connect_clicked(move |_| {
                s.window_action(command);
                p.popdown()
            });
            list.append(&b);
        }
    }
    pop.set_child(Some(&list));
    dock.register_menu(&pop);
    pop.popup();
}
