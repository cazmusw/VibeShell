use crate::filesystem::{display_location, location_components};
use crate::locale::tr;
use gtk::{gdk, gio, glib, pango, prelude::*};

pub struct AddressBar {
    pub widget: gtk::Box,
    pub entry: gtk::Entry,
    pub edit: gtk::Button,
    stack: gtk::Stack,
    scroller: gtk::ScrolledWindow,
    segments: gtk::Box,
}

impl AddressBar {
    pub fn new() -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 3);
        widget.add_css_class("files-address");
        widget.set_hexpand(true);
        let stack = gtk::Stack::new();
        stack.set_hexpand(true);
        stack.set_hhomogeneous(false);
        let segments = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        let scroller = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Never)
            .overlay_scrolling(true)
            .child(&segments)
            .build();
        stack.add_named(&scroller, Some("segments"));

        let entry = gtk::Entry::builder()
            .placeholder_text(tr("Путь к папке"))
            .width_chars(10)
            .hexpand(true)
            .build();
        entry.set_icon_from_icon_name(gtk::EntryIconPosition::Primary, Some("folder-symbolic"));
        entry.set_icon_activatable(gtk::EntryIconPosition::Primary, false);
        stack.add_named(&entry, Some("text"));
        widget.append(&stack);

        let edit =
            crate::tab::icon_button("document-edit-symbolic", tr("Редактировать путь (Ctrl+L)"));
        edit.add_css_class("flat");
        widget.append(&edit);
        let keys = gtk::EventControllerKey::new();
        let weak_stack = stack.downgrade();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(stack) = weak_stack.upgrade()
            {
                stack.set_visible_child_name("segments");
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(keys);
        Self {
            widget,
            entry,
            edit,
            stack,
            scroller,
            segments,
        }
    }

    pub fn begin(&self, file: &gio::File) {
        self.entry.set_text(&file.parse_name());
        self.stack.set_visible_child_name("text");
        self.entry.grab_focus();
        self.entry.select_region(0, -1);
    }

    pub fn finish(&self) {
        self.stack.set_visible_child_name("segments");
    }

    pub fn update(&self, file: &gio::File, navigate: impl Fn(&gio::File) + Clone + 'static) {
        while let Some(child) = self.segments.first_child() {
            self.segments.remove(&child);
        }
        for (index, component) in location_components(file).into_iter().enumerate() {
            if index != 0 {
                let separator = gtk::Image::from_icon_name("go-next-symbolic");
                separator.set_pixel_size(12);
                separator.add_css_class("files-address-separator");
                self.segments.append(&separator);
            }
            let name = display_location(&component);
            let label = gtk::Label::new(Some(&name));
            label.set_max_width_chars(22);
            label.set_ellipsize(pango::EllipsizeMode::Middle);
            let button = gtk::Button::builder().child(&label).build();
            button.add_css_class("flat");
            button.set_tooltip_text(Some(&component.parse_name()));
            if component.equal(file) {
                button.add_css_class("current-segment");
            }
            let navigate = navigate.clone();
            button.connect_clicked(move |_| navigate(&component));
            self.segments.append(&button);
        }
        let first_frame = std::cell::Cell::new(true);
        self.scroller.add_tick_callback(move |scroller, _| {
            if first_frame.replace(false) {
                return glib::ControlFlow::Continue;
            }
            let horizontal = scroller.hadjustment();
            horizontal.set_value(horizontal.upper() - horizontal.page_size());
            glib::ControlFlow::Break
        });
    }
}
