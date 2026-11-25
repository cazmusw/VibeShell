use crate::{
    desktop::{self, Application as Desktop, Window},
    ipc::{ApiError, Command, DialogInfo, Shared},
    panel::{LayoutEdit, LayoutEditor},
    settings::{self, DockEdge, IconFilter, PanelItem, Settings, SymbolicIconWeight, ThemeMode},
    temperatures::{self, Kind as SensorKind, Reading, Snapshot as Temperatures},
    toplevel::Tracker,
};
use crate::{
    dock::{self, Action, Dock},
    dock_model::Launches,
};
use gtk::{gdk, gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};

use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    future::{Future, poll_fn},
    path::PathBuf,
    rc::Rc,
    task::Poll,
    time::{Duration, Instant},
};

struct AppEntry {
    info: gio::AppInfo,
    model: Desktop,
    name: String,
    desktop_file: Option<PathBuf>,
}
struct View {
    app: gtk::Application,
    appearance_css: gtk::CssProvider,
    appearance_style: Cell<(u32, u32, SymbolicIconWeight)>,
    trash: Rc<crate::trash::Trash>,
    tray: Rc<crate::tray_ui::Tray>,
    panels: RefCell<Vec<crate::panel::Panel>>,
    status_text: RefCell<String>,
    primary_dock: RefCell<Rc<DockView>>,
    extra_docks: RefCell<Vec<Rc<DockView>>>,

    apps: Vec<AppEntry>,
    models: Vec<Desktop>,
    windows: RefCell<Vec<Window>>,
    settings: RefCell<Settings>,
    path: PathBuf,
    tracker: Rc<RefCell<Tracker>>,
    preview_busy: Cell<bool>,
    shared: Shared,
    pending: RefCell<Launches>,
    preferences: RefCell<Option<Preferences>>,
    quick_appearance: RefCell<Option<QuickAppearance>>,
    wifi_pane: RefCell<Option<Rc<crate::network_ui::WifiPane>>>,
    bluetooth_pane: RefCell<Option<Rc<crate::bluetooth_ui::BluetoothPane>>>,
    brightness_pane: RefCell<Option<Rc<crate::brightness_ui::BrightnessPane>>>,
    temperature_pane: RefCell<Option<gtk::Box>>,
    temperature_timer: RefCell<Option<glib::SourceId>>,
    temperature_pending: Cell<bool>,
    temperature_snapshot: RefCell<Option<Temperatures>>,
    power_pane: RefCell<Option<Rc<crate::power_ui::PowerPane>>>,
    audio_pane: RefCell<Option<Rc<crate::audio_ui::AudioPane>>>,
    dialog: RefCell<Option<ShellDialog>>,
    syncing: Cell<bool>,
    dropping_file: Cell<bool>,
}
struct DockView {
    dock: Dock,
    monitor: RefCell<Option<gdk::Monitor>>,
    geometry: Cell<Option<gdk::Rectangle>>,
    popup: RefCell<Option<DockMenu>>,
    cards: RefCell<Option<WindowCards>>,
    cards_timer: RefCell<Option<glib::SourceId>>,
}
impl DockView {
    fn new(app: &gtk::Application) -> Rc<Self> {
        Rc::new(Self {
            dock: Dock::new(app, None),
            monitor: RefCell::new(None),
            geometry: Cell::new(None),
            popup: RefCell::new(None),
            cards: RefCell::new(None),
            cards_timer: RefCell::new(None),
        })
    }
    fn place(&self, monitor: Option<&gdk::Monitor>) {
        let geometry = monitor.map(|m| m.geometry());
        let changed = self.monitor.borrow().as_ref() != monitor;
        if changed || self.geometry.get() != geometry {
            self.close_cards();
            self.close_popup();
            *self.monitor.borrow_mut() = monitor.cloned();
            self.geometry.set(geometry);
            self.dock.set_monitor(monitor);
        }
    }
    fn close_cards(&self) {
        if let Some(timer) = self.cards_timer.borrow_mut().take() {
            timer.remove();
        }
        let open = self.cards.borrow_mut().take();
        if let Some(open) = open {
            open.popup.popdown();
            if open.popup.parent().is_some() {
                open.popup.unparent();
            }
            self.dock.menu(false);
        }
    }

    fn defer_cards_close(self: &Rc<Self>, delay_ms: u64) {
        if let Some(timer) = self.cards_timer.borrow_mut().take() {
            timer.remove();
        }
        if self.cards.borrow().is_none() {
            return;
        }
        let weak = Rc::downgrade(self);
        let timer =
            glib::timeout_add_local_once(std::time::Duration::from_millis(delay_ms), move || {
                if let Some(v) = weak.upgrade() {
                    v.cards_timer.borrow_mut().take();
                    v.close_cards();
                }
            });
        *self.cards_timer.borrow_mut() = Some(timer);
    }

    fn close_popup(&self) {
        let open = self.popup.borrow_mut().take();
        if let Some(DockMenu { popup: p, .. }) = open {
            p.popdown();
            if p.parent().is_some() {
                p.unparent();
            }
            self.dock.menu(false);
        }
    }
}
struct WindowCards {
    app: String,
    rows: Vec<WindowCard>,
    popup: gtk::Popover,
}
struct DockMenu {
    app: String,
    windows: Vec<(u64, u32)>,
    popup: gtk::Popover,
}
struct WindowCard {
    key: (u64, u32),
    button: gtk::Button,
    title: gtk::Label,
    status: gtk::Label,
    preview: CardPreview,
}
struct CardPreview {
    stack: gtk::Stack,
    picture: gtk::Picture,
    identifier: RefCell<Option<String>>,
    last_attempt: Cell<Option<Instant>>,
}
impl CardPreview {
    fn new(fallback: &gtk::Box, capture_available: bool) -> Self {
        let picture = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .can_shrink(true)
            .build();
        let stack = gtk::Stack::new();
        stack.add_named(fallback, Some("fallback"));
        if capture_available {
            let canvas = gtk::Overlay::new();
            canvas.set_size_request(224, 92);
            canvas.add_css_class("dock-window-preview");
            canvas.add_overlay(&picture);
            stack.add_named(&canvas, Some("frame"));
        }
        Self {
            stack,
            picture,
            identifier: RefCell::new(None),
            last_attempt: Cell::new(None),
        }
    }
    fn invalidate(&self, identifier: Option<&str>) -> bool {
        let invalid = self
            .identifier
            .borrow()
            .as_deref()
            .is_some_and(|current| Some(current) != identifier);
        if invalid {
            self.identifier.borrow_mut().take();
            self.picture.set_paintable(gdk::Paintable::NONE);
            self.stack.set_visible_child_name("fallback");
            self.last_attempt.set(None);
        }
        invalid
    }
    fn display(&self, identifier: String, image: &gdk::Texture) {
        self.picture.set_paintable(Some(image));
        *self.identifier.borrow_mut() = Some(identifier);
        self.stack.set_visible_child_name("frame");
    }
}
impl WindowCard {
    fn visible_in(&self, scroll: &gtk::ScrolledWindow) -> bool {
        self.button.compute_bounds(scroll).is_some_and(|bounds| {
            bounds.y() + bounds.height() > 0. && bounds.y() < scroll.height() as f32
        })
    }

    fn update(&self, window: &Window) {
        self.title.set_text(if window.title.is_empty() {
            "Окно"
        } else {
            &window.title
        });
        self.status.set_text(if window.minimized {
            "Свёрнуто"
        } else if window.active {
            "Активно"
        } else {
            "Открыто"
        });
        self.describe();
    }
    fn describe(&self) {
        let mut description = format!("{}. {}.", self.title.text(), self.status.text());
        if self.preview.identifier.borrow().is_some() {
            description.push_str(" Миниатюра окна.");
        }
        self.button
            .update_property(&[gtk::accessible::Property::Label(&description)]);
    }
}
struct Preferences {
    window: gtk::ApplicationWindow,
    pages: gtk::Stack,
    appearance: Appearance,
    dock_edge: gtk::DropDown,
    preferred: gtk::Entry,
    current_output_only: gtk::Switch,
    all_outputs: gtk::Switch,
    auto_hide: gtk::Switch,
    magnification: gtk::Switch,
    hover_values: [gtk::SpinButton; HOVER_VALUES.len()],
    hover_easing: gtk::DropDown,
    hover_cards: gtk::Switch,
    minimize: gtk::Switch,
    size: gtk::SpinButton,
    visual_size: gtk::SpinButton,
    icon_filter: gtk::DropDown,
    show_delay: gtk::SpinButton,
    hide_delay: gtk::SpinButton,
    panel_transparency: gtk::SpinButton,
    dock_transparency: gtk::SpinButton,
    panel: LayoutEditor,
    error: gtk::Label,
}

const HOVER_VALUES: [(&str, &str, f64, f64, f64); 7] = [
    (
        "Увеличение значков, %",
        "dock.hover.zoom_percent",
        0.,
        70.,
        5.,
    ),
    (
        "Радиус влияния, % размера значка",
        "dock.hover.radius_percent",
        25.,
        200.,
        5.,
    ),
    (
        "Расширение фона, %",
        "dock.hover.background_percent",
        0.,
        25.,
        1.,
    ),
    (
        "Длительность увеличения, мс",
        "dock.hover.grow_ms",
        0.,
        1_000.,
        25.,
    ),
    (
        "Длительность возврата, мс",
        "dock.hover.shrink_ms",
        0.,
        1_000.,
        25.,
    ),
    (
        "Задержка открытия карточек, мс",
        "dock.hover.cards_show_delay_ms",
        0.,
        2_000.,
        50.,
    ),
    (
        "Задержка закрытия карточек, мс",
        "dock.hover.cards_hide_delay_ms",
        0.,
        2_000.,
        50.,
    ),
];

const SYMBOLIC_WEIGHTS: [(SymbolicIconWeight, &str, &str); 3] = [
    (SymbolicIconWeight::Default, "default", "Из темы"),
    (SymbolicIconWeight::Medium, "medium", "Толще"),
    (SymbolicIconWeight::Bold, "bold", "Максимальная"),
];

const ICON_FILTERS: [(IconFilter, &str, &str); 3] = [
    (IconFilter::Linear, "linear", "Обычная"),
    (IconFilter::Smooth, "smooth", "Сглаженная"),
    (IconFilter::Nearest, "nearest", "Пиксельная"),
];

const HOVER_EASINGS: [(settings::HoverEasing, &str, &str); 3] = [
    (settings::HoverEasing::Linear, "linear", "Линейная"),
    (
        settings::HoverEasing::EaseOut,
        "ease_out",
        "Замедление к концу",
    ),
    (
        settings::HoverEasing::EaseInOut,
        "ease_in_out",
        "Плавное начало и конец",
    ),
];
#[derive(Clone, Copy)]
enum PreferencesSection {
    General,
    Appearance,
    Dock,
}
impl PreferencesSection {
    fn name(self) -> &'static str {
        match self {
            Self::General => "general",
            Self::Appearance => "appearance",
            Self::Dock => "dock",
        }
    }
}
struct Appearance {
    theme: gtk::DropDown,
    symbolic_weight: gtk::DropDown,
    reduced: gtk::Switch,
}
impl Appearance {
    fn sync(&self, settings: &Settings) {
        self.theme.set_selected(match settings.theme.mode {
            ThemeMode::Dark => 0,
            ThemeMode::Light => 1,
            ThemeMode::System => 2,
        });
        self.symbolic_weight.set_selected(
            SYMBOLIC_WEIGHTS
                .iter()
                .position(|(weight, _, _)| *weight == settings.theme.symbolic_icon_weight)
                .unwrap_or(0) as u32,
        );
        self.reduced.set_active(settings.general.reduced_motion);
    }
}
struct QuickAppearance {
    appearance: Appearance,
    controls: gtk::Box,
    managed_note: gtk::Label,
    error: gtk::Label,
    _audio: Rc<crate::audio_ui::AudioPane>,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum DialogKind {
    Launcher,
    Settings,
    ControlCenter,
    Wifi,
    Bluetooth,
    Brightness,
    Temperatures,
    Power,
    Audio,
    EmptyTrash,
}
impl DialogKind {
    fn name(self) -> &'static str {
        match self {
            Self::Launcher => "launcher",
            Self::Settings => "settings",
            Self::ControlCenter => "control-center",
            Self::Wifi => "wifi",
            Self::Bluetooth => "bluetooth",
            Self::Brightness => "brightness",
            Self::Temperatures => "temperatures",
            Self::Power => "power",
            Self::Audio => "audio",
            Self::EmptyTrash => "empty-trash",
        }
    }
}
struct ShellDialog {
    kind: DialogKind,
    monitor: gdk::Monitor,
    window: gtk::ApplicationWindow,
}
fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
        });
    base.join("mosaic-shell/config.toml")
}
async fn read_desktop_drop(drop: &gdk::Drop) -> Option<PathBuf> {
    let (stream, _) = drop
        .read_future(&[dock::FILE_DROP_MIME], glib::Priority::DEFAULT)
        .await
        .ok()?;
    let (bytes, size, error) = stream
        .read_all_future(
            vec![0; desktop::DESKTOP_DROP_LIMIT + 1],
            glib::Priority::DEFAULT,
        )
        .await
        .ok()?;
    if error.is_some() {
        return None;
    }
    let uri = desktop::dropped_desktop_uri(&bytes[..size])?;
    gio::File::for_uri(uri).path()
}
fn label(text: &str) -> gtk::Label {
    gtk::Label::new(Some(text))
}
fn file_image(path: &std::path::Path, size: i32, native_raster: bool) -> Option<gtk::Widget> {
    let size = if native_raster {
        let (format, width, height) = gtk::gdk_pixbuf::Pixbuf::file_info(path)?;
        if format.name().as_deref() == Some("svg") {
            size
        } else {
            size.min(width.max(height))
        }
    } else {
        size
    };
    gtk::gdk_pixbuf::Pixbuf::from_file_at_scale(path, size, size, true)
        .ok()
        .map(pixbuf_image)
}

fn pixbuf_image(pixels: gtk::gdk_pixbuf::Pixbuf) -> gtk::Widget {
    let format = if pixels.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    let texture = gdk::MemoryTexture::new(
        pixels.width(),
        pixels.height(),
        format,
        &pixels.read_pixel_bytes(),
        pixels.rowstride() as usize,
    );
    gtk::Picture::for_paintable(&texture).upcast()
}

fn image(info: Option<&gio::AppInfo>, id: &str, name: &str, size: i32, dock: bool) -> gtk::Widget {
    let icon = info.and_then(|a| a.icon());
    if let Some(file) = icon.and_downcast_ref::<gio::FileIcon>()
        && let Some(path) = file.file().path()
        && let Some(image) = file_image(&path, if dock { 160 } else { size }, dock)
    {
        return image;
    }
    let display_theme =
        gdk::Display::default().map(|display| gtk::IconTheme::for_display(&display));
    let app_id = id.strip_prefix("window:").unwrap_or(id);
    let app_id = app_id.strip_suffix(".desktop").unwrap_or(app_id);
    let base_name = app_id.rsplit('.').next().unwrap_or(app_id);
    let lower_name = base_name.to_lowercase();
    let mut names = icon
        .and_downcast_ref::<gio::ThemedIcon>()
        .map(gio::ThemedIcon::names)
        .unwrap_or_default();
    names.extend([app_id.into(), base_name.into(), lower_name.into()]);
    for name in names
        .iter()
        .filter(|name| !name.is_empty() && !name.contains(['/', '\\', ':']))
    {
        if let Some(theme) = display_theme.as_ref().filter(|theme| theme.has_icon(name)) {
            if dock && !name.ends_with("-symbolic") {
                let icon = theme.lookup_icon(
                    name,
                    &[],
                    size,
                    1,
                    gtk::TextDirection::None,
                    gtk::IconLookupFlags::empty(),
                );
                if let Some(path) = icon.file().and_then(|file| file.path())
                    && !path.to_string_lossy().ends_with(".symbolic.png")
                    && path
                        .extension()
                        .and_then(std::ffi::OsStr::to_str)
                        .is_some_and(|extension| {
                            matches!(extension, "png" | "xpm" | "jpg" | "jpeg" | "webp")
                        })
                    && let Some(image) = file_image(&path, 160, true)
                {
                    return image;
                }
            }
            let image = gtk::Image::from_icon_name(name);
            image.set_pixel_size(size);
            return image.upcast();
        }
        // Some launchers install freestanding icons under XDG pixmaps.
        for dir in std::iter::once(glib::user_data_dir()).chain(glib::system_data_dirs()) {
            for extension in ["png", "svg", "xpm"] {
                let path = dir.join("pixmaps").join(format!("{name}.{extension}"));
                if path.is_file()
                    && let Some(image) = file_image(&path, if dock { 160 } else { size }, dock)
                {
                    return image;
                }
            }
        }
    }
    let text: String = name
        .rsplit('.')
        .next()
        .filter(|name| *name != "desktop")
        .unwrap_or(base_name)
        .chars()
        .find(|character| character.is_alphanumeric())
        .unwrap_or('?')
        .to_uppercase()
        .collect();
    let tile = label(&text);
    tile.add_css_class("fallback-icon");
    tile.set_size_request(size, size);
    tile.upcast()
}
impl View {
    fn primary_dock(&self) -> Rc<DockView> {
        self.primary_dock.borrow().clone()
    }
    fn docks(&self) -> Vec<Rc<DockView>> {
        std::iter::once(self.primary_dock())
            .chain(self.extra_docks.borrow().iter().cloned())
            .collect()
    }
    fn dock_on(&self, monitor: &gdk::Monitor) -> Option<Rc<DockView>> {
        self.docks()
            .into_iter()
            .find(|d| d.monitor.borrow().as_ref() == Some(monitor))
    }
    fn dock_named(&self, name: &str) -> Option<Rc<DockView>> {
        self.docks().into_iter().find(|d| {
            !name.is_empty()
                && d.monitor
                    .borrow()
                    .as_ref()
                    .and_then(gdk::Monitor::connector)
                    .as_deref()
                    == Some(name)
        })
    }
    fn resolve_dock(&self, hint: &str) -> Result<Rc<DockView>, ApiError> {
        if hint.is_empty() {
            Ok(self.primary_dock())
        } else {
            self.dock_named(hint)
                .ok_or_else(|| ApiError::InvalidArgument("no Dock on this output".into()))
        }
    }
    fn new_dock(self: &Rc<Self>, config: &settings::Dock) -> Rc<DockView> {
        let dock = DockView::new(&self.app);
        self.wire_dock(&dock);
        dock.dock
            .configure(config, self.reduced_motion(), !self.path.is_symlink());
        self.style_window(dock.dock.window());
        dock.dock.set_trash_state(self.trash.state());
        dock
    }
    fn wire_dock(self: &Rc<Self>, dock: &Rc<DockView>) {
        let weak = Rc::downgrade(self);
        let owner = Rc::downgrade(dock);
        dock.dock.callback(move |action| {
            if let (Some(v), Some(d)) = (weak.upgrade(), owner.upgrade()) {
                match action {
                    Action::Activate(id) => v.activate(&d, &id),
                    Action::Hover(id, button) => v.hover_cards(&d, id, button),
                    Action::HoverLeave => d.defer_cards_close(v.cards_hide_delay()),
                    Action::CancelHover => d.close_cards(),
                    Action::Menu(id, b) => v.menu(&d, &b, &id),
                    Action::Applications => {
                        let monitor = d.monitor.borrow().clone();
                        v.launcher(monitor.as_ref(), "");
                    }
                    Action::OpenTrash => v.open_trash(),
                    Action::ConfirmEmptyTrash => {
                        if let Some(monitor) = d.monitor.borrow().clone() {
                            v.confirm_empty_trash(&monitor);
                        }
                    }
                    Action::Reorder(source, before) => v.reorder_pin(&source, &before),
                    Action::DesktopDrop(drop, before) => v.drop_desktop_file(drop, before),
                }
            }
        });
        let owner = Rc::downgrade(dock);
        dock.dock.window().connect_unmap(move |_| {
            if let Some(d) = owner.upgrade() {
                d.close_cards();
                d.close_popup();
            }
        });
    }
    fn release_dock_focus(&self) {
        for dock in self.docks() {
            dock.dock.release_focus();
        }
    }
    fn open_trash(self: &Rc<Self>) {
        self.close_dialog();
        self.release_dock_focus();
        self.trash.refresh();
        let context = gdk::Display::default().map(|display| display.app_launch_context());
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if gio::AppInfo::launch_default_for_uri_future(crate::trash::URI, context.as_ref())
                .await
                .is_err()
                && let Some(view) = weak.upgrade()
            {
                view.fail("Не удалось открыть корзину");
            }
        });
    }
    fn confirm_empty_trash(self: &Rc<Self>, monitor: &gdk::Monitor) {
        if self.trash.state() != crate::trash::TrashState::Full {
            self.trash.refresh();
            return;
        }
        let Some((window, body)) = self.dialog_window(
            DialogKind::EmptyTrash,
            monitor,
            "Очистить корзину?",
            410,
            180,
        ) else {
            return;
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        let message = label("Файлы будут удалены без возможности восстановления.");
        message.set_wrap(true);
        content.append(&message);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        row.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Отмена");
        let confirm = gtk::Button::with_label("Удалить файлы");
        confirm.add_css_class("destructive-action");
        let weak = window.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
        let weak = Rc::downgrade(self);
        confirm.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.close_dialog();
                let weak = Rc::downgrade(&view);
                view.trash.clear_confirmed(move |result| {
                    if let Some(view) = weak.upgrade() {
                        view.fail(if result.is_ok() {
                            ""
                        } else {
                            "Не удалось полностью очистить корзину"
                        });
                    }
                });
            }
        });
        row.append(&cancel);
        row.append(&confirm);
        content.append(&row);
        body.append(&content);
        window.present();
        cancel.grab_focus();
    }
    fn sync_outputs(self: &Rc<Self>) {
        let Some(display) = gdk::Display::default() else {
            return;
        };
        let model = display.monitors();
        let monitors: Vec<gdk::Monitor> = (0..model.n_items())
            .filter_map(|n| model.item(n).and_downcast::<gdk::Monitor>())
            .filter(|m| m.is_valid())
            .collect();
        let removed = self
            .panels
            .borrow()
            .iter()
            .any(|p| !monitors.contains(&p.monitor));
        if removed {
            self.tray.menus.close();
        }
        if self
            .dialog
            .borrow()
            .as_ref()
            .is_some_and(|dialog| !monitors.contains(&dialog.monitor))
        {
            self.close_dialog();
        }
        {
            let mut panels = self.panels.borrow_mut();
            panels.retain(|p| {
                if monitors.contains(&p.monitor) {
                    true
                } else {
                    p.window.close();
                    false
                }
            });
        }
        for monitor in &monitors {
            if self.panels.borrow().iter().any(|p| p.monitor == *monitor) {
                continue;
            }
            let weak = Rc::downgrade(self);
            let source = monitor.clone();
            let panel = crate::panel::Panel::new(&self.app, monitor, move |item| {
                let Some(view) = weak.upgrade() else {
                    return;
                };
                match item {
                    PanelItem::Menu => view.launcher(Some(&source), ""),
                    PanelItem::Settings => view.settings(Some(&source)),
                    PanelItem::Clock => view.toggle_control_center(Some(&source), ""),
                    PanelItem::Wifi => view.show_wifi(Some(&source), ""),
                    PanelItem::Bluetooth => view.show_bluetooth(Some(&source), ""),
                    PanelItem::Brightness => view.show_brightness(Some(&source), ""),
                    PanelItem::Temperatures => view.show_temperatures(Some(&source), ""),
                    PanelItem::Power => view.show_power(Some(&source), ""),
                    PanelItem::Audio => view.show_audio(&source),
                    PanelItem::Appearance => {
                        view.settings_section(Some(&source), PreferencesSection::Appearance);
                    }
                    PanelItem::Dock => {
                        view.settings_section(Some(&source), PreferencesSection::Dock);
                    }
                    PanelItem::Active | PanelItem::Status | PanelItem::Spacer | PanelItem::Tray => {
                    }
                }
            });
            self.tray.attach(&panel.tray, monitor);
            self.style_window(&panel.window);
            panel.status.set_text(&self.status_text.borrow());
            self.panels.borrow_mut().push(panel);
            let weak = Rc::downgrade(self);
            monitor.connect_notify_local(None, move |_, spec| {
                if matches!(spec.name(), "geometry" | "scale" | "connector" | "valid")
                    && let Some(v) = weak.upgrade()
                {
                    v.sync_outputs();
                }
            });
        }
        {
            let settings = self.settings.borrow();
            for panel in self.panels.borrow().iter() {
                panel.configure(&settings.panel);
            }
        }
        self.sync_temperature_poll();
        let names: Vec<_> = monitors
            .iter()
            .map(|m| m.connector().map(|s| s.to_string()).unwrap_or_default())
            .collect();
        let name =
            crate::outputs::select_output(&self.settings.borrow().dock.preferred_output, &names);
        let selected = name
            .and_then(|name| {
                monitors
                    .iter()
                    .find(|m| m.connector().as_deref() == Some(name.as_str()))
                    .cloned()
            })
            .or_else(|| monitors.first().cloned());
        let dock_config = self.settings.borrow().dock.clone();
        let mut extras = std::mem::take(&mut *self.extra_docks.borrow_mut());
        if let Some(index) = extras
            .iter()
            .position(|d| selected.is_some() && d.monitor.borrow().as_ref() == selected.as_ref())
        {
            // Keep both physical docks when changing only the preferred output.
            let next = extras.swap_remove(index);
            extras.push(self.primary_dock.replace(next));
        } else if dock_config.all_outputs
            && self
                .primary_dock()
                .monitor
                .borrow()
                .as_ref()
                .is_some_and(|old| monitors.contains(old) && Some(old) != selected.as_ref())
        {
            // The preferred output has returned. Leave the surviving Dock in place.
            extras.push(self.primary_dock.replace(self.new_dock(&dock_config)));
        }
        extras.retain(|d| {
            let monitor = d.monitor.borrow().clone();
            let keep = dock_config.all_outputs
                && monitor.is_some_and(|m| monitors.contains(&m) && Some(&m) != selected.as_ref());
            if !keep {
                d.place(None);
                d.dock.window().destroy();
                d.dock.trigger().destroy();
            }
            keep
        });
        self.primary_dock().place(selected.as_ref());
        if dock_config.all_outputs {
            for monitor in monitors.iter().filter(|m| Some(*m) != selected.as_ref()) {
                if let Some(d) = extras
                    .iter()
                    .find(|d| d.monitor.borrow().as_ref() == Some(monitor))
                {
                    d.place(Some(monitor));
                } else {
                    let d = self.new_dock(&dock_config);
                    d.place(Some(monitor));
                    extras.push(d);
                }
            }
        }
        *self.extra_docks.borrow_mut() = extras;
        self.redraw();
    }
    fn output_info(&self) -> Vec<crate::ipc::OutputInfo> {
        let mut out: Vec<_> = self
            .panels
            .borrow()
            .iter()
            .map(|p| {
                let r = p.monitor.geometry();
                let chosen = self.primary_dock().monitor.borrow().as_ref() == Some(&p.monitor);
                crate::ipc::OutputInfo {
                    connector: p.connector(),
                    manufacturer: p
                        .monitor
                        .manufacturer()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    model: p.monitor.model().map(|s| s.to_string()).unwrap_or_default(),
                    description: p
                        .monitor
                        .description()
                        .map(|s| s.to_string())
                        .unwrap_or_default(),
                    x: r.x(),
                    y: r.y(),
                    width: r.width(),
                    height: r.height(),
                    scale: p.monitor.scale(),
                    panel_mapped: p.window.is_mapped(),
                    panel_width: p.window.width(),
                    panel_height: p.window.height(),
                    dock_selected: chosen,
                    dock_mapped: self
                        .dock_on(&p.monitor)
                        .is_some_and(|d| d.dock.window().is_mapped()),
                }
            })
            .collect();
        out.sort_by(|a, b| a.connector.cmp(&b.connector));
        out
    }

    fn app_entry(&self, id: &str) -> Option<&AppEntry> {
        self.apps
            .iter()
            .find(|entry| entry.model.id == id)
            .or_else(|| {
                let id = desktop::match_app(id, &self.models)?;
                self.apps.iter().find(|entry| entry.model.id == id)
            })
    }
    fn key(&self, w: &Window) -> String {
        desktop::match_app(&w.app_id, &self.models)
            .map(|id| {
                self.settings
                    .borrow()
                    .pins
                    .iter()
                    .find(|pin| {
                        pin.as_str() == id
                            || self
                                .app_entry(pin)
                                .is_some_and(|entry| entry.model.id == id)
                    })
                    .cloned()
                    .unwrap_or(id)
            })
            .unwrap_or_else(|| {
                if w.app_id.is_empty() {
                    format!("window:{}:{}", w.epoch, w.id)
                } else {
                    format!("window:{}", w.app_id)
                }
            })
    }
    fn fail(&self, message: &str) {
        *self.status_text.borrow_mut() = message.into();
        for p in self.panels.borrow().iter() {
            p.status.set_text(message);
        }
    }
    fn show_error(&self, error: &ApiError) {
        let category = match error {
            ApiError::Conflict(_) => "settings_conflict",
            ApiError::PermissionDenied(_) => "settings_denied",
            ApiError::InvalidArgument(_) => "settings_invalid",
            _ => "settings_failed",
        };
        crate::diagnostics::event("shell", category, 1);
        self.fail(match error {
            ApiError::Conflict(_) => "Настройки изменились. Повторите действие после обновления.",
            ApiError::PermissionDenied(_) => "Настройки управляются декларативной конфигурацией.",
            ApiError::InvalidArgument(_) => "Некорректные значения настроек.",
            _ => "Не удалось применить настройки.",
        });
    }
    fn launch(self: &Rc<Self>, id: &str) {
        self.release_dock_focus();
        let Some(entry) = self.app_entry(id) else {
            crate::diagnostics::event("shell", "launcher_entry_missing", 1);
            self.fail("Приложение не найдено");
            return;
        };
        let context = gdk::Display::default().map(|d| d.app_launch_context());
        match entry.info.launch(&[], context.as_ref()) {
            Ok(()) => {
                self.close_dialog();
                self.fail("");
                let id = entry.model.id.as_str();
                let has_window = self.windows.borrow().iter().any(|window| {
                    desktop::match_app(&window.app_id, &self.models).as_deref() == Some(id)
                });
                if !has_window {
                    self.track_launch(id);
                }
                self.redraw();
            }
            Err(_) => {
                crate::diagnostics::event("shell", "launcher_failed", 1);
                self.fail("Не удалось запустить приложение");
            }
        }
    }
    fn track_launch(self: &Rc<Self>, id: &str) {
        let token = self.pending.borrow_mut().start(id);
        let id = id.to_owned();
        let weak = Rc::downgrade(self);
        glib::timeout_add_local_once(Duration::from_secs(10), move || {
            if let Some(view) = weak.upgrade() {
                let expired = view.pending.borrow_mut().expire(&id, token);
                if expired {
                    crate::diagnostics::event("shell", "launcher_window_unobserved", 0);
                    view.redraw();
                }
            }
        });
    }
    fn commit(self: &Rc<Self>, next: Settings) -> Result<Settings, ApiError> {
        if self.path.is_symlink() {
            return Err(ApiError::PermissionDenied("managed configuration".into()));
        }
        if next == *self.settings.borrow() {
            return Ok(next);
        }
        let saved = settings::save(&self.path, &next).map_err(|e| {
            if e.kind() == std::io::ErrorKind::PermissionDenied {
                ApiError::PermissionDenied("managed configuration".into())
            } else if e.to_string().contains("revision") {
                ApiError::Conflict("revision changed".into())
            } else {
                ApiError::Failed("cannot save config".into())
            }
        })?;
        *self.settings.borrow_mut() = saved.clone();
        self.fail("");
        self.reconfigure();
        self.redraw();
        Ok(saved)
    }
    fn patch(self: &Rc<Self>, revision: u64, patch: &str) -> Result<Settings, ApiError> {
        if self.settings.borrow().revision != revision {
            return Err(ApiError::Conflict("stale expected_revision".into()));
        }
        let next = settings::patched(&self.settings.borrow(), patch)
            .map_err(|_| ApiError::InvalidArgument("invalid config patch".into()))?;
        self.commit(next)
    }
    fn change(self: &Rc<Self>, patch: &str) {
        if self.syncing.get() {
            return;
        }
        let revision = self.settings.borrow().revision;
        if let Err(error) = self.patch(revision, patch) {
            self.show_error(&error);
            self.sync_preferences();
            if let Some(prefs) = self.preferences.borrow().as_ref() {
                prefs.error.set_text(&self.status_text.borrow());
                prefs.error.set_visible(true);
            }
            if let Some(quick) = self.quick_appearance.borrow().as_ref() {
                quick.error.set_text(&self.status_text.borrow());
                quick.error.set_visible(true);
            }
        }
    }
    fn pin(self: &Rc<Self>, id: &str, pinned: bool) {
        let mut next = self.settings.borrow().clone();
        if pinned {
            let Some(entry) = self.app_entry(id) else {
                self.fail("Приложение не найдено");
                return;
            };
            if next.pins.iter().any(|pin| {
                self.app_entry(pin)
                    .is_some_and(|pinned| pinned.model.id == entry.model.id)
            }) {
                return;
            }
            next.pins.push(id.into());
        } else {
            next.pins.retain(|p| p != id);
        }
        if let Err(error) = self.commit(next) {
            self.show_error(&error);
        }
    }
    fn drop_desktop_file(self: &Rc<Self>, drop: gdk::Drop, before: String) {
        let view = self.clone();
        glib::spawn_future_local(async move {
            if view.dropping_file.replace(true) {
                drop.finish(gdk::DragAction::empty());
                return;
            }
            let result = view.pin_dropped_file(&drop, &before).await;
            view.dropping_file.set(false);
            if let Err(error) = result {
                if matches!(error, ApiError::InvalidArgument(_)) {
                    view.fail("Перетащите один ярлык установленного приложения");
                } else {
                    view.show_error(&error);
                }
                drop.finish(gdk::DragAction::empty());
            } else {
                view.fail("");
                drop.finish(gdk::DragAction::COPY);
            }
        });
    }
    async fn resolve_dropped_app(&self, drop: &gdk::Drop) -> Option<String> {
        let path = read_desktop_drop(drop).await?;
        let candidates: Vec<_> = self
            .apps
            .iter()
            .filter_map(|app| {
                let file = app.desktop_file.as_ref()?;
                (file.file_name() == path.file_name()).then(|| (app.model.id.clone(), file.clone()))
            })
            .collect();
        if candidates.is_empty() {
            return None;
        }
        desktop::resolve_desktop_drop(&path, &candidates).await
    }
    async fn pin_dropped_file(
        self: &Rc<Self>,
        drop: &gdk::Drop,
        before: &str,
    ) -> Result<(), ApiError> {
        let id = {
            let mut read = std::pin::pin!(self.resolve_dropped_app(drop));
            let mut deadline = glib::timeout_future_seconds(4);
            poll_fn(|cx| {
                if let Poll::Ready(id) = read.as_mut().poll(cx) {
                    Poll::Ready(id.ok_or_else(|| {
                        ApiError::InvalidArgument("expected one installed desktop launcher".into())
                    }))
                } else {
                    deadline
                        .as_mut()
                        .poll(cx)
                        .map(|_| Err(ApiError::Timeout("desktop drop deadline exceeded".into())))
                }
            })
            .await
        }?;
        let mut next = self.settings.borrow().clone();
        if self.path.is_symlink() {
            return Err(ApiError::PermissionDenied("managed configuration".into()));
        }
        if !next.pins.iter().any(|pin| {
            pin == &id
                || self
                    .app_entry(pin)
                    .is_some_and(|entry| entry.model.id == id)
        }) {
            let position = next
                .pins
                .iter()
                .position(|pin| pin == before)
                .unwrap_or(next.pins.len());
            next.pins.insert(position, id);
            self.commit(next)?;
        }
        Ok(())
    }
    fn reorder(
        self: &Rc<Self>,
        revision: u64,
        source: &str,
        before: &str,
    ) -> Result<Settings, ApiError> {
        if revision != self.settings.borrow().revision {
            return Err(ApiError::Conflict("stale expected_revision".into()));
        }
        let mut next = self.settings.borrow().clone();
        next.pins = crate::dock_model::reorder(&next.pins, source, before)
            .map_err(ApiError::InvalidArgument)?;
        self.commit(next)
    }
    fn reorder_pin(self: &Rc<Self>, source: &str, before: &str) {
        let revision = self.settings.borrow().revision;
        if let Err(error) = self.reorder(revision, source, before) {
            self.show_error(&error);
        }
    }
    fn reload(self: &Rc<Self>) -> Result<Settings, ApiError> {
        let next = settings::load(&self.path).map_err(|_| {
            ApiError::InvalidArgument("invalid config; last working settings retained".into())
        })?;
        *self.settings.borrow_mut() = next.clone();
        self.reconfigure();
        self.redraw();
        self.fail("");
        Ok(next)
    }
    fn light(&self) -> bool {
        match self.settings.borrow().theme.mode {
            ThemeMode::Dark => false,
            ThemeMode::Light => true,
            ThemeMode::System => gtk::Settings::default().is_some_and(|s| {
                s.gtk_interface_color_scheme() == gtk::InterfaceColorScheme::Light
            }),
        }
    }
    fn style_window(&self, w: &gtk::ApplicationWindow) {
        if self.light() {
            w.add_css_class("light");
        } else {
            w.remove_css_class("light");
        }
    }
    fn reduced_motion(&self) -> bool {
        self.settings.borrow().general.reduced_motion
            || gtk::Settings::default().is_some_and(|s| !s.is_gtk_enable_animations())
    }
    fn reconfigure(self: &Rc<Self>) {
        let s = self.settings.borrow().clone();
        self.sync_appearance_style(&s);
        let reduced = self.reduced_motion();
        for dock in self.docks() {
            if s.dock.edge != dock.dock.edge() {
                dock.close_cards();
                dock.close_popup();
            }
            dock.dock
                .configure(&s.dock, reduced, !self.path.is_symlink());
        }
        for w in self.app.windows() {
            if self.light() {
                w.add_css_class("light");
            } else {
                w.remove_css_class("light");
            }
        }
        self.sync_outputs();
        self.sync_preferences();
    }
    fn sync_appearance_style(&self, settings: &Settings) {
        let values = (
            settings.panel.transparency_percent,
            settings.dock.transparency_percent,
            settings.theme.symbolic_icon_weight,
        );
        if self.appearance_style.replace(values) == values {
            return;
        }
        let mut css = String::new();
        let stroke = match values.2 {
            SymbolicIconWeight::Default => None,
            SymbolicIconWeight::Medium => Some(0.45),
            SymbolicIconWeight::Bold => Some(0.9),
        };
        for (name, transparency) in [("panel", values.0), ("dock", values.1)] {
            if transparency > 0 {
                let coverage = 100 - transparency;
                let contrast = format!("var(--mosaic-{name}-contrast)");
                css.push_str(&format!(
                    "window.mosaic-{name} {{ --mosaic-{name}-coverage: {coverage}%; \
                     --mosaic-{name}-shadow: 0 1px 1px {contrast}, 0 -1px 1px {contrast}, \
                     1px 0 1px {contrast}, -1px 0 1px {contrast}; }}\n"
                ));
            }
            if let Some(stroke) = stroke {
                // Legacy symbolic assets use filled paths. Expand their silhouette
                // in CSS so the same control works with these and with outlined icons.
                let contrast_shadow = if transparency > 0 {
                    format!(", var(--mosaic-{name}-shadow)")
                } else {
                    String::new()
                };
                css.push_str(&format!(
                    "window.mosaic-{name} .mosaic-symbolic image {{ -gtk-icon-shadow: \
                     0 {stroke}px currentColor, 0 -{stroke}px currentColor, \
                     {stroke}px 0 currentColor, -{stroke}px 0 currentColor{contrast_shadow}; }}\n"
                ));
            }
        }
        self.appearance_css.load_from_string(&css);
    }
    fn sync_preferences(&self) {
        let s = self.settings.borrow();
        self.syncing.set(true);
        if let Some(quick) = self.quick_appearance.borrow().as_ref() {
            let managed = self.path.is_symlink();
            quick.managed_note.set_visible(managed);
            quick.controls.set_sensitive(!managed);
            quick.error.set_visible(false);
            quick.appearance.sync(&s);
        }
        if let Some(p) = self.preferences.borrow().as_ref() {
            self.sync_full_preferences(p, &s);
        }
        self.syncing.set(false);
    }
    fn sync_full_preferences(&self, p: &Preferences, s: &Settings) {
        p.error.set_visible(false);
        p.appearance.sync(s);
        p.dock_edge.set_selected(match s.dock.edge {
            DockEdge::Bottom => 0,
            DockEdge::Left => 1,
            DockEdge::Right => 2,
        });
        p.preferred.set_text(&s.dock.preferred_output);
        p.current_output_only.set_active(s.dock.current_output_only);
        p.all_outputs.set_active(s.dock.all_outputs);
        p.auto_hide.set_active(s.dock.auto_hide);
        p.magnification.set_active(s.dock.magnification);
        let hover = &s.dock.hover;
        for (spin, value) in p.hover_values.iter().zip([
            hover.zoom_percent,
            hover.radius_percent,
            hover.background_percent,
            hover.grow_ms,
            hover.shrink_ms,
            hover.cards_show_delay_ms,
            hover.cards_hide_delay_ms,
        ]) {
            spin.set_value(f64::from(value));
        }
        let easing = HOVER_EASINGS
            .iter()
            .position(|(curve, _, _)| *curve == hover.easing)
            .unwrap_or(1);
        p.hover_easing.set_selected(easing as u32);
        p.hover_cards.set_active(hover.cards_enabled);
        p.minimize.set_active(s.dock.minimize_on_click);
        p.size.set_value(f64::from(s.dock.icon_size));
        p.visual_size
            .set_value(f64::from(s.dock.icon_visual_percent));
        p.icon_filter.set_selected(
            ICON_FILTERS
                .iter()
                .position(|(filter, _, _)| *filter == s.dock.icon_filter)
                .unwrap_or(0) as u32,
        );
        p.show_delay.set_value(s.dock.show_delay_ms as f64);
        p.hide_delay.set_value(s.dock.hide_delay_ms as f64);
        p.panel_transparency
            .set_value(f64::from(s.panel.transparency_percent));
        p.dock_transparency
            .set_value(f64::from(s.dock.transparency_percent));
        p.panel.sync(&s.panel);
    }
    fn edit_panel(self: &Rc<Self>, edit: LayoutEdit) {
        if self.syncing.get() {
            return;
        }
        let mut panel = self.settings.borrow().panel.clone();
        match edit {
            LayoutEdit::Move(item, direction) => {
                let Some(index) = panel.order.iter().position(|id| *id == item) else {
                    return;
                };
                let Some(next) = index.checked_add_signed(direction) else {
                    return;
                };
                if next >= panel.order.len() {
                    return;
                }
                panel.order.swap(index, next);
            }
            LayoutEdit::Visible(item, visible) => {
                if !item.can_hide() {
                    return;
                }
                panel.hidden.retain(|id| *id != item);
                if visible {
                    if !panel.order.contains(&item) {
                        panel.order.push(item);
                    }
                } else {
                    panel.hidden.push(item);
                }
            }
            LayoutEdit::Reset => {
                let defaults = settings::Panel::default();
                panel.order = defaults.order;
                panel.hidden = defaults.hidden;
            }
        }
        self.change(&format!("[panel]\n{}", toml::to_string(&panel).unwrap()));
    }
    fn action(&self, w: &Window, action: &str) {
        if action == "activate" {
            self.close_dialog();
        }
        self.release_dock_focus();
        if let Err(e) = self.tracker.borrow().request(w.epoch, w.id, action) {
            self.fail(&e);
        }
    }
    fn dock_windows<'a>(
        &self,
        dock: &DockView,
        windows: &'a [Window],
    ) -> impl Iterator<Item = &'a Window> + use<'a> {
        let output = self.settings.borrow().dock.current_output_only.then(|| {
            dock.monitor
                .borrow()
                .as_ref()
                .and_then(gdk::Monitor::connector)
                .map(|name| name.to_string())
                .unwrap_or_default()
        });
        windows
            .iter()
            .filter(move |window| desktop::dock_window_visible(window, output.as_deref()))
    }
    fn activate(self: &Rc<Self>, dock: &DockView, id: &str) {
        self.close_dialog();
        dock.close_cards();
        let group: Vec<_> = self
            .dock_windows(dock, &self.windows.borrow())
            .filter(|w| self.key(w) == id)
            .cloned()
            .collect();
        use desktop::DockClick;
        let decision = desktop::dock_click(&group, self.settings.borrow().dock.minimize_on_click);
        let (epoch, id, action) = match decision {
            DockClick::Launch => return self.launch(id),
            DockClick::Activate(epoch, id) => (epoch, id, "activate"),
            DockClick::Minimize(epoch, id) => (epoch, id, "minimize"),
        };
        if let Some(w) = group.iter().find(|w| (w.epoch, w.id) == (epoch, id)) {
            self.action(w, action);
        }
    }
    fn hover_cards(self: &Rc<Self>, dock: &Rc<DockView>, id: String, button: gtk::Button) {
        if !self.settings.borrow().dock.hover.cards_enabled {
            dock.close_cards();
            return;
        }
        if dock
            .cards
            .borrow()
            .as_ref()
            .is_some_and(|open| open.app == id)
        {
            if let Some(timer) = dock.cards_timer.borrow_mut().take() {
                timer.remove();
            }
            return;
        }
        dock.close_cards();
        if dock
            .popup
            .borrow()
            .as_ref()
            .is_some_and(|open| open.popup.is_visible())
        {
            return;
        }
        let weak = Rc::downgrade(self);
        let owner = Rc::downgrade(dock);
        let button = button.downgrade();
        let delay = u64::from(self.settings.borrow().dock.hover.cards_show_delay_ms);
        let timer =
            glib::timeout_add_local_once(std::time::Duration::from_millis(delay), move || {
                if let (Some(v), Some(dock)) = (weak.upgrade(), owner.upgrade()) {
                    dock.cards_timer.borrow_mut().take();
                    if let Some(button) = button.upgrade().filter(|b| b.is_mapped()) {
                        v.show_cards(&dock, &id, &button);
                    }
                }
            });
        *dock.cards_timer.borrow_mut() = Some(timer);
    }

    fn cards_hide_delay(&self) -> u64 {
        u64::from(self.settings.borrow().dock.hover.cards_hide_delay_ms)
    }

    fn refresh_card_previews(
        self: &Rc<Self>,
        dock: &Rc<DockView>,
        popup: &gtk::Popover,
        scroll: &gtk::ScrolledWindow,
    ) {
        let view = Rc::downgrade(self);
        let dock = Rc::downgrade(dock);
        let popup = popup.downgrade();
        let scroll = scroll.downgrade();
        glib::spawn_future_local(async move {
            loop {
                glib::timeout_future(Duration::from_millis(120)).await;
                let (Some(view), Some(dock), Some(popup), Some(scroll)) = (
                    view.upgrade(),
                    dock.upgrade(),
                    popup.upgrade(),
                    scroll.upgrade(),
                ) else {
                    break;
                };
                let candidate =
                    {
                        let open = dock.cards.borrow();
                        let Some(open) = open.as_ref().filter(|open| open.popup == popup) else {
                            break;
                        };
                        let tracker = view.tracker.borrow();
                        let mut candidate = None;
                        for row in &open.rows {
                            let identifier = row
                                .visible_in(&scroll)
                                .then(|| tracker.preview_identifier(row.key))
                                .flatten();
                            if row.preview.invalidate(identifier) {
                                row.describe();
                            }
                            if candidate.is_none()
                                && !view.preview_busy.get()
                                && let Some(identifier) = identifier
                                && row.preview.last_attempt.get().is_none_or(|time| {
                                    time.elapsed() >= Duration::from_millis(1500)
                                })
                            {
                                row.preview.last_attempt.set(Some(Instant::now()));
                                candidate = Some((row.key, identifier.to_owned()));
                            }
                        }
                        candidate
                    };
                let Some((key, identifier)) = candidate else {
                    continue;
                };
                view.preview_busy.set(true);
                let requested = identifier.clone();
                let result =
                    gio::spawn_blocking(move || crate::dock_preview::capture(&requested)).await;
                view.preview_busy.set(false);
                let cards = dock.cards.borrow();
                let Some(row) = cards
                    .as_ref()
                    .filter(|open| open.popup == popup)
                    .and_then(|open| open.rows.iter().find(|row| row.key == key))
                else {
                    break;
                };
                if !row.visible_in(&scroll)
                    || view.tracker.borrow().preview_identifier(key) != Some(identifier.as_str())
                {
                    continue;
                }
                if let Ok(Some(bytes)) = result
                    && let Some(texture) = crate::dock_preview::texture(bytes)
                {
                    row.preview.display(identifier, &texture);
                    row.describe();
                }
            }
        });
    }

    fn show_cards(self: &Rc<Self>, dock: &Rc<DockView>, id: &str, button: &gtk::Button) {
        if !self.settings.borrow().dock.hover.cards_enabled {
            return;
        }
        let mut windows: Vec<_> = self
            .dock_windows(dock, &self.windows.borrow())
            .filter(|w| self.key(w) == id)
            .cloned()
            .collect();
        if windows.is_empty() {
            return;
        }
        windows.sort_by_key(|w| (w.epoch, w.id));
        let popover = gtk::Popover::builder()
            .autohide(false)
            .position(dock.dock.popup_position())
            .build();
        popover.add_css_class("mosaic-popup");
        popover.add_css_class("dock-cards");
        if self.light() {
            popover.add_css_class("light");
        }
        popover.set_parent(button);
        let list = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let app = self.app_entry(id);
        let capture_available = self.tracker.borrow().preview_available();
        let mut rows = Vec::with_capacity(windows.len());
        for window in &windows {
            let card = gtk::Button::new();
            card.add_css_class("dock-window-card");
            let frame = gtk::Box::new(gtk::Orientation::Vertical, 0);
            let titlebar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            titlebar.add_css_class("dock-window-titlebar");
            let title = label("");
            title.set_xalign(0.);
            title.set_hexpand(true);
            title.set_max_width_chars(17);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            titlebar.append(&title);
            let status = label("");
            status.add_css_class("dim-label");
            titlebar.append(&status);
            frame.append(&titlebar);
            let contents = gtk::Box::new(gtk::Orientation::Vertical, 5);
            contents.add_css_class("dock-window-content");
            let fallback_icon = image(
                app.map(|app| &app.info),
                id,
                app.map_or(&window.title, |app| &app.name),
                28,
                false,
            );
            fallback_icon.set_vexpand(true);
            fallback_icon.set_valign(gtk::Align::Center);
            contents.append(&fallback_icon);
            let preview = CardPreview::new(&contents, capture_available);
            frame.append(&preview.stack);
            card.set_child(Some(&frame));
            let weak = Rc::downgrade(self);
            let owner = Rc::downgrade(dock);
            let key = (window.epoch, window.id);
            let row = WindowCard {
                key,
                button: card.clone(),
                title,
                status,
                preview,
            };
            row.update(window);
            rows.push(row);
            card.connect_clicked(move |_| {
                if let (Some(v), Some(dock)) = (weak.upgrade(), owner.upgrade()) {
                    dock.close_cards();
                    if let Some(w) = v
                        .dock_windows(&dock, &v.windows.borrow())
                        .find(|w| (w.epoch, w.id) == key)
                    {
                        v.action(w, "activate");
                    }
                }
            });
            list.append(&card);
        }
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(230)
            .width_request(260)
            .max_content_height(270)
            .propagate_natural_height(true)
            .child(&list)
            .build();
        popover.set_child(Some(&scroll));
        let motion = gtk::EventControllerMotion::new();
        let weak = Rc::downgrade(dock);
        motion.connect_enter(move |_, _, _| {
            if let Some(v) = weak.upgrade()
                && let Some(timer) = v.cards_timer.borrow_mut().take()
            {
                timer.remove();
            }
        });
        let weak = Rc::downgrade(dock);
        let view = Rc::downgrade(self);
        motion.connect_leave(move |_| {
            if let (Some(dock), Some(view)) = (weak.upgrade(), view.upgrade()) {
                dock.defer_cards_close(view.cards_hide_delay());
            }
        });
        popover.add_controller(motion);
        let weak = Rc::downgrade(dock);
        popover.connect_closed(move |p| {
            if let Some(v) = weak.upgrade() {
                let same = v
                    .cards
                    .borrow()
                    .as_ref()
                    .is_some_and(|open| open.popup == *p);
                if same {
                    v.close_cards();
                }
            }
        });
        *dock.cards.borrow_mut() = Some(WindowCards {
            app: id.into(),
            rows,
            popup: popover.clone(),
        });
        dock.dock.menu(true);
        popover.popup();
        if capture_available {
            self.refresh_card_previews(dock, &popover, &scroll);
        }
    }

    fn dock_icon_key<'a>(&'a self, id: &'a str) -> Option<&'a str> {
        let key = self
            .app_entry(id)
            .map_or(id, |entry| entry.model.id.as_str());
        crate::dock_icons::valid_key(key).then_some(key)
    }

    fn choose_dock_icon(self: &Rc<Self>, parent: gtk::ApplicationWindow, key: String) {
        if self.path.is_symlink() {
            self.show_error(&ApiError::PermissionDenied("managed configuration".into()));
            return;
        }
        self.release_dock_focus();
        if gio::SettingsSchemaSource::default()
            .and_then(|source| source.lookup("org.gtk.gtk4.Settings.FileChooser", true))
            .is_none()
        {
            self.dock_icon_error(&parent, "Для выбора файла нужны схемы настроек GTK 4.");
            return;
        }
        let filter = gtk::FileFilter::new();
        filter.set_name(Some("Изображения PNG"));
        filter.add_mime_type("image/png");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        let chooser = gtk::FileDialog::builder()
            .title("Выбрать значок Dock")
            .accept_label("Выбрать")
            .filters(&filters)
            .default_filter(&filter)
            .modal(true)
            .build();
        let config = self.path.clone();
        let view = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = chooser.open_future(Some(&parent)).await;
            let imported = match result {
                Ok(file) => {
                    let Some(source) = file.path() else {
                        if let Some(view) = view.upgrade() {
                            view.dock_icon_error(&parent, "Выберите локальный файл PNG.");
                        }
                        return;
                    };
                    gio::spawn_blocking(move || crate::dock_icons::import_png(&config, &source))
                        .await
                }
                Err(error)
                    if error.matches(gtk::DialogError::Dismissed)
                        || error.matches(gtk::DialogError::Cancelled) =>
                {
                    return;
                }
                Err(_) => {
                    if let Some(view) = view.upgrade() {
                        view.dock_icon_error(&parent, "Не удалось открыть окно выбора значка.");
                    }
                    return;
                }
            };
            let Some(view) = view.upgrade() else { return };
            let Ok(Ok(asset)) = imported else {
                view.dock_icon_error(&parent, "Выберите PNG до 8 МиБ с размером до 4096 пикселей по каждой стороне и до 4 194 304 пикселей всего.");
                return;
            };
            let mut next = view.settings.borrow().clone();
            next.dock_icons.insert(key, asset);
            if let Err(error) = view.commit(next) {
                view.show_error(&error);
            }
        });
    }

    fn dock_icon_error(&self, parent: &gtk::ApplicationWindow, message: &str) {
        crate::diagnostics::event("shell", "dock_icon_import_failed", 1);
        self.fail(message);
        gtk::AlertDialog::builder()
            .message("Не удалось назначить значок")
            .detail(message)
            .buttons(["Закрыть"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build()
            .show(parent.is_visible().then_some(parent));
    }

    fn menu(self: &Rc<Self>, dock: &Rc<DockView>, button: &gtk::Button, id: &str) {
        dock.close_cards();
        dock.close_popup();
        let popover = gtk::Popover::new();
        popover.set_position(dock.dock.popup_position());
        popover.add_css_class("mosaic-popup");
        popover.set_has_arrow(false);
        popover.set_parent(button);
        let content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let windows = self.windows.borrow();
        let mut menu_windows = Vec::new();
        for w in self
            .dock_windows(dock, &windows)
            .filter(|w| self.key(w) == id)
        {
            menu_windows.push((w.epoch, w.id));
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 4);
            let b = gtk::Button::with_label(if w.title.is_empty() {
                "Окно"
            } else {
                &w.title
            });
            b.set_hexpand(true);
            if let Some(l) = b.child().and_downcast::<gtk::Label>() {
                l.set_max_width_chars(42);
                l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            }
            let weak = Rc::downgrade(self);
            let window = w.clone();
            let p = popover.downgrade();
            b.connect_clicked(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.action(&window, "activate");
                }
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
            });
            row.append(&b);
            let close = gtk::Button::with_label("×");
            close.update_property(&[gtk::accessible::Property::Label("Закрыть окно")]);
            let weak = Rc::downgrade(self);
            let w = w.clone();
            let p = popover.downgrade();
            close.connect_clicked(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.action(&w, "close");
                }
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
            });
            row.append(&close);
            content.append(&row);
        }
        let known = self.app_entry(id).is_some();
        let pins = self.settings.borrow().pins.clone();
        let position = pins.iter().position(|pin| pin == id);
        let pinned = position.is_some();
        let managed = self.path.is_symlink();
        if known {
            let b = gtk::Button::with_label("Открыть новое окно");
            let weak = Rc::downgrade(self);
            let id = id.to_owned();
            let p = popover.downgrade();
            b.connect_clicked(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.launch(&id);
                }
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
            });
            content.append(&b);
        }
        if known || pinned {
            let b = gtk::Button::with_label(if pinned {
                "Открепить"
            } else {
                "Закрепить"
            });
            b.set_sensitive(!managed);
            let weak = Rc::downgrade(self);
            let id = id.to_owned();
            let p = popover.downgrade();
            b.connect_clicked(move |_| {
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
                if let Some(v) = weak.upgrade() {
                    v.pin(&id, !pinned);
                }
            });
            content.append(&b);
        }
        if managed && (known || pinned) {
            let status = label("Закрепления заданы в конфигурации");
            status.add_css_class("dim-label");
            content.append(&status);
        } else if let Some(position) = position {
            let (previous, next) = if dock.dock.edge().vertical() {
                ("Переместить вверх", "Переместить вниз")
            } else {
                ("Переместить влево", "Переместить вправо")
            };
            let moves = [
                (
                    previous,
                    position.checked_sub(1).map(|index| pins[index].as_str()),
                ),
                (
                    next,
                    (position + 1 < pins.len())
                        .then(|| pins.get(position + 2).map_or("", String::as_str)),
                ),
            ];
            for (text, before) in moves {
                let Some(before) = before else { continue };
                let button = gtk::Button::with_label(text);
                let weak = Rc::downgrade(self);
                let source = id.to_owned();
                let before = before.to_owned();
                let p = popover.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(p) = p.upgrade() {
                        p.popdown();
                    }
                    if let Some(view) = weak.upgrade() {
                        view.reorder_pin(&source, &before);
                    }
                });
                content.append(&button);
            }
        }
        if let Some(key) = self.dock_icon_key(id).map(str::to_owned) {
            let custom = self.settings.borrow().dock_icons.contains_key(&key);
            content.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let item = gtk::Button::with_label("Выбрать значок…");
            item.set_sensitive(!managed);
            let weak = Rc::downgrade(self);
            let parent = dock.dock.window().downgrade();
            let p = popover.downgrade();
            let choose_key = key.clone();
            item.connect_clicked(move |_| {
                if let Some(p) = p.upgrade() {
                    p.popdown();
                }
                if let (Some(view), Some(parent)) = (weak.upgrade(), parent.upgrade()) {
                    view.choose_dock_icon(parent, choose_key.clone());
                }
            });
            content.append(&item);
            if custom {
                let item = gtk::Button::with_label("Восстановить значок");
                item.set_sensitive(!managed);
                let weak = Rc::downgrade(self);
                let p = popover.downgrade();
                item.connect_clicked(move |_| {
                    if let Some(p) = p.upgrade() {
                        p.popdown();
                    }
                    if let Some(view) = weak.upgrade() {
                        let mut next = view.settings.borrow().clone();
                        next.dock_icons.remove(&key);
                        if let Err(error) = view.commit(next) {
                            view.show_error(&error);
                        }
                    }
                });
                content.append(&item);
            }
        }
        popover.set_child(Some(&content));
        let weak = Rc::downgrade(dock);
        popover.connect_closed(move |p| {
            if let Some(v) = weak.upgrade() {
                let same = v
                    .popup
                    .borrow()
                    .as_ref()
                    .is_some_and(|open| open.popup == *p);
                if same {
                    v.close_popup();
                }
            }
        });
        *dock.popup.borrow_mut() = Some(DockMenu {
            app: id.into(),
            windows: menu_windows,
            popup: popover.clone(),
        });
        dock.dock.menu(true);
        popover.popup();
    }
    fn redraw_dock(&self, dock: &DockView, windows: &[Window]) {
        if !self.settings.borrow().dock.hover.cards_enabled {
            dock.close_cards();
        }
        let dock_windows: Vec<_> = self.dock_windows(dock, windows).collect();
        let stale_cards = dock.cards.borrow().as_ref().is_some_and(|open| {
            let mut group: Vec<_> = dock_windows
                .iter()
                .copied()
                .filter(|w| self.key(w) == open.app)
                .collect();
            group.sort_by_key(|w| (w.epoch, w.id));
            if !group
                .iter()
                .map(|w| (w.epoch, w.id))
                .eq(open.rows.iter().map(|row| row.key))
            {
                return true;
            }
            for (row, window) in open.rows.iter().zip(group) {
                row.preview
                    .invalidate(self.tracker.borrow().preview_identifier(row.key));
                row.update(window);
            }
            false
        });
        if stale_cards {
            dock.close_cards();
        }
        let stale_menu = dock.popup.borrow().as_ref().is_some_and(|open| {
            !dock_windows
                .iter()
                .copied()
                .filter(|w| self.key(w) == open.app)
                .map(|w| (w.epoch, w.id))
                .eq(open.windows.iter().copied())
        });
        if stale_menu {
            dock.close_popup();
        }
        let running: Vec<_> = dock_windows.iter().map(|w| self.key(w)).collect();
        let settings = self.settings.borrow();
        let pins = &settings.pins;
        let items = desktop::dock_order(pins, &running)
            .into_iter()
            .map(|id| {
                let entry = self.app_entry(&id);
                let group: Vec<_> = dock_windows
                    .iter()
                    .copied()
                    .filter(|w| self.key(w) == id)
                    .collect();
                let name = entry.map(|a| a.name.as_str()).unwrap_or_else(|| {
                    if id == "window:org.mosaic.Shell.Ui" {
                        "Mosaic"
                    } else if let Some(window) =
                        group.first().filter(|window| window.app_id.is_empty())
                    {
                        &window.title
                    } else {
                        id.trim_start_matches("window:")
                    }
                });
                dock::Item {
                    name: name.into(),
                    pinned: pins.contains(&id),
                    custom_icon: self
                        .dock_icon_key(&id)
                        .and_then(|key| settings.dock_icons.get(key))
                        .cloned(),
                    windows: group.len(),
                    active: group.iter().any(|w| w.active),
                    pending: self
                        .pending
                        .borrow()
                        .contains(entry.map_or(&id, |app| &app.model.id)),
                    id,
                }
            })
            .collect();
        dock.dock.update(
            items,
            |item, size| {
                if let Some(pixels) = item
                    .custom_icon
                    .as_deref()
                    .and_then(|asset| crate::dock_icons::load_png(&self.path, asset, 160))
                {
                    return pixbuf_image(pixels);
                }
                image(
                    self.app_entry(&item.id).map(|app| &app.info),
                    &item.id,
                    &item.name,
                    size,
                    true,
                )
            },
            || {
                dock.close_cards();
                dock.close_popup();
            },
        );
    }
    fn redraw(self: &Rc<Self>) {
        let windows = self.windows.borrow();
        let only = self.panels.borrow().len() == 1;
        let on_output = |w: &Window, name: &str| {
            w.outputs.iter().any(|n| n == name) || (only && w.outputs.is_empty())
        };
        for dock in self.docks() {
            self.redraw_dock(&dock, &windows);
            let monitor = dock.monitor.borrow().clone();
            let selected = monitor
                .as_ref()
                .and_then(gdk::Monitor::connector)
                .unwrap_or_default();
            let fullscreen = monitor.is_some()
                && windows
                    .iter()
                    .any(|w| w.active && w.fullscreen && on_output(w, &selected));
            if fullscreen {
                dock.close_cards();
                dock.close_popup();
            }
            dock.dock.fullscreen(fullscreen);
        }
        for panel in self.panels.borrow().iter() {
            let connector = panel.connector();
            panel.window.set_visible(
                !windows
                    .iter()
                    .any(|w| w.active && w.fullscreen && on_output(w, &connector)),
            );
            if let Some(w) = windows
                .iter()
                .find(|w| w.active && on_output(w, &connector))
            {
                let key = self.key(w);
                panel
                    .active
                    .set_text(self.app_entry(&key).map(|a| a.name.as_str()).unwrap_or(
                        if w.app_id == "org.mosaic.Shell.Ui" {
                            "Mosaic"
                        } else {
                            &w.app_id
                        },
                    ));
            } else {
                panel.active.set_text("Рабочий стол");
            }
        }
    }

    fn dialog_monitor(&self, source: Option<&gdk::Monitor>, hint: &str) -> Option<gdk::Monitor> {
        let panels = self.panels.borrow();
        if let Some(source) = source
            && panels
                .iter()
                .any(|p| p.monitor == *source && p.monitor.is_valid())
        {
            return Some(source.clone());
        }
        let names: Vec<_> = panels
            .iter()
            .filter(|p| p.monitor.is_valid())
            .map(|p| p.connector())
            .collect();
        let windows = self.windows.borrow();
        let active = windows
            .iter()
            .find(|w| w.active && !w.minimized)
            .map(|w| w.outputs.as_slice())
            .unwrap_or_default();
        crate::outputs::select_dialog_output(
            hint,
            active,
            &self.settings.borrow().dock.preferred_output,
            &names,
        )
        .and_then(|name| {
            panels
                .iter()
                .find(|p| p.connector() == name)
                .map(|p| p.monitor.clone())
        })
        .or_else(|| {
            self.primary_dock()
                .monitor
                .borrow()
                .as_ref()
                .filter(|monitor| monitor.is_valid())
                .cloned()
        })
    }
    fn close_dialog(&self) {
        let window = self.dialog.borrow().as_ref().map(|d| d.window.clone());
        if let Some(window) = window {
            window.close();
        }
    }
    fn dialog_info(&self) -> DialogInfo {
        let dialog = self.dialog.borrow();
        let Some(dialog) = dialog.as_ref() else {
            return DialogInfo::default();
        };
        let connector = dialog
            .window
            .surface()
            .and_then(|surface| surface.display().monitor_at_surface(&surface))
            .and_then(|monitor| monitor.connector())
            .map(|connector| connector.to_string())
            .unwrap_or_default();
        DialogInfo {
            kind: dialog.kind.name().into(),
            connector,
            mapped: dialog.window.is_mapped(),
        }
    }
    fn dialog_window(
        self: &Rc<Self>,
        kind: DialogKind,
        monitor: &gdk::Monitor,
        title: &str,
        width: i32,
        height: i32,
    ) -> Option<(gtk::ApplicationWindow, gtk::Box)> {
        if self
            .dialog
            .borrow()
            .as_ref()
            .is_some_and(|d| d.kind == kind && d.monitor == *monitor && d.window.is_visible())
        {
            return None;
        }
        self.close_dialog();
        self.release_dock_focus();
        for dock in self.docks() {
            dock.close_cards();
            dock.close_popup();
        }
        self.tray.close_popovers();
        for panel in self.panels.borrow().iter() {
            panel.close_calendar();
        }
        let geometry = monitor.geometry();
        let panel_zone = self
            .panels
            .borrow()
            .iter()
            .find(|panel| panel.monitor == *monitor && panel.window.is_mapped())
            .map_or(0, |panel| panel.window.exclusive_zone().max(0));
        let dock_zone = self
            .dock_on(monitor)
            .filter(|d| d.dock.window().is_mapped())
            .map_or(0, |d| d.dock.window().exclusive_zone().max(0));
        let (reserved_width, reserved_height) = if self.primary_dock().dock.edge().vertical() {
            (dock_zone, panel_zone)
        } else {
            (0, panel_zone + dock_zone)
        };
        let available_width = (geometry.width() - reserved_width - 24).max(1);
        let available_height = (geometry.height() - reserved_height - 24).max(1);
        let window = gtk::ApplicationWindow::builder()
            .application(&self.app)
            .title(title)
            .decorated(false)
            .default_width(width.min(available_width))
            .default_height(height.min(available_height))
            .build();
        window.init_layer_shell();
        window.set_namespace(Some("mosaic-dialog"));
        window.set_layer(Layer::Overlay);
        window.set_exclusive_zone(0);
        window.set_keyboard_mode(KeyboardMode::Exclusive);
        window.set_respect_close(true);
        window.set_monitor(Some(monitor));
        window.add_css_class("mosaic-window");
        self.style_window(&window);
        let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let header = gtk::HeaderBar::new();
        header.set_show_title_buttons(false);
        header.set_title_widget(Some(&label(title)));
        let close = gtk::Button::from_icon_name("window-close-symbolic");
        close.set_tooltip_text(Some("Закрыть"));
        close.update_property(&[gtk::accessible::Property::Label("Закрыть")]);
        let weak = window.downgrade();
        close.connect_clicked(move |_| {
            if let Some(window) = weak.upgrade() {
                window.close();
            }
        });
        header.pack_end(&close);
        body.append(&header);
        window.set_child(Some(&body));
        let escape = gtk::EventControllerKey::new();
        escape.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = window.downgrade();
        escape.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                if let Some(window) = weak.upgrade() {
                    window.close();
                }
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(escape);
        let weak = Rc::downgrade(self);
        window.connect_close_request(move |window| {
            if let Some(view) = weak.upgrade() {
                if view
                    .dialog
                    .borrow()
                    .as_ref()
                    .is_some_and(|d| d.window == *window)
                {
                    let closed = view.dialog.borrow_mut().take();
                    match closed.map(|dialog| dialog.kind) {
                        Some(DialogKind::ControlCenter) => {
                            view.quick_appearance.borrow_mut().take();
                        }
                        Some(DialogKind::Wifi) => {
                            view.wifi_pane.borrow_mut().take();
                        }
                        Some(DialogKind::Bluetooth) => {
                            view.bluetooth_pane.borrow_mut().take();
                        }
                        Some(DialogKind::Brightness) => {
                            view.brightness_pane.borrow_mut().take();
                        }
                        Some(DialogKind::Temperatures) => {
                            view.temperature_pane.borrow_mut().take();
                            view.sync_temperature_poll();
                        }
                        Some(DialogKind::Power) => {
                            view.power_pane.borrow_mut().take();
                        }
                        Some(DialogKind::Audio) => {
                            view.audio_pane.borrow_mut().take();
                        }
                        _ => (),
                    }
                }
                if view
                    .preferences
                    .borrow()
                    .as_ref()
                    .is_some_and(|p| p.window == *window)
                {
                    view.preferences.borrow_mut().take();
                }
            }
            glib::Propagation::Proceed
        });
        *self.dialog.borrow_mut() = Some(ShellDialog {
            kind,
            monitor: monitor.clone(),
            window: window.clone(),
        });
        Some((window, body))
    }
    fn launcher(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Launcher, &monitor, "Приложения", 560, 600)
        else {
            return;
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        let search = gtk::SearchEntry::new();
        search.set_placeholder_text(Some("Найти приложение"));
        content.append(&search);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        let mut rows = Vec::new();
        for entry in &self.apps {
            let b = gtk::Button::new();
            b.add_css_class("catalog-button");
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.append(&image(
                Some(&entry.info),
                &entry.model.id,
                &entry.name,
                32,
                false,
            ));
            row.append(&label(&entry.name));
            b.set_child(Some(&row));
            let weak = Rc::downgrade(self);
            let id = entry.model.id.clone();
            let w = window.downgrade();
            b.connect_clicked(move |_| {
                if let Some(v) = weak.upgrade() {
                    v.launch(&id);
                }
                if let Some(w) = w.upgrade() {
                    w.close();
                }
            });
            let item = gtk::ListBoxRow::new();
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            b.set_hexpand(true);
            actions.append(&b);
            if !entry.info.supported_types().is_empty() {
                actions.append(&Self::default_application_menu(&entry.info, &entry.name));
            }
            item.set_child(Some(&actions));
            list.append(&item);
            rows.push((entry.name.to_lowercase(), item));
        }
        search.connect_search_changed(move |entry| {
            let query = entry.text().to_lowercase();
            for (name, row) in &rows {
                row.set_visible(name.contains(&query));
            }
        });
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .child(&list)
            .build();
        content.append(&scroll);
        body.append(&content);
        window.present();
        search.grab_focus();
    }

    fn default_application_menu(info: &gio::AppInfo, name: &str) -> gtk::MenuButton {
        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .valign(gtk::Align::Center)
            .tooltip_text(format!("Назначить по умолчанию: {name}"))
            .build();
        let info = info.clone();
        menu.set_create_popup_func(move |menu| {
            let popover = gtk::Popover::new();
            let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
            content.append(&label("Открывать этим приложением по умолчанию"));

            let mut types = info.supported_types().to_vec();
            types.sort_unstable();
            types.dedup();
            let choices = gtk::StringList::new(&[]);
            for mime in &types {
                choices.append(mime);
            }
            let select = gtk::DropDown::builder().model(&choices).build();
            select.set_enable_search(true);
            content.append(&select);

            let status = label("");
            status.set_wrap(true);
            let save = gtk::Button::with_label("Назначить");
            let app = info.clone();
            let feedback = status.clone();
            save.connect_clicked(move |_| {
                let Some(mime) = types.get(select.selected() as usize) else {
                    return;
                };
                match app.set_as_default_for_type(mime) {
                    Ok(())
                        if gio::AppInfo::default_for_type(mime, false)
                            .is_some_and(|current| current.equal(&app)) =>
                    {
                        feedback.set_text("Приложение по умолчанию обновлено");
                    }
                    Ok(()) => feedback.set_text("Действует другая настройка для этого типа"),
                    Err(error) => feedback.set_text(&format!("Не удалось сохранить: {error}")),
                }
            });
            content.append(&save);
            content.append(&status);
            popover.set_child(Some(&content));
            menu.set_popover(Some(&popover));
        });
        menu
    }
    fn settings_row(name: &str, widget: &impl IsA<gtk::Widget>) -> gtk::Box {
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        let l = label(name);
        l.set_xalign(0.);
        l.set_hexpand(true);
        row.append(&l);
        row.append(widget);
        widget
            .as_ref()
            .update_property(&[gtk::accessible::Property::Label(name)]);
        row
    }
    fn appearance(self: &Rc<Self>) -> Appearance {
        let theme = gtk::DropDown::from_strings(&["Тёмная", "Светлая", "Как в системе"]);
        let weak = Rc::downgrade(self);
        theme.connect_selected_notify(move |theme| {
            if let Some(view) = weak.upgrade() {
                let mode = match theme.selected() {
                    0 => "dark",
                    1 => "light",
                    2 => "system",
                    _ => return,
                };
                view.change(&format!("theme.mode='{mode}'"));
            }
        });
        let symbolic_weight =
            gtk::DropDown::from_strings(&SYMBOLIC_WEIGHTS.map(|(_, _, label)| label));
        let weak = Rc::downgrade(self);
        symbolic_weight.connect_selected_notify(move |widget| {
            if let Some(view) = weak.upgrade()
                && let Some((_, key, _)) = SYMBOLIC_WEIGHTS.get(widget.selected() as usize)
            {
                view.change(&format!("theme.symbolic_icon_weight='{key}'"));
            }
        });
        let reduced = gtk::Switch::new();
        let weak = Rc::downgrade(self);
        reduced.connect_active_notify(move |toggle| {
            if let Some(view) = weak.upgrade() {
                view.change(&format!("general.reduced_motion={}", toggle.is_active()));
            }
        });
        Appearance {
            theme,
            symbolic_weight,
            reduced,
        }
    }

    fn toggle_control_center(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let kind = DialogKind::ControlCenter;
        if self.dialog.borrow().as_ref().is_some_and(|dialog| {
            dialog.kind == kind && dialog.monitor == monitor && dialog.window.is_visible()
        }) {
            self.close_dialog();
            return;
        }
        let Some((window, body)) = self.dialog_window(kind, &monitor, "Центр управления", 450, 610)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let wifi = gtk::Button::with_label("Wi-Fi");
            let weak = Rc::downgrade(self);
            let source = monitor.clone();
            wifi.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.show_wifi(Some(&source), "");
                }
            });
            header.pack_start(&wifi);
            let brightness = gtk::Button::with_label("Яркость");
            let weak = Rc::downgrade(self);
            let source = monitor.clone();
            brightness.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.show_brightness(Some(&source), "");
                }
            });
            header.pack_start(&brightness);
            let bluetooth = gtk::Button::with_label("Bluetooth");
            let weak = Rc::downgrade(self);
            let source = monitor.clone();
            bluetooth.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.show_bluetooth(Some(&source), "");
                }
            });
            header.pack_end(&bluetooth);
        }

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(20);
        content.set_margin_end(20);
        let managed_note = label("Настройки управляются декларативной конфигурацией.");
        managed_note.set_wrap(true);
        managed_note.set_xalign(0.);
        content.append(&managed_note);
        let controls = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let appearance = self.appearance();
        controls.append(&Self::settings_row("Тема", &appearance.theme));
        controls.append(&Self::settings_row(
            "Толщина символических значков",
            &appearance.symbolic_weight,
        ));
        controls.append(&Self::settings_row(
            "Уменьшить движение",
            &appearance.reduced,
        ));
        content.append(&controls);
        let error = label("");
        error.set_wrap(true);
        error.set_xalign(0.);
        content.append(&error);
        let temperatures = gtk::Button::with_label("Температуры");
        let weak = Rc::downgrade(self);
        let source = monitor.clone();
        temperatures.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.show_temperatures(Some(&source), "");
            }
        });

        let full = gtk::Button::with_label("Все настройки");
        full.set_halign(gtk::Align::End);
        let weak = Rc::downgrade(self);
        let source = monitor.clone();
        full.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.settings(Some(&source));
            }
        });
        content.append(&full);
        let system_pages = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        temperatures.set_hexpand(true);
        system_pages.append(&temperatures);
        let power = gtk::Button::with_label("Питание");
        power.set_hexpand(true);
        let weak = Rc::downgrade(self);
        let source = monitor.clone();
        power.connect_clicked(move |_| {
            if let Some(view) = weak.upgrade() {
                view.show_power(Some(&source), "");
            }
        });
        system_pages.append(&power);
        content.append(&system_pages);
        let audio = crate::audio_ui::AudioPane::new(true);
        content.append(audio.widget());
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&content)
            .build();
        body.append(&scroll);

        let focus = if self.path.is_symlink() {
            full.upcast::<gtk::Widget>()
        } else {
            appearance.theme.clone().upcast()
        };
        *self.quick_appearance.borrow_mut() = Some(QuickAppearance {
            appearance,
            controls,
            managed_note,
            error,
            _audio: audio,
        });
        self.sync_preferences();
        window.present();
        focus.grab_focus();
    }

    fn show_wifi(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Wifi, &monitor, "Wi-Fi", 450, 470)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let back = gtk::Button::with_label("Назад");
            let weak = Rc::downgrade(self);
            let source = monitor.clone();
            back.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.toggle_control_center(Some(&source), "");
                }
            });
            header.pack_start(&back);
        }
        let pane = crate::network_ui::WifiPane::new();
        body.append(pane.widget());
        *self.wifi_pane.borrow_mut() = Some(pane);
        window.present();
    }

    fn show_bluetooth(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Bluetooth, &monitor, "Bluetooth", 480, 470)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let back = gtk::Button::with_label("Назад");
            let weak = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.toggle_control_center(Some(&monitor), "");
                }
            });
            header.pack_start(&back);
        }
        let pane = crate::bluetooth_ui::BluetoothPane::new();
        body.append(&pane.widget);
        self.bluetooth_pane.replace(Some(pane));
        window.present();
    }

    fn show_brightness(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Brightness, &monitor, "Яркость", 450, 390)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let back = gtk::Button::with_label("Назад");
            let weak = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.toggle_control_center(Some(&monitor), "");
                }
            });
            header.pack_start(&back);
        }
        let pane = crate::brightness_ui::BrightnessPane::new();
        body.append(&pane.widget);
        self.brightness_pane.replace(Some(pane));
        window.present();
    }

    fn show_temperatures(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Temperatures, &monitor, "Температуры", 470, 424)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let back = gtk::Button::with_label("Назад");
            let weak = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.toggle_control_center(Some(&monitor), "");
                }
            });
            header.pack_start(&back);
        }
        let rows = gtk::Box::new(gtk::Orientation::Vertical, 6);
        rows.set_margin_top(16);
        rows.set_margin_bottom(16);
        rows.set_margin_start(20);
        rows.set_margin_end(20);
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&rows)
            .build();
        body.append(&scroll);
        self.temperature_pane.replace(Some(rows));
        self.render_temperatures();
        self.sync_temperature_poll();
        window.present();
    }

    fn show_power(self: &Rc<Self>, source: Option<&gdk::Monitor>, hint: &str) {
        let Some(monitor) = self.dialog_monitor(source, hint) else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Power, &monitor, "Питание", 450, 370)
        else {
            return;
        };
        if let Some(header) = body.first_child().and_downcast::<gtk::HeaderBar>() {
            let back = gtk::Button::with_label("Назад");
            let weak = Rc::downgrade(self);
            back.connect_clicked(move |_| {
                if let Some(view) = weak.upgrade() {
                    view.toggle_control_center(Some(&monitor), "");
                }
            });
            header.pack_start(&back);
        }
        let pane = crate::power_ui::PowerPane::new();
        body.append(&pane.widget);
        self.power_pane.replace(Some(pane));
        window.present();
    }

    fn sync_temperature_poll(self: &Rc<Self>) {
        let visible = self.temperature_pane.borrow().is_some()
            || (self
                .settings
                .borrow()
                .panel
                .is_visible(PanelItem::Temperatures)
                && !self.panels.borrow().is_empty());
        if !visible {
            if let Some(timer) = self.temperature_timer.borrow_mut().take() {
                timer.remove();
            }
            self.temperature_snapshot.borrow_mut().take();
            return;
        }
        if self.temperature_timer.borrow().is_some() {
            self.render_temperature_summary();
            return;
        }
        let weak = Rc::downgrade(self);
        *self.temperature_timer.borrow_mut() =
            Some(glib::timeout_add_seconds_local(3, move || {
                let Some(view) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                view.sample_temperatures();
                glib::ControlFlow::Continue
            }));
        self.sample_temperatures();
    }

    fn sample_temperatures(self: &Rc<Self>) {
        if self.temperature_pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(temperatures::read_system).await;
            let Some(view) = weak.upgrade() else { return };
            view.temperature_pending.set(false);
            if view.temperature_timer.borrow().is_none() {
                return;
            }
            let snapshot = result.unwrap_or_default();
            if view.temperature_snapshot.borrow().as_ref() != Some(&snapshot) {
                *view.temperature_snapshot.borrow_mut() = Some(snapshot);
                view.render_temperatures();
            }
        });
    }

    fn render_temperature_summary(&self) {
        let snapshot = self.temperature_snapshot.borrow();
        let parts = [SensorKind::Cpu, SensorKind::Gpu]
            .into_iter()
            .filter_map(|kind| {
                let value = snapshot.as_ref()?.hottest(kind)?;
                Some(format!("{} {:.0}°", kind.title(), f64::from(value) / 1000.))
            })
            .collect::<Vec<_>>();
        let text = if parts.is_empty() {
            "Темп. —".into()
        } else {
            parts.join(" · ")
        };
        for panel in self.panels.borrow().iter() {
            panel.set_temperature_summary(&text);
        }
    }

    fn render_temperatures(&self) {
        self.render_temperature_summary();
        let pane = self.temperature_pane.borrow();
        let Some(rows) = pane.as_ref() else { return };
        while let Some(child) = rows.first_child() {
            rows.remove(&child);
        }
        let snapshot = self.temperature_snapshot.borrow();
        let Some(snapshot) = snapshot.as_ref() else {
            rows.append(&label("Чтение датчиков…"));
            return;
        };
        if snapshot.sensors.is_empty() {
            rows.append(&label("Доступных датчиков температуры нет"));
            return;
        }
        for kind in [
            SensorKind::Cpu,
            SensorKind::Gpu,
            SensorKind::Storage,
            SensorKind::Other,
        ] {
            let mut sensors = snapshot
                .sensors
                .iter()
                .filter(|sensor| sensor.kind == kind)
                .peekable();
            if sensors.peek().is_none() {
                continue;
            }
            let group = label(kind.title());
            group.set_xalign(0.);
            group.add_css_class("heading");
            group.set_margin_top(8);
            rows.append(&group);
            for sensor in sensors {
                let text = match sensor.reading {
                    Reading::Value { milli_celsius } => {
                        format!("{:.1} °C", f64::from(milli_celsius) / 1000.).replace('.', ",")
                    }
                    Reading::Unavailable => "Нет данных".into(),
                    Reading::Suspended => "Устройство спит".into(),
                };
                let name = format!("{} · {}", sensor.source, sensor.label);
                let name = label(&name);
                name.set_hexpand(true);
                name.set_xalign(0.);
                name.set_ellipsize(gtk::pango::EllipsizeMode::End);
                let value = label(&text);
                let row = gtk::Box::new(gtk::Orientation::Horizontal, 16);
                row.append(&name);
                row.append(&value);
                row.set_tooltip_text(Some(&sensor.id));
                rows.append(&row);
            }
        }
    }

    fn show_audio(self: &Rc<Self>, monitor: &gdk::Monitor) {
        let Some((window, body)) = self.dialog_window(DialogKind::Audio, monitor, "Звук", 450, 610)
        else {
            return;
        };
        let pane = crate::audio_ui::AudioPane::new(false);
        let content = pane.widget();
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(20);
        content.set_margin_end(20);
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(content)
            .build();
        body.append(&scroll);
        self.audio_pane.replace(Some(pane));
        window.present();
    }

    fn settings(self: &Rc<Self>, source: Option<&gdk::Monitor>) {
        self.settings_section(source, PreferencesSection::General);
    }

    fn settings_section(
        self: &Rc<Self>,
        source: Option<&gdk::Monitor>,
        section: PreferencesSection,
    ) {
        let Some(monitor) = self.dialog_monitor(source, "") else {
            return;
        };
        let Some((window, body)) =
            self.dialog_window(DialogKind::Settings, &monitor, "Настройки Mosaic", 580, 550)
        else {
            if let Some(preferences) = self.preferences.borrow().as_ref() {
                preferences.pages.set_visible_child_name(section.name());
            }
            return;
        };
        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_top(20);
        content.set_margin_bottom(20);
        content.set_margin_start(24);
        content.set_margin_end(24);
        let managed = self.path.is_symlink();
        if managed {
            let note = label("Настройки управляются декларативной конфигурацией.");
            note.set_wrap(true);
            content.append(&note);
        }
        let pages = gtk::Stack::new();
        pages.set_hhomogeneous(false);
        pages.set_vhomogeneous(false);
        pages.set_vexpand(true);
        let tabs = gtk::StackSwitcher::builder().stack(&pages).build();
        tabs.add_css_class("settings-tabs");
        tabs.set_halign(gtk::Align::Start);
        content.append(&tabs);
        content.append(&pages);
        body.append(&content);

        let appearance = self.appearance();
        let general = gtk::Box::new(gtk::Orientation::Vertical, 14);
        let reduced = Self::settings_row("Уменьшить движение", &appearance.reduced);
        reduced.set_sensitive(!managed);
        general.append(&reduced);
        let close = gtk::Button::with_label("Завершить оболочку");
        close.set_halign(gtk::Align::Start);
        let app = self.app.downgrade();
        close.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.quit();
            }
        });
        general.append(&close);
        pages.add_titled(
            &general,
            Some(PreferencesSection::General.name()),
            "Основные",
        );

        let theme = gtk::Box::new(gtk::Orientation::Vertical, 14);
        theme.append(&Self::settings_row("Тема", &appearance.theme));
        theme.append(&Self::settings_row(
            "Толщина символических значков",
            &appearance.symbolic_weight,
        ));
        theme.set_sensitive(!managed);
        pages.add_titled(
            &theme,
            Some(PreferencesSection::Appearance.name()),
            "Оформление",
        );

        let controls = gtk::Box::new(gtk::Orientation::Vertical, 14);
        controls.set_sensitive(!managed);
        let dock_transparency = gtk::SpinButton::with_range(0., 100., 1.);
        controls.append(&Self::settings_row(
            "Прозрачность фона, %",
            &dock_transparency,
        ));
        let dock_edge = gtk::DropDown::from_strings(&["Снизу", "Слева", "Справа"]);
        controls.append(&Self::settings_row("Положение Dock", &dock_edge));
        let preferred = gtk::Entry::new();
        preferred.set_placeholder_text(Some("Авто (пусто)"));
        preferred.set_max_length(256);
        controls.append(&Self::settings_row("Экран Dock (connector)", &preferred));
        let output_names = self
            .output_info()
            .iter()
            .map(|o| o.connector.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>()
            .join(", ");
        let note = label(&format!(
            "Доступные выходы: {output_names}. Enter — применить."
        ));
        note.set_wrap(true);
        controls.append(&note);
        let weak = Rc::downgrade(self);
        preferred.connect_activate(move |entry| {
            if let Some(v) = weak.upgrade() {
                let value = toml::Value::String(entry.text().to_string());
                v.change(&format!("dock.preferred_output={value}"));
            }
        });
        let auto_hide = gtk::Switch::new();
        let current_output_only = gtk::Switch::new();
        controls.append(&Self::settings_row(
            "Окна только на экране Dock",
            &current_output_only,
        ));
        let all_outputs = gtk::Switch::new();
        controls.append(&Self::settings_row("Dock на каждом мониторе", &all_outputs));
        controls.append(&Self::settings_row(
            "Автоматически скрывать Dock",
            &auto_hide,
        ));
        let magnification = gtk::Switch::new();
        controls.append(&Self::settings_row(
            "Увеличивать значки при наведении",
            &magnification,
        ));
        let minimize = gtk::Switch::new();
        controls.append(&Self::settings_row(
            "Повторный клик сворачивает активное окно",
            &minimize,
        ));
        let size = gtk::SpinButton::with_range(32., 80., 4.);
        controls.append(&Self::settings_row("Размер ячейки значка, px", &size));
        let visual_size = gtk::SpinButton::with_range(50., 100., 1.);
        controls.append(&Self::settings_row(
            "Размер изображения значка, %",
            &visual_size,
        ));
        let icon_filter = gtk::DropDown::from_strings(&ICON_FILTERS.map(|(_, _, label)| label));
        controls.append(&Self::settings_row(
            "Фильтрация растровых иконок",
            &icon_filter,
        ));
        let weak = Rc::downgrade(self);
        icon_filter.connect_selected_notify(move |widget| {
            if let Some(view) = weak.upgrade()
                && let Some((_, key, _)) = ICON_FILTERS.get(widget.selected() as usize)
            {
                view.change(&format!("dock.icon_filter='{key}'"));
            }
        });
        let show_delay = gtk::SpinButton::with_range(0., 2000., 50.);
        controls.append(&Self::settings_row("Задержка показа, мс", &show_delay));
        let hide_delay = gtk::SpinButton::with_range(100., 5000., 100.);
        controls.append(&Self::settings_row("Задержка скрытия, мс", &hide_delay));
        let hover_title = label("Анимация наведения");
        hover_title.set_xalign(0.);
        hover_title.set_margin_top(12);
        hover_title.add_css_class("heading");
        controls.append(&hover_title);
        let hover_values = HOVER_VALUES.map(|(label, _, min, max, step)| {
            let spin = gtk::SpinButton::with_range(min, max, step);
            controls.append(&Self::settings_row(label, &spin));
            spin
        });
        let hover_easing = gtk::DropDown::from_strings(&HOVER_EASINGS.map(|(_, _, text)| text));
        controls.append(&Self::settings_row("Кривая анимации", &hover_easing));
        let hover_cards = gtk::Switch::new();
        controls.append(&Self::settings_row(
            "Карточки окон при наведении",
            &hover_cards,
        ));
        let weak = Rc::downgrade(self);
        hover_easing.connect_selected_notify(move |widget| {
            if let Some(v) = weak.upgrade()
                && let Some((_, name, _)) = HOVER_EASINGS.get(widget.selected() as usize)
            {
                v.change(&format!("dock.hover.easing='{name}'"));
            }
        });
        let weak = Rc::downgrade(self);
        let panel = LayoutEditor::new(move |edit| {
            if let Some(view) = weak.upgrade() {
                view.edit_panel(edit);
            }
        });
        let info = label(
            "Порядок закреплений меняется перетаскиванием в Dock.\nПравый клик по значку открывает действия приложения.",
        );
        info.set_wrap(true);
        info.set_xalign(0.);
        controls.append(&info);
        let panel_controls = gtk::Box::new(gtk::Orientation::Vertical, 14);
        panel_controls.set_sensitive(!managed);
        let panel_transparency = gtk::SpinButton::with_range(0., 100., 1.);
        panel_controls.append(&Self::settings_row(
            "Прозрачность фона, %",
            &panel_transparency,
        ));
        panel_controls.append(panel.widget());
        for (key, title, page) in [
            (PreferencesSection::Dock.name(), "Dock", &controls),
            ("panel", "Панель", &panel_controls),
        ] {
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(page)
                .build();
            pages.add_titled(&scroll, Some(key), title);
        }
        pages.set_visible_child_name(section.name());
        let error = label("");
        error.set_visible(false);
        error.set_wrap(true);
        error.set_margin_top(8);
        error.set_margin_bottom(8);
        error.set_margin_start(24);
        error.set_margin_end(24);
        body.append(&error);
        let weak = Rc::downgrade(self);
        dock_edge.connect_selected_notify(move |dropdown| {
            if let Some(view) = weak.upgrade() {
                let edge = match dropdown.selected() {
                    0 => "bottom",
                    1 => "left",
                    2 => "right",
                    _ => return,
                };
                view.change(&format!("dock.edge='{edge}'"));
            }
        });
        for (widget, key) in [
            (&auto_hide, "dock.auto_hide"),
            (&current_output_only, "dock.current_output_only"),
            (&all_outputs, "dock.all_outputs"),
            (&magnification, "dock.magnification"),
            (&hover_cards, "dock.hover.cards_enabled"),
            (&minimize, "dock.minimize_on_click"),
        ] {
            let weak = Rc::downgrade(self);
            widget.connect_active_notify(move |w| {
                if let Some(v) = weak.upgrade() {
                    v.change(&format!("{key}={}", w.is_active()));
                }
            });
        }
        for (widget, key) in [
            (&size, "dock.icon_size"),
            (&visual_size, "dock.icon_visual_percent"),
            (&show_delay, "dock.show_delay_ms"),
            (&hide_delay, "dock.hide_delay_ms"),
            (&panel_transparency, "panel.transparency_percent"),
            (&dock_transparency, "dock.transparency_percent"),
        ] {
            let weak = Rc::downgrade(self);
            widget.connect_value_changed(move |w| {
                if let Some(v) = weak.upgrade() {
                    v.change(&format!("{key}={}", w.value_as_int()));
                }
            });
        }
        for (widget, (_, key, ..)) in hover_values.iter().zip(HOVER_VALUES) {
            let weak = Rc::downgrade(self);
            widget.connect_value_changed(move |w| {
                if let Some(v) = weak.upgrade() {
                    v.change(&format!("{key}={}", w.value_as_int()));
                }
            });
        }
        *self.preferences.borrow_mut() = Some(Preferences {
            window: window.clone(),
            pages,
            appearance,
            dock_edge,
            preferred,
            current_output_only,
            all_outputs,
            auto_hide,
            magnification,
            hover_values,
            hover_easing,
            hover_cards,
            minimize,
            size,
            visual_size,
            icon_filter,
            show_delay,
            hide_delay,
            panel_transparency,
            dock_transparency,
            panel,
            error,
        });
        self.sync_preferences();
        window.present();
    }
}
pub fn run(
    app: &gtk::Application,
    commands: async_channel::Receiver<Command>,
    shared: Shared,
) -> Result<(), Box<dyn std::error::Error>> {
    if !gtk4_layer_shell::is_supported() {
        return Err("wlr-layer-shell unavailable".into());
    }
    let (tracker, events) = Tracker::new()?;
    let provider = gtk::CssProvider::new();
    let appearance_css = gtk::CssProvider::new();
    provider.load_from_string(include_str!("../assets/style.css"));
    if let Some(display) = gdk::Display::default() {
        if let Some(icons) = option_env!("MOSAIC_GTK_ICON_DIR") {
            gtk::IconTheme::for_display(&display).add_search_path(icons);
        }
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
        gtk::style_context_add_provider_for_display(
            &display,
            &appearance_css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
    let path = config_path();
    let (settings, error) = match settings::load(&path) {
        Ok(s) => (s, None),
        Err(_) => (
            Settings::default(),
            Some("Неверная конфигурация; используются встроенные значения"),
        ),
    };
    let mut apps: Vec<_> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .filter_map(|info| {
            let id = info.id()?.to_string();
            let desktop = gio_unix::DesktopAppInfo::new(&id);
            let wm = desktop
                .as_ref()
                .and_then(|d| d.startup_wm_class())
                .map(|s| s.to_string());
            let desktop_file = desktop.and_then(|d| d.filename());
            let name = info.display_name().to_string();
            Some(AppEntry {
                info,
                model: Desktop { id, wm_class: wm },
                name,
                desktop_file,
            })
        })
        .collect();
    apps.sort_by_key(|a| a.name.to_lowercase());
    let view = Rc::new_cyclic(|owner: &std::rc::Weak<View>| View {
        app: app.clone(),
        appearance_css,
        appearance_style: Cell::default(),
        trash: crate::trash::Trash::new({
            let owner = owner.clone();
            move |state| {
                if let Some(view) = owner.upgrade() {
                    for dock in view.docks() {
                        dock.dock.set_trash_state(state);
                    }
                }
            }
        }),
        tray: crate::tray_ui::Tray::new(),
        panels: RefCell::new(Vec::new()),
        status_text: RefCell::new(error.unwrap_or("").into()),
        primary_dock: RefCell::new(DockView::new(app)),
        extra_docks: RefCell::new(Vec::new()),
        models: apps.iter().map(|a| a.model.clone()).collect(),
        apps,
        windows: RefCell::new(Vec::new()),
        settings: RefCell::new(settings),
        path,
        tracker,
        preview_busy: Cell::new(false),
        shared,
        pending: RefCell::new(Launches::default()),
        preferences: RefCell::new(None),
        quick_appearance: RefCell::new(None),
        wifi_pane: RefCell::new(None),
        bluetooth_pane: RefCell::new(None),
        brightness_pane: RefCell::new(None),
        temperature_pane: RefCell::new(None),
        temperature_timer: RefCell::new(None),
        temperature_pending: Cell::new(false),
        temperature_snapshot: RefCell::new(None),
        power_pane: RefCell::new(None),
        audio_pane: RefCell::new(None),
        dialog: RefCell::new(None),
        syncing: Cell::new(false),
        dropping_file: Cell::new(false),
    });
    view.wire_dock(&view.primary_dock());
    if let Some(settings) = gtk::Settings::default() {
        let weak = Rc::downgrade(&view);
        settings.connect_gtk_interface_color_scheme_notify(move |_| {
            if let Some(v) = weak.upgrade() {
                v.reconfigure();
            }
        });
        let weak = Rc::downgrade(&view);
        settings.connect_gtk_enable_animations_notify(move |_| {
            if let Some(v) = weak.upgrade() {
                v.reconfigure();
            }
        });
    }
    if let Some(display) = gdk::Display::default() {
        let weak = Rc::downgrade(&view);
        display.monitors().connect_items_changed(move |_, _, _, _| {
            if let Some(v) = weak.upgrade() {
                v.sync_outputs();
            }
        });
        let weak = Rc::downgrade(&view);
        gtk::IconTheme::for_display(&display).connect_changed(move |_| {
            let weak = weak.clone();
            glib::idle_add_local_once(move || {
                if let Some(view) = weak.upgrade() {
                    for dock in view.docks() {
                        dock.dock.invalidate_icons();
                    }
                    view.redraw();
                }
            });
        });
    }
    view.reconfigure();
    view.trash.start();
    let v = view.clone();
    glib::MainContext::default().spawn_local(async move {
        while let Ok((windows, healthy)) = events.recv().await {
            if !healthy {
                v.fail("Wayland отключён; управление окнами недоступно");
            }
            let opened: HashSet<_> = desktop::newly_observed_windows(&v.windows.borrow(), &windows)
                .into_iter()
                .filter_map(|w| desktop::match_app(&w.app_id, &v.models))
                .collect();
            v.pending.borrow_mut().acknowledge(&opened);
            if let Ok(mut shared) = v.shared.lock() {
                *shared = windows.clone();
            }
            *v.windows.borrow_mut() = windows;
            v.redraw();
        }
    });
    glib::MainContext::default().spawn_local(async move {
        while let Ok(command) = commands.recv().await {
            match command {
                Command::TrashStatus(reply) => {
                    let _ = reply.try_send(Ok(view.trash.state().name().into()));
                }
                Command::Launcher(hint) => view.launcher(None, &hint),
                Command::ControlCenter(hint) => view.toggle_control_center(None, &hint),
                Command::Wifi(hint) => view.show_wifi(None, &hint),
                Command::Bluetooth(hint) => view.show_bluetooth(None, &hint),
                Command::Brightness(hint) => view.show_brightness(None, &hint),
                Command::Temperatures(hint) => view.show_temperatures(None, &hint),
                Command::Power(hint) => view.show_power(None, &hint),
                Command::Settings => view.settings(None),
                Command::CloseDialog => view.close_dialog(),
                Command::DialogInfo(reply) => {
                    let _ = reply.try_send(Ok(view.dialog_info()));
                }
                Command::Dock(hint, reply) => {
                    let result = view.resolve_dock(&hint).map(|target| {
                        view.close_dialog();
                        view.release_dock_focus();
                        target.dock.show(true);
                    });
                    let _ = reply.try_send(result);
                }
                Command::Quit => {
                    view.app.quit();
                    break;
                }
                Command::Launch(id) => view.launch(&id),
                Command::Pin(id, pinned) => view.pin(&id, pinned),
                Command::ReadConfig(reply) => {
                    let _ = reply.try_send(Ok(view.settings.borrow().clone()));
                }
                Command::ValidatePatch(patch, reply) => {
                    let result = settings::patched(&view.settings.borrow(), &patch)
                        .map_err(|_| ApiError::InvalidArgument("invalid config patch".into()))
                        .and_then(|s| {
                            toml::to_string(&s)
                                .map_err(|_| ApiError::Failed("serialization".into()))
                        });
                    let _ = reply.try_send(result);
                }
                Command::ApplyPatch(revision, patch, deadline, reply) => {
                    let result = if std::time::Instant::now() > deadline {
                        Err(ApiError::Timeout("deadline expired".into()))
                    } else {
                        view.patch(revision, &patch)
                    };
                    let _ = reply.try_send(result);
                }
                Command::Reorder(revision, source, before, deadline, reply) => {
                    let result = if std::time::Instant::now() > deadline {
                        Err(ApiError::Timeout("deadline expired".into()))
                    } else {
                        view.reorder(revision, &source, &before)
                    };
                    let _ = reply.try_send(result);
                }
                Command::Reload(reply) => {
                    let result = view.reload();
                    if let Err(e) = &result {
                        view.show_error(e);
                    }
                    let _ = reply.try_send(result);
                }
                Command::TrayState(reply) => {
                    let _ = reply.try_send(
                        toml::to_string(&view.tray.info())
                            .map_err(|_| ApiError::Failed("tray state".into())),
                    );
                }
                Command::Outputs(reply) => {
                    let _ = reply.try_send(Ok(view.output_info()));
                }
                Command::DockCards(hint, reply) => {
                    let ids = view.resolve_dock(&hint).map(|dock| {
                        dock.cards
                            .borrow()
                            .as_ref()
                            .filter(|open| open.popup.is_mapped())
                            .map(|open| open.rows.iter().map(|row| row.key.1).collect())
                            .unwrap_or_default()
                    });
                    let _ = reply.try_send(ids);
                }
                Command::DockInfo(hint, reply) => {
                    let _ = reply.try_send(view.resolve_dock(&hint).map(|d| d.dock.info()));
                }
                Command::Window(epoch, id, action) => {
                    if action == "activate" {
                        view.close_dialog();
                    }
                    if let Err(e) = view.tracker.borrow().request(epoch, id, &action) {
                        view.fail(&e);
                    }
                }
            }
        }
    });
    Ok(())
}
