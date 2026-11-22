use crate::settings::{self, PanelItem};
use gtk::{gdk, glib, prelude::*};
use gtk4_layer_shell::{Edge, KeyboardMode, Layer, LayerShell};
use std::rc::Rc;

pub struct Panel {
    pub monitor: gdk::Monitor,
    pub window: gtk::ApplicationWindow,
    pub active: gtk::Label,
    pub status: gtk::Label,
    pub tray: gtk::Box,
    clock: gtk::MenuButton,
    row: gtk::Box,
    items: Vec<(PanelItem, gtk::Widget)>,
}
impl Panel {
    pub fn new(
        app: &gtk::Application,
        monitor: &gdk::Monitor,
        activate: impl Fn(PanelItem) + 'static,
    ) -> Self {
        let activate = Rc::new(activate);
        let window = gtk::ApplicationWindow::builder().application(app).build();
        window.add_css_class("mosaic-panel");
        window.init_layer_shell();
        window.set_namespace(Some("mosaic-panel"));
        window.set_monitor(Some(monitor));
        window.set_layer(Layer::Top);
        window.set_keyboard_mode(KeyboardMode::None);
        for edge in [Edge::Top, Edge::Left, Edge::Right] {
            window.set_anchor(edge, true);
        }
        window.auto_exclusive_zone_enable();
        window.set_default_height(32);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        let menu = gtk::Button::with_label("◈ Mosaic");
        menu.add_css_class("panel-button");
        let action = activate.clone();
        menu.connect_clicked(move |_| action(PanelItem::Menu));
        let active = gtk::Label::new(Some("Рабочий стол"));
        active.set_max_width_chars(24);
        active.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let status = gtk::Label::new(None);
        status.set_max_width_chars(48);
        status.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        spacer.set_hexpand(true);
        let tray = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        let button = gtk::Button::with_label("⚙");
        button.add_css_class("panel-button");
        button.set_tooltip_text(Some("Настройки Mosaic"));
        button.update_property(&[gtk::accessible::Property::Label("Настройки Mosaic")]);
        let action = activate.clone();
        button.connect_clicked(move |_| action(PanelItem::Settings));
        let clock = calendar_button(&window);
        let quick = gtk::Button::with_label("☷");
        quick.add_css_class("panel-button");
        quick.set_tooltip_text(Some("Центр управления"));
        quick.update_property(&[gtk::accessible::Property::Label("Центр управления")]);
        let action = activate.clone();
        quick.connect_clicked(move |_| action(PanelItem::Clock));
        let clock_group = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        clock_group.append(&quick);
        clock_group.append(&clock);
        let mut items = vec![
            (PanelItem::Menu, menu.upcast()),
            (PanelItem::Active, active.clone().upcast()),
            (PanelItem::Status, status.clone().upcast()),
            (PanelItem::Spacer, spacer.upcast()),
            (PanelItem::Tray, tray.clone().upcast()),
            (PanelItem::Clock, clock_group.upcast()),
            (PanelItem::Settings, button.upcast()),
        ];
        let icons = gtk::IconTheme::for_display(&monitor.display());
        items.extend(PanelItem::SHORTCUTS.map(|item| {
            let (title, icon) = shortcut(item).unwrap();
            let button = if item == PanelItem::Temperatures {
                gtk::Button::with_label("Темп. —")
            } else if icons.has_icon(icon) {
                gtk::Button::from_icon_name(icon)
            } else {
                gtk::Button::with_label(title)
            };
            button.add_css_class("panel-button");
            button.set_tooltip_text(Some(title));
            button.update_property(&[gtk::accessible::Property::Label(title)]);
            button.set_visible(false);
            let action = activate.clone();
            button.connect_clicked(move |_| action(item));
            (item, button.upcast())
        }));
        for (_, item) in &items {
            row.append(item);
        }
        window.set_child(Some(&row));
        Self {
            monitor: monitor.clone(),
            window,
            active,
            status,
            tray,
            clock,
            row,
            items,
        }
    }
    pub fn close_calendar(&self) {
        self.clock.popdown();
    }
    pub fn configure(&self, layout: &settings::Panel) {
        for (key, widget) in &self.items {
            widget.set_visible(layout.is_visible(*key));
        }
        let mut previous = None;
        for key in &layout.order {
            let item = self.items.iter().find(|(id, _)| id == key).unwrap();
            self.row.reorder_child_after(&item.1, previous);
            previous = Some(&item.1);
        }
    }
    pub fn connector(&self) -> String {
        self.monitor
            .connector()
            .map(|v| v.to_string())
            .unwrap_or_default()
    }

    pub fn set_temperature_summary(&self, text: &str) {
        if let Some((_, widget)) = self
            .items
            .iter()
            .find(|(item, _)| *item == PanelItem::Temperatures)
            && let Some(button) = widget.downcast_ref::<gtk::Button>()
        {
            button.set_label(text);
            button.update_property(&[gtk::accessible::Property::Label(&format!(
                "Температуры: {text}"
            ))]);
        }
    }
}

fn shortcut(item: PanelItem) -> Option<(&'static str, &'static str)> {
    Some(match item {
        PanelItem::Wifi => ("Wi-Fi", "network-wireless-symbolic"),
        PanelItem::Bluetooth => ("Bluetooth", "bluetooth-symbolic"),
        PanelItem::Brightness => ("Яркость", "display-brightness-symbolic"),
        PanelItem::Audio => ("Звук", "audio-volume-high-symbolic"),
        PanelItem::Appearance => ("Оформление", "preferences-desktop-appearance-symbolic"),
        PanelItem::Dock => ("Настройки Dock", "video-display-symbolic"),
        PanelItem::Temperatures => ("Температуры", "temperature-symbolic"),
        PanelItem::Power => ("Питание", "battery-symbolic"),
        _ => return None,
    })
}

pub enum LayoutEdit {
    Move(PanelItem, isize),
    Visible(PanelItem, bool),
    Reset,
}

pub struct LayoutEditor {
    widget: gtk::Box,
    list: gtk::Box,
    rows: Vec<LayoutRow>,
}

struct LayoutRow {
    item: PanelItem,
    widget: gtk::Box,
    visible: Option<gtk::CheckButton>,
    earlier: gtk::Button,
    later: gtk::Button,
}

impl LayoutEditor {
    pub fn new(edit: impl Fn(LayoutEdit) + 'static) -> Self {
        let edit = Rc::new(edit);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let title = gtk::Label::new(Some("Верхняя панель"));
        title.set_xalign(0.);
        title.add_css_class("heading");
        widget.append(&title);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        widget.append(&list);
        let rows = PanelItem::BUILT_IN
            .into_iter()
            .chain(PanelItem::SHORTCUTS)
            .map(|item| {
                let shortcut = shortcut(item);
                let name = match item {
                    PanelItem::Menu => "Меню Mosaic",
                    PanelItem::Active => "Активное окно",
                    PanelItem::Status => "Строка состояния",
                    PanelItem::Spacer => "Свободное место",
                    PanelItem::Tray => "Системный трей",
                    PanelItem::Clock => "Центр управления и часы",
                    PanelItem::Settings => "Настройки",
                    _ => shortcut.unwrap().0,
                };
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                let title = gtk::Label::new(Some(name));
                title.set_xalign(0.);
                title.set_hexpand(true);
                row.append(&title);
                let visible = item.can_hide().then(|| {
                    let action = if shortcut.is_some() {
                        "Закрепить"
                    } else {
                        "Показывать"
                    };
                    let toggle = gtk::CheckButton::with_label(action);
                    let accessible_name = format!("{action}: {name}");
                    toggle.update_property(&[gtk::accessible::Property::Label(&accessible_name)]);
                    let edit = edit.clone();
                    toggle.connect_toggled(move |toggle| {
                        edit(LayoutEdit::Visible(item, toggle.is_active()));
                    });
                    row.append(&toggle);
                    toggle
                });
                let [earlier, later] =
                    [("↑", "Переместить выше", -1), ("↓", "Переместить ниже", 1)].map(
                        |(label, action, direction)| {
                            let button = gtk::Button::with_label(label);
                            let accessible_name = format!("{action}: {name}");
                            button.set_tooltip_text(Some(&accessible_name));
                            button.update_property(&[gtk::accessible::Property::Label(
                                &accessible_name,
                            )]);
                            let edit = edit.clone();
                            button
                                .connect_clicked(move |_| edit(LayoutEdit::Move(item, direction)));
                            row.append(&button);
                            button
                        },
                    );
                list.append(&row);
                LayoutRow {
                    item,
                    widget: row,
                    visible,
                    earlier,
                    later,
                }
            })
            .collect();
        let reset = gtk::Button::with_label("Восстановить раскладку панели");
        reset.set_halign(gtk::Align::Start);
        reset.connect_clicked(move |_| edit(LayoutEdit::Reset));
        widget.append(&reset);
        Self { widget, list, rows }
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    pub fn sync(&self, layout: &settings::Panel) {
        let mut previous = None;
        let unpinned = PanelItem::SHORTCUTS
            .iter()
            .filter(|item| !layout.order.contains(item));
        for (index, item) in layout.order.iter().chain(unpinned).enumerate() {
            let row = self.rows.iter().find(|row| row.item == *item).unwrap();
            if let Some(visible) = &row.visible {
                visible.set_active(layout.is_visible(*item));
            }
            row.earlier
                .set_sensitive(index > 0 && index < layout.order.len());
            row.later.set_sensitive(index + 1 < layout.order.len());
            self.list.reorder_child_after(&row.widget, previous);
            previous = Some(&row.widget);
        }
    }
}

fn update_clock(clock: &gtk::Label) {
    let delay = if let Ok(now) = glib::DateTime::now_local() {
        if let Ok(text) = now.format("%a %H:%M") {
            clock.set_text(&text);
        }
        (60 - now.second()) as u64
    } else {
        60
    };
    let weak = clock.downgrade();
    glib::timeout_add_local_once(std::time::Duration::from_secs(delay), move || {
        if let Some(clock) = weak.upgrade() {
            update_clock(&clock);
        }
    });
}

fn calendar_button(window: &gtk::ApplicationWindow) -> gtk::MenuButton {
    let clock = gtk::Label::new(None);
    update_clock(&clock);

    let button = gtk::MenuButton::new();
    button.add_css_class("panel-clock");
    button.set_tooltip_text(Some("Календарь"));
    button.set_child(Some(&clock));

    let calendar = gtk::Calendar::new();
    let today = gtk::Button::with_label("Сегодня");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.append(&calendar);
    content.append(&today);
    let popup = gtk::Popover::builder()
        .position(gtk::PositionType::Bottom)
        .autohide(true)
        .child(&content)
        .build();
    popup.add_css_class("mosaic-popup");
    popup.add_css_class("panel-calendar");

    let grid = calendar.downgrade();
    today.connect_clicked(move |_| {
        if let Some(grid) = grid.upgrade()
            && let Ok(now) = glib::DateTime::now_local()
        {
            grid.set_date(&now);
        }
    });
    let panel = window.downgrade();
    let grid = calendar.downgrade();
    popup.connect_map(move |_| {
        if let Some(grid) = grid.upgrade()
            && let Ok(now) = glib::DateTime::now_local()
        {
            grid.set_date(&now);
        }
        if let Some(panel) = panel.upgrade() {
            panel.set_keyboard_mode(KeyboardMode::Exclusive);
        }
    });
    let panel = window.downgrade();
    popup.connect_unmap(move |_| {
        if let Some(panel) = panel.upgrade() {
            panel.set_keyboard_mode(KeyboardMode::None);
        }
    });
    button.set_popover(Some(&popup));
    button
}
