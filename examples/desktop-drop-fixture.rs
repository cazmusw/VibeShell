use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{Edge, Layer, LayerShell};
use std::path::PathBuf;

fn main() {
    let runtime = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").expect("missing runtime"));
    assert!(!runtime.starts_with("/run/user"));
    assert!(
        runtime
            .components()
            .any(|part| part.as_os_str().to_string_lossy().starts_with("mosaic-"))
    );
    let files: Vec<_> = std::env::args_os()
        .skip(1)
        .map(gio::File::for_path)
        .collect();
    assert!(!files.is_empty());
    let mut uris = String::new();
    for file in files {
        uris.push_str(&file.uri());
        uris.push_str("\r\n");
    }
    gtk::init().expect("cannot initialize fixture");
    let window = gtk::Window::builder()
        .default_width(180)
        .default_height(72)
        .decorated(false)
        .build();
    window.init_layer_shell();
    window.set_layer(Layer::Top);
    window.set_anchor(Edge::Left, true);
    window.set_anchor(Edge::Top, true);
    window.set_margin(Edge::Left, 150);
    window.set_margin(Edge::Top, 96);
    let handle = gtk::Button::with_label("Mosaic drag fixture");
    window.set_child(Some(&handle));
    let source = gtk::DragSource::new();
    source.set_actions(gdk::DragAction::COPY | gdk::DragAction::MOVE);
    source.set_content(Some(&gdk::ContentProvider::new_union(&[
        gdk::ContentProvider::for_bytes(
            "text/uri-list",
            &glib::Bytes::from_owned(uris.into_bytes()),
        ),
        gdk::ContentProvider::for_value(&"mosaic-pin:mosaic-stage-0-probe.desktop".to_value()),
    ])));
    source.connect_drag_begin(|_, drag| {
        println!("drag");
        drag.connect_dnd_finished(|_| println!("dnd-finished"));
        drag.connect_cancel(|_, _| println!("dnd-cancelled"));
    });
    source.connect_drag_end(|_, drag, _| {
        println!(
            "finished-copy={}",
            drag.selected_action() == gdk::DragAction::COPY
        );
    });
    handle.add_controller(source);
    window.connect_map(|_| println!("mapped"));
    window.present();
    glib::MainLoop::new(None, false).run();
}
