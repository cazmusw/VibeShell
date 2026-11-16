use crate::app::Shell;
use gtk::{prelude::*, Orientation};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::{rc::Rc, time::Duration};
pub fn create(shell: &Rc<Shell>, monitor: &gtk::gdk::Monitor) -> gtk::ApplicationWindow {
    let window = gtk::ApplicationWindow::builder()
        .application(&shell.app)
        .title("Mosaic Panel")
        .build();
    window.init_layer_shell();
    window.set_namespace(Some("mosaic-panel"));
    window.set_monitor(Some(monitor));
    window.set_layer(Layer::Top);
    window.set_keyboard_mode(KeyboardMode::None);
    for edge in [Edge::Top, Edge::Left, Edge::Right] {
        window.set_anchor(edge, true)
    }
    window.set_exclusive_zone(shell.config.borrow().panel.height);
    let row = gtk::Box::new(Orientation::Horizontal, 12);
    row.add_css_class("panel");
    row.set_height_request(shell.config.borrow().panel.height);
    let menu = gtk::MenuButton::builder().label("◈ Mosaic").build();
    let pop = gtk::Popover::new();
    let list = gtk::Box::new(Orientation::Vertical, 4);
    for (label, action) in [("Приложения", 0), ("Домашняя папка", 1), ("Настройки", 2)]
    {
        let b = gtk::Button::with_label(label);
        let s = shell.clone();
        let p = pop.clone();
        b.connect_clicked(move |_| {
            p.popdown();
            match action {
                0 => s.show_launcher(true),
                1 => {
                    if let Some(home) = std::env::var_os("HOME") {
                        let file = gtk::gio::File::for_path(home);
                        if let Err(e) = gtk::gio::AppInfo::launch_default_for_uri(
                            file.uri().as_str(),
                            None::<&gtk::gio::AppLaunchContext>,
                        ) {
                            s.message(&e.to_string())
                        }
                    }
                }
                _ => s.settings(),
            }
        });
        list.append(&b)
    }
    pop.set_child(Some(&list));
    menu.set_popover(Some(&pop));
    row.append(&menu);
    let active = gtk::Label::new(Some("Рабочий стол"));
    active.set_ellipsize(gtk::pango::EllipsizeMode::End);
    active.set_hexpand(true);
    active.set_xalign(0.);
    row.append(&active);
    shell.active_labels.borrow_mut().push(active);
    let clock = gtk::MenuButton::new();
    let calendar = gtk::Calendar::new();
    let pop = gtk::Popover::new();
    pop.set_child(Some(&calendar));
    clock.set_popover(Some(&pop));
    let weak_clock = clock.downgrade();
    let tick = move || {
        let Some(c) = weak_clock.upgrade() else { return gtk::glib::ControlFlow::Break };
        if let Ok(now) = gtk::glib::DateTime::now_local() {
            if let Ok(text) = now.format("%a, %d %b  %H:%M") {
                c.set_label(text.as_str())
            }
        }
        gtk::glib::ControlFlow::Continue
    };
    tick();
    gtk::glib::timeout_add_local(Duration::from_secs(10), tick);
    row.append(&clock);
    let notifications = gtk::Button::from_icon_name("preferences-system-notifications-symbolic");
    notifications.set_tooltip_text(Some("Центр уведомлений"));
    let s = shell.clone();
    notifications.connect_clicked(move |_| s.show_notifications());
    row.append(&notifications);
    window.set_child(Some(&row));
    window.present();
    window
}
