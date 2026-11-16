use crate::{catalog::Catalog, dock, ipc, launcher, panel};
use gtk::{glib, prelude::*, Orientation};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use shell_core::Config;
use shell_wayland::{WaylandHandle, WindowCommand};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

pub struct Shell {
    pub app: gtk::Application,
    pub css_provider: gtk::CssProvider,
    pub config: RefCell<Config>,
    pub catalog: Catalog,
    pub wayland: WaylandHandle,
    pub overlay: RefCell<Option<gtk::Window>>,
    pub docks: RefCell<Vec<Rc<dock::Dock>>>,
    pub active_labels: RefCell<Vec<gtk::Label>>,
    pub notifications: std::sync::Arc<std::sync::Mutex<Result<String, String>>>,
}
pub fn start(app: &gtk::Application) -> anyhow::Result<()> {
    if !app.windows().is_empty() {
        return Ok(());
    }
    if !gtk4_layer_shell::is_supported() {
        anyhow::bail!("Композитор не поддерживает layer-shell; требуется Labwc/Wayland")
    }
    let config = Config::load()?;
    let catalog = Catalog::load();
    let wayland = WaylandHandle::start()?;
    let shell = Rc::new(Shell {
        app: app.clone(),
        css_provider: gtk::CssProvider::new(),
        config: RefCell::new(config),
        catalog,
        wayland,
        overlay: RefCell::new(None),
        docks: RefCell::new(vec![]),
        active_labels: RefCell::new(vec![]),
        notifications: std::sync::Arc::new(std::sync::Mutex::new(Err("Ожидание сервиса".into()))),
    });
    if let Some(display) = gtk::gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &shell.css_provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    shell.apply_theme();
    let display = gtk::gdk::Display::default().ok_or_else(|| anyhow::anyhow!("Нет GDK display"))?;
    let monitors = display.monitors();
    for i in 0..monitors.n_items() {
        if let Some(monitor) = monitors.item(i).and_downcast::<gtk::gdk::Monitor>() {
            panel::create(&shell, &monitor);
        }
    }
    if let Some(monitor) = monitors.item(0).and_downcast::<gtk::gdk::Monitor>() {
        let dock = dock::Dock::new(&shell, &monitor);
        shell.docks.borrow_mut().push(dock);
    }
    let rx = ipc::start();
    let s = shell.clone();
    let revision = Cell::new(u64::MAX);
    glib::timeout_add_local(Duration::from_millis(80), move || {
        for c in rx.try_iter() {
            match c {
                ipc::UiCommand::Launcher => s.show_launcher(false),
                ipc::UiCommand::Applications => s.show_launcher(true),
                ipc::UiCommand::Settings => s.settings(),
                ipc::UiCommand::Notifications => s.show_notifications(),
                ipc::UiCommand::Reload => s.reload(),
                ipc::UiCommand::ShowDock => {
                    for d in s.docks.borrow().iter() {
                        d.summon();
                    }
                }
            }
        }
        let snapshot = s.wayland.snapshot();
        if snapshot.revision != revision.get() {
            revision.set(snapshot.revision);
            for dock in s.docks.borrow().iter() {
                dock.rebuild(&s, &snapshot.windows);
            }
            let title = snapshot
                .windows
                .iter()
                .find(|w| w.active)
                .map(|w| {
                    s.catalog
                        .match_window(&w.app_id)
                        .map(|a| a.name.as_str())
                        .unwrap_or(&w.app_id)
                })
                .unwrap_or("Рабочий стол");
            for label in s.active_labels.borrow().iter() {
                label.set_text(title);
                label.set_tooltip_text(snapshot.error.as_deref())
            }
        }
        glib::ControlFlow::Continue
    });
    let shared = shell.notifications.clone();
    std::thread::spawn(move || loop {
        let result = (|| -> anyhow::Result<String> {
            let conn = zbus::blocking::Connection::session()?;
            let p = zbus::blocking::Proxy::new(
                &conn,
                "org.mosaic.Services1",
                "/org/mosaic/Services1",
                "org.mosaic.Services1",
            )?;
            Ok(p.call("GetNotifications", &())?)
        })();
        *shared.lock().unwrap() = result.map_err(|e| e.to_string());
        std::thread::sleep(Duration::from_secs(2));
    });
    Ok(())
}
impl Shell {
    pub fn show_launcher(self: &Rc<Self>, all: bool) {
        launcher::show(self, all)
    }
    pub fn window_action(&self, c: WindowCommand) {
        if let Err(e) = self.wayland.send(c) {
            self.message(&e.to_string())
        }
    }
    pub fn message(&self, text: &str) {
        let dialog = gtk::Window::builder()
            .application(&self.app)
            .title("Mosaic")
            .default_width(440)
            .build();
        let content = gtk::Box::new(Orientation::Vertical, 16);
        content.add_css_class("settings");
        let label = gtk::Label::new(Some(text));
        label.set_wrap(true);
        label.set_selectable(true);
        content.append(&label);
        let b = gtk::Button::with_label("Закрыть");
        let d = dialog.clone();
        b.connect_clicked(move |_| d.close());
        content.append(&b);
        dialog.set_child(Some(&content));
        dialog.present();
    }
    pub fn apply_theme(&self) {
        let provider = &self.css_provider;
        let config = self.config.borrow();
        let css = include_str!("../../../assets/style.css");
        let theme = if config.theme == "light" {
            "@define-color surface #f4f5f8; @define-color ink #242731; @define-color edge #c5c9d3;"
        } else {
            "@define-color surface #252934; @define-color ink #eff1f7; @define-color edge #444b5e;"
        };
        provider.load_from_string(&format!("{theme}\n{css}"));
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_application_prefer_dark_theme(config.theme == "dark");
            settings.set_gtk_enable_animations(!config.reduced_motion);
        }
    }
    pub fn toggle_pin(self: &Rc<Self>, id: &str) {
        let mut c = self.config.borrow().clone();
        if c.dock.pinned.iter().any(|p| p == id) {
            c.dock.pinned.retain(|p| p != id)
        } else {
            c.dock.pinned.push(id.to_string())
        }
        self.save(c)
    }
    fn save(self: &Rc<Self>, config: Config) {
        let result = self.config.borrow().save_changes(&config);
        match result {
            Ok(()) => {
                *self.config.borrow_mut() = config;
                self.apply_theme();
                self.refresh_dock()
            }
            Err(e) => self.message(&e.to_string()),
        }
    }
    fn refresh_dock(self: &Rc<Self>) {
        for dock in self.docks.borrow().iter() {
            dock.reconfigure(self);
        }
    }
    pub fn reload(self: &Rc<Self>) {
        match Config::load() {
            Ok(c) => {
                *self.config.borrow_mut() = c;
                self.apply_theme();
                self.refresh_dock()
            }
            Err(e) => self.message(&e.to_string()),
        }
    }
    pub fn settings(self: &Rc<Self>) {
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title("Mosaic — Настройки")
            .default_width(520)
            .default_height(360)
            .build();
        let box_ = gtk::Box::new(Orientation::Vertical, 16);
        box_.add_css_class("settings");
        let title = gtk::Label::new(Some("Оформление Mosaic"));
        title.add_css_class("title");
        box_.append(&title);
        let strict = self.config.borrow().mode == shell_core::ConfigMode::Strict;
        let label = gtk::Label::new(Some(if strict {
            "Настройки управляются Nix (strict)"
        } else {
            "Изменения сохраняются в пользовательские overrides"
        }));
        label.set_wrap(true);
        box_.append(&label);
        let dark = gtk::CheckButton::with_label("Тёмная тема");
        dark.set_active(self.config.borrow().theme == "dark");
        dark.set_sensitive(!strict);
        let s = self.clone();
        dark.connect_toggled(move |b| {
            let mut c = s.config.borrow().clone();
            c.theme = if b.is_active() { "dark" } else { "light" }.into();
            s.save(c)
        });
        box_.append(&dark);
        let reduced = gtk::CheckButton::with_label("Уменьшить анимацию");
        reduced.set_active(self.config.borrow().reduced_motion);
        reduced.set_sensitive(!strict);
        let s = self.clone();
        reduced.connect_toggled(move |b| {
            let mut c = s.config.borrow().clone();
            c.reduced_motion = b.is_active();
            s.save(c)
        });
        box_.append(&reduced);
        let label = gtk::Label::new(Some("Размер иконок Dock"));
        box_.append(&label);
        let size = gtk::Scale::with_range(Orientation::Horizontal, 32., 80., 4.);
        size.set_value(self.config.borrow().dock.icon_size as f64);
        size.set_draw_value(true);
        size.set_sensitive(!strict);
        let s = self.clone();
        size.connect_value_changed(move |b| {
            let mut c = s.config.borrow().clone();
            c.dock.icon_size = b.value() as i32;
            s.save(c)
        });
        box_.append(&size);
        let autohide = gtk::CheckButton::with_label("Автоматически скрывать Dock");
        autohide.set_active(self.config.borrow().dock.autohide);
        autohide.set_sensitive(!strict);
        let s = self.clone();
        autohide.connect_toggled(move |b| {
            let mut c = s.config.borrow().clone();
            c.dock.autohide = b.is_active();
            s.save(c);
        });
        box_.append(&autohide);
        let magnification = gtk::CheckButton::with_label("Увеличивать иконки при наведении");
        magnification.set_active(self.config.borrow().dock.magnification);
        magnification.set_sensitive(!strict);
        let s = self.clone();
        magnification.connect_toggled(move |b| {
            let mut c = s.config.borrow().clone();
            c.dock.magnification = b.is_active();
            s.save(c);
        });
        box_.append(&magnification);
        box_.append(&gtk::Label::new(Some("Увеличение иконок")));
        let factor = gtk::Scale::with_range(Orientation::Horizontal, 1.0, 2.0, 0.05);
        factor.set_value(self.config.borrow().dock.magnification_factor);
        factor.set_digits(2);
        factor.set_draw_value(true);
        factor.set_sensitive(!strict);
        let s = self.clone();
        factor.connect_value_changed(move |b| {
            let mut c = s.config.borrow().clone();
            c.dock.magnification_factor = b.value();
            s.save(c);
        });
        box_.append(&factor);
        let status = gtk::Label::new(Some(
            "Начальная версия: системные интеграции и расширенные эффекты — в разработке.",
        ));
        status.set_wrap(true);
        box_.append(&status);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&box_)
            .build();
        window.set_child(Some(&scroll));
        window.present();
    }
    pub fn show_notifications(self: &Rc<Self>) {
        if let Some(old) = self.overlay.borrow_mut().take() {
            old.close()
        }
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title("Уведомления")
            .default_width(440)
            .default_height(520)
            .build();
        window.init_layer_shell();
        window.set_layer(Layer::Overlay);
        window.set_anchor(Edge::Top, true);
        window.set_anchor(Edge::Right, true);
        window.set_margin(Edge::Top, 44);
        window.set_margin(Edge::Right, 12);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.set_namespace(Some("mosaic-notifications"));
        let content = gtk::Box::new(Orientation::Vertical, 12);
        content.add_css_class("settings");
        let title = gtk::Label::new(Some("Уведомления"));
        title.add_css_class("title");
        content.append(&title);
        match &*self.notifications.lock().unwrap() {
            Ok(json) => match serde_json::from_str::<Vec<serde_json::Value>>(json) {
                Ok(items) => {
                    if items.is_empty() {
                        content.append(&gtk::Label::new(Some("Нет уведомлений")))
                    }
                    for item in items.iter().rev().take(100) {
                        let app = item["app"].as_str().unwrap_or("");
                        let title = item["summary"].as_str().unwrap_or("");
                        let body = item["body"].as_str().unwrap_or("");
                        let l = gtk::Label::new(Some(&format!("{app}\n{title}\n{body}")));
                        l.set_wrap(true);
                        l.set_xalign(0.);
                        l.add_css_class("notification");
                        content.append(&l)
                    }
                }
                Err(e) => content.append(&gtk::Label::new(Some(&format!("Ошибка данных: {e}")))),
            },
            Err(_) => content.append(&gtk::Label::new(Some(
                "Сервис уведомлений недоступен. Запустите mosaic-services.",
            ))),
        }
        let close = gtk::Button::with_label("Закрыть");
        let w = window.clone();
        close.connect_clicked(move |_| w.close());
        content.append(&close);
        let scroll = gtk::ScrolledWindow::builder().child(&content).build();
        window.set_child(Some(&scroll));
        let key = gtk::EventControllerKey::new();
        let w = window.clone();
        key.connect_key_pressed(move |_, k, _, _| {
            if k == gtk::gdk::Key::Escape {
                w.close();
                gtk::glib::Propagation::Stop
            } else {
                gtk::glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
        window.present();
        *self.overlay.borrow_mut() = Some(window.upcast());
    }
}
