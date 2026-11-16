use crate::{app::Shell, catalog};
use gtk::{prelude::*, Orientation};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use std::rc::Rc;
pub fn show(shell: &Rc<Shell>, all: bool) {
    if let Some(old) = shell.overlay.borrow_mut().take() {
        old.close()
    }
    let window = gtk::ApplicationWindow::builder()
        .application(&shell.app)
        .title("Mosaic — Приложения")
        .default_width(600)
        .default_height(480)
        .build();
    window.init_layer_shell();
    window.set_layer(Layer::Overlay);
    window.set_namespace(Some("mosaic-launcher"));
    window.set_keyboard_mode(KeyboardMode::Exclusive);
    let outer = gtk::Box::new(Orientation::Vertical, 12);
    outer.add_css_class("launcher");
    let entry = gtk::SearchEntry::new();
    entry.set_placeholder_text(Some("Поиск приложений…"));
    outer.append(&entry);
    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("results");
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&list)
        .build();
    outer.append(&scroll);
    window.set_child(Some(&outer));
    let catalog = shell.catalog.clone();
    let s = shell.clone();
    let w = window.clone();
    let results_list = list.clone();
    let rebuild = Rc::new(move |query: &str| {
        let list = &results_list;
        while let Some(c) = list.first_child() {
            list.remove(&c)
        }
        let mut results: Vec<_> = catalog
            .apps
            .iter()
            .filter_map(|a| {
                shell_core::search_score(query, &a.name)
                    .or_else(|| shell_core::search_score(query, &a.id))
                    .map(|score| (score, a))
            })
            .collect();
        results.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.name.cmp(&b.1.name)));
        for (_, a) in results.into_iter().take(if all { 500 } else { 30 }) {
            let row = gtk::ListBoxRow::new();
            row.set_widget_name(&a.id);
            let content = gtk::Box::new(Orientation::Horizontal, 12);
            content.set_margin_top(6);
            content.set_margin_bottom(6);
            content.append(&catalog::icon(Some(a), 32));
            content.append(&gtk::Label::new(Some(&a.name)));
            row.set_child(Some(&content));
            list.append(&row);
        }
        if let Some(row) = list.row_at_index(0) {
            list.select_row(Some(&row))
        }
    });
    list.connect_row_activated(move |_, row| {
        if let Err(e) = s.catalog.launch(row.widget_name().as_str()) {
            s.message(&e.to_string())
        }
        w.close()
    });
    let r = rebuild.clone();
    entry.connect_search_changed(move |e| r(e.text().as_str()));
    let l = list.clone();
    entry.connect_activate(move |_| {
        if let Some(row) = l.selected_row() {
            l.emit_by_name::<()>("row-activated", &[&row]);
        }
    });
    let l = list.clone();
    let key = gtk::EventControllerKey::new();
    let w = window.clone();
    key.connect_key_pressed(move |_, key, _, _| {
        if key == gtk::gdk::Key::Escape {
            w.close();
            return gtk::glib::Propagation::Stop;
        }
        if key == gtk::gdk::Key::Down {
            l.grab_focus();
            return gtk::glib::Propagation::Stop;
        }
        gtk::glib::Propagation::Proceed
    });
    window.add_controller(key);
    rebuild("");
    window.present();
    entry.grab_focus();
    *shell.overlay.borrow_mut() = Some(window.upcast());
}
