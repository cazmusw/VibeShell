use crate::audio::{AudioChoice, AudioState, BUS, Device, PATH, Playback};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

pub struct AudioPane {
    widget: gtk::Box,
    controls: gtk::Box,
    status: gtk::Label,
    sink: gtk::DropDown,
    source: gtk::DropDown,
    volume: gtk::Scale,
    input_volume: gtk::Scale,
    balance_row: gtk::Box,
    balance: gtk::Scale,
    sink_mute: gtk::CheckButton,
    source_mute: gtk::CheckButton,
    playback: gtk::Box,
    playback_empty: gtk::Label,
    streams: RefCell<Vec<StreamRow>>,
    hardware: gtk::Box,
    hardware_page: gtk::StackPage,
    hardware_rows: RefCell<Vec<HardwareRow>>,
    state: RefCell<AudioState>,
    connection: RefCell<Option<gio::DBusConnection>>,
    subscriptions: RefCell<Vec<gio::SignalSubscription>>,
    refreshing: Cell<bool>,
    dirty: Cell<bool>,
    pending: Cell<bool>,
    syncing: Cell<bool>,
    volume_timer: RefCell<Option<VolumeEdit>>,
    error: Cell<bool>,
}

#[derive(PartialEq)]
enum VolumeTarget {
    Output(String),
    Input(String),
    Balance(String),
    Stream(u32),
}
impl VolumeTarget {
    fn is_current(&self, state: &AudioState) -> bool {
        state.available
            && match self {
                Self::Output(name) => *name == state.default_sink,
                Self::Input(name) => *name == state.default_source,
                Self::Balance(name) => {
                    *name == state.default_sink
                        && state
                            .sinks
                            .iter()
                            .any(|device| device.name == *name && device.balance.is_some())
                }
                Self::Stream(index) => state.playback.iter().any(|stream| stream.index == *index),
            }
    }
}
struct VolumeEdit {
    target: VolumeTarget,
    timer: glib::SourceId,
}

struct StreamRow {
    index: u32,
    widget: gtk::Box,
    title: gtk::Label,
    output: gtk::DropDown,
    volume: gtk::Scale,
    mute: gtk::CheckButton,
}

struct HardwareRow {
    action: &'static str,
    name: String,
    widget: gtk::Box,
    title: gtk::Label,
    selection: gtk::DropDown,
    options: Rc<RefCell<Vec<u32>>>,
}

impl AudioPane {
    pub fn new(embedded: bool) -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 8);
        if embedded {
            widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            let title = gtk::Label::new(Some("Звук"));
            title.set_xalign(0.);
            title.add_css_class("heading");
            widget.append(&title);
        }
        let status = gtk::Label::new(Some("Проверяем аудиосервер…"));
        status.set_xalign(0.);
        status.set_wrap(true);
        widget.append(&status);
        let controls = gtk::Box::new(gtk::Orientation::Vertical, 8);
        controls.set_visible(false);
        let devices = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let sink = Self::devices(&devices, "Выход");
        let volume = Self::volume_scale("Громкость выхода");
        devices.append(&volume);
        let sink_mute = gtk::CheckButton::with_label("Отключить звук");
        devices.append(&sink_mute);
        let balance_row = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let balance_label = gtk::Label::new(Some("Баланс выхода"));
        balance_label.set_xalign(0.);
        balance_row.append(&balance_label);
        let balance = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 200., 1.);
        balance.set_draw_value(true);
        balance.set_value_pos(gtk::PositionType::Right);
        balance.add_mark(100., gtk::PositionType::Bottom, None);
        balance.set_format_value_func(|_, value| {
            let value = value.round() as u32;
            match value.cmp(&100) {
                std::cmp::Ordering::Less => format!("Влево {}%", 100 - value),
                std::cmp::Ordering::Equal => "Центр".into(),
                std::cmp::Ordering::Greater => format!("Вправо {}%", value - 100),
            }
        });
        balance.update_property(&[gtk::accessible::Property::Label("Баланс выхода")]);
        balance_row.append(&balance);
        devices.append(&balance_row);
        let source = Self::devices(&devices, "Вход");
        let input_level = gtk::Label::new(Some("Уровень входа"));
        input_level.set_xalign(0.);
        devices.append(&input_level);
        let input_volume = Self::volume_scale("Уровень входа");
        devices.append(&input_volume);
        let source_mute = gtk::CheckButton::with_label("Отключить вход");
        devices.append(&source_mute);
        let playback = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let applications = gtk::Box::new(gtk::Orientation::Vertical, 8);
        let playback_empty = gtk::Label::new(Some("Начните воспроизведение в приложении."));
        playback_empty.set_xalign(0.);
        playback_empty.set_wrap(true);
        applications.append(&playback_empty);
        applications.append(&playback);
        let stack = gtk::Stack::new();
        stack.set_hhomogeneous(false);
        stack.set_vhomogeneous(false);
        stack.add_titled(&devices, Some("devices"), "Устройства");
        stack.add_titled(&applications, Some("applications"), "Приложения");
        let hardware = gtk::Box::new(gtk::Orientation::Vertical, 12);
        let hardware_page = stack.add_titled(&hardware, Some("hardware"), "Профили и порты");
        hardware_page.set_visible(false);
        let tabs = gtk::StackSwitcher::new();
        tabs.add_css_class("audio-tabs");
        tabs.set_stack(Some(&stack));
        tabs.set_halign(gtk::Align::Start);
        controls.append(&tabs);
        controls.append(&stack);
        widget.append(&controls);

        let pane = Rc::new(Self {
            widget,
            controls,
            status,
            sink,
            source,
            volume,
            input_volume,
            balance_row,
            balance,
            sink_mute,
            source_mute,
            playback,
            playback_empty,
            streams: RefCell::new(Vec::new()),
            hardware,
            hardware_page,
            hardware_rows: RefCell::new(Vec::new()),
            state: RefCell::new(AudioState::default()),
            connection: RefCell::new(None),
            subscriptions: RefCell::new(Vec::new()),
            refreshing: Cell::new(false),
            dirty: Cell::new(false),
            pending: Cell::new(false),
            syncing: Cell::new(false),
            volume_timer: RefCell::new(None),
            error: Cell::new(false),
        });
        pane.connect_controls();
        pane.connect_bus();
        pane
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    fn volume_scale(name: &str) -> gtk::Scale {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0., 100., 1.);
        scale.set_draw_value(true);
        scale.set_format_value_func(|_, value| format!("{value:.0}%"));
        scale.set_hexpand(true);
        scale.update_property(&[gtk::accessible::Property::Label(name)]);
        scale
    }

    fn devices(controls: &gtk::Box, caption: &str) -> gtk::DropDown {
        let label = gtk::Label::new(Some(caption));
        label.set_xalign(0.);
        controls.append(&label);
        let dropdown = gtk::DropDown::from_strings(&[]);
        dropdown.set_hexpand(true);
        dropdown.set_enable_search(true);
        dropdown.update_property(&[gtk::accessible::Property::Label(caption)]);
        controls.append(&dropdown);
        dropdown
    }

    fn connect_controls(self: &Rc<Self>) {
        for (input, dropdown) in [(false, &self.sink), (true, &self.source)] {
            let weak = Rc::downgrade(self);
            dropdown.connect_selected_notify(move |dropdown| {
                let Some(pane) = weak.upgrade().filter(|p| !p.syncing.get()) else {
                    return;
                };
                let state = pane.state.borrow();
                let devices = if input { &state.sources } else { &state.sinks };
                let Some(device) = devices.get(dropdown.selected() as usize) else {
                    return;
                };
                let name = device.name.clone();
                drop(state);
                pane.apply(
                    if input {
                        "default-source"
                    } else {
                        "default-sink"
                    },
                    name,
                    0,
                );
            });
        }
        for (input, mute) in [(false, &self.sink_mute), (true, &self.source_mute)] {
            let weak = Rc::downgrade(self);
            mute.connect_toggled(move |button| {
                let Some(pane) = weak.upgrade().filter(|p| !p.syncing.get()) else {
                    return;
                };
                let state = pane.state.borrow();
                let name = if input {
                    &state.default_source
                } else {
                    &state.default_sink
                }
                .clone();
                drop(state);
                pane.apply(
                    if input { "source-mute" } else { "sink-mute" },
                    name,
                    u32::from(button.is_active()),
                );
            });
        }
        for (scale, target) in [
            (
                &self.volume,
                VolumeTarget::Output as fn(String) -> VolumeTarget,
            ),
            (&self.input_volume, VolumeTarget::Input),
            (&self.balance, VolumeTarget::Balance),
        ] {
            let weak = Rc::downgrade(self);
            scale.connect_value_changed(move |scale| {
                let Some(pane) = weak
                    .upgrade()
                    .filter(|p| !p.syncing.get() && !p.pending.get())
                else {
                    return;
                };
                let state = pane.state.borrow();
                let target = if *scale == pane.input_volume {
                    target(state.default_source.clone())
                } else {
                    target(state.default_sink.clone())
                };
                drop(state);
                pane.schedule_volume(target, scale.value());
            });
        }
    }

    fn schedule_volume(self: &Rc<Self>, target: VolumeTarget, value: f64) {
        let previous = self.cancel_volume();
        let replaced = previous.is_some_and(|previous| previous != target);
        let weak = Rc::downgrade(self);
        let timer = glib::timeout_add_local_once(Duration::from_millis(125), move || {
            let Some(pane) = weak.upgrade() else { return };
            let Some(edit) = pane.volume_timer.borrow_mut().take() else {
                return;
            };
            let state = pane.state.borrow();
            let value = value.round() as u32;
            if !edit.target.is_current(&state) {
                drop(state);
                pane.render();
                return;
            }
            let action = match edit.target {
                VolumeTarget::Output(name) => ("sink-volume", name),
                VolumeTarget::Input(name) => ("source-volume", name),
                VolumeTarget::Balance(name) => ("sink-balance", name),
                VolumeTarget::Stream(index) => ("stream-volume", index.to_string()),
            };
            drop(state);
            pane.apply(action.0, action.1, value);
        });
        *self.volume_timer.borrow_mut() = Some(VolumeEdit { target, timer });
        if replaced {
            self.render();
        }
    }

    fn cancel_volume(&self) -> Option<VolumeTarget> {
        let edit = self.volume_timer.borrow_mut().take()?;
        edit.timer.remove();
        Some(edit.target)
    }

    fn connect_bus(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = gio::bus_get_future(gio::BusType::Session).await;
            let Some(pane) = weak.upgrade() else { return };
            let Ok(connection) = result else {
                pane.render();
                return;
            };
            let weak = Rc::downgrade(&pane);
            let change = connection.subscribe_to_signal(
                Some(BUS),
                Some(BUS),
                Some("AudioChanged"),
                Some(PATH),
                None,
                gio::DBusSignalFlags::NONE,
                move |_| {
                    if let Some(pane) = weak.upgrade() {
                        pane.refresh();
                    }
                },
            );
            let weak = Rc::downgrade(&pane);
            let owner = connection.subscribe_to_signal(
                Some("org.freedesktop.DBus"),
                Some("org.freedesktop.DBus"),
                Some("NameOwnerChanged"),
                Some("/org/freedesktop/DBus"),
                Some(BUS),
                gio::DBusSignalFlags::NONE,
                move |_| {
                    if let Some(pane) = weak.upgrade() {
                        pane.cancel_volume();
                        *pane.state.borrow_mut() = AudioState::default();
                        pane.render();
                        pane.refresh();
                    }
                },
            );
            *pane.subscriptions.borrow_mut() = vec![change, owner];
            *pane.connection.borrow_mut() = Some(connection);
            pane.refresh();
        });
    }

    fn refresh(self: &Rc<Self>) {
        if self.refreshing.replace(true) {
            self.dirty.set(true);
            return;
        }
        let Some(connection) = self.connection.borrow().clone() else {
            self.refreshing.set(false);
            return;
        };
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = connection
                .call_future(
                    Some(BUS),
                    PATH,
                    BUS,
                    "GetAudio",
                    None,
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    4500,
                )
                .await;
            let Some(pane) = weak.upgrade() else { return };
            pane.refreshing.set(false);
            if pane.dirty.replace(false) {
                pane.refresh();
                return;
            }
            let next = result
                .ok()
                .and_then(|value| value.get::<(String,)>())
                .and_then(|(payload,)| toml::from_str(&payload).ok())
                .unwrap_or_default();
            pane.state.replace(next);
            let target_lost = {
                let state = pane.state.borrow();
                let edit = pane.volume_timer.borrow();
                edit.as_ref()
                    .is_some_and(|edit| !edit.target.is_current(&state))
            };
            if target_lost {
                pane.cancel_volume();
            }
            pane.render();
        });
    }

    fn apply(self: &Rc<Self>, action: &'static str, device: String, value: u32) {
        if self.pending.get() || device.is_empty() {
            return;
        }
        let Some(connection) = self.connection.borrow().clone() else {
            return;
        };
        self.pending.set(true);
        self.cancel_volume();
        self.error.set(false);
        self.controls.set_sensitive(false);
        self.status.set_text("Применяем…");
        self.status.set_visible(true);
        let parameters = (self.state.borrow().revision, action, device, value).to_variant();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = connection
                .call_future(
                    Some(BUS),
                    PATH,
                    BUS,
                    "SetAudio",
                    Some(&parameters),
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    4500,
                )
                .await;
            if let Some(pane) = weak.upgrade() {
                pane.pending.set(false);
                pane.error.set(result.is_err());
                pane.refresh();
            }
        });
    }

    fn sync_dropdown(dropdown: &gtk::DropDown, devices: &[Device], selected: &str) {
        let model = dropdown.model().and_downcast::<gtk::StringList>().unwrap();
        if model.n_items() as usize != devices.len()
            || devices
                .iter()
                .enumerate()
                .any(|(i, d)| model.string(i as u32).as_deref() != Some(d.description.as_str()))
        {
            let names = devices
                .iter()
                .map(|d| d.description.as_str())
                .collect::<Vec<_>>();
            model.splice(0, model.n_items(), &names);
        }
        dropdown.set_selected(
            devices
                .iter()
                .position(|d| d.name == selected)
                .map_or(gtk::INVALID_LIST_POSITION, |i| i as u32),
        );
    }

    fn sync_playback(self: &Rc<Self>, state: &AudioState) {
        self.playback_empty.set_visible(state.playback.is_empty());
        let mut rows = self.streams.borrow_mut();
        rows.retain(|row| {
            if state
                .playback
                .iter()
                .any(|stream| stream.index == row.index)
            {
                true
            } else {
                self.playback.remove(&row.widget);
                false
            }
        });
        let volume_timer = self.volume_timer.borrow();
        let mut previous = None;
        for stream in &state.playback {
            let row = if let Some(i) = rows.iter().position(|row| row.index == stream.index) {
                &rows[i]
            } else {
                let row = StreamRow::new(self, stream.index);
                self.playback.append(&row.widget);
                rows.push(row);
                rows.last().unwrap()
            };
            row.sync(
                stream,
                &state.sinks,
                volume_timer
                    .as_ref()
                    .is_some_and(|edit| edit.target == VolumeTarget::Stream(stream.index)),
            );
            self.playback
                .reorder_child_after(&row.widget, previous.as_ref());
            previous = Some(row.widget.clone());
        }
    }

    fn sync_hardware(self: &Rc<Self>, state: &AudioState) {
        let mut items = Vec::new();
        for card in &state.cards {
            if !card.profiles.is_empty() {
                items.push((
                    "card-profile",
                    card.name.as_str(),
                    format!("Профиль · {}", card.description),
                    card.profiles.as_slice(),
                    card.active_profile.as_str(),
                ));
            }
        }
        for (action, title, devices, active) in [
            (
                "sink-port",
                "Порт выхода",
                state.sinks.as_slice(),
                &state.default_sink,
            ),
            (
                "source-port",
                "Порт входа",
                state.sources.as_slice(),
                &state.default_source,
            ),
        ] {
            if let Some(device) = devices.iter().find(|device| device.name == *active)
                && !device.ports.is_empty()
            {
                items.push((
                    action,
                    device.name.as_str(),
                    format!("{title} · {}", device.description),
                    device.ports.as_slice(),
                    device.active_port.as_str(),
                ));
            }
        }
        let mut rows = self.hardware_rows.borrow_mut();
        rows.retain(|row| {
            if items
                .iter()
                .any(|(action, name, ..)| row.action == *action && row.name == *name)
            {
                true
            } else {
                self.hardware.remove(&row.widget);
                false
            }
        });
        let mut previous = None;
        for (action, name, title, choices, active) in &items {
            let row = if let Some(i) = rows
                .iter()
                .position(|row| row.action == *action && row.name == *name)
            {
                &rows[i]
            } else {
                let row = HardwareRow::new(self, action, name);
                self.hardware.append(&row.widget);
                rows.push(row);
                rows.last().unwrap()
            };
            row.sync(title, choices, active);
            self.hardware
                .reorder_child_after(&row.widget, previous.as_ref());
            previous = Some(row.widget.clone());
        }
        self.hardware_page.set_visible(!items.is_empty());
    }

    fn render(self: &Rc<Self>) {
        let state = self.state.borrow();
        self.controls.set_visible(state.available);
        self.controls.set_sensitive(!self.pending.get());
        let message = if !state.available {
            "Аудиосервер недоступен"
        } else if self.pending.get() {
            "Применяем…"
        } else if self.error.get() {
            "Не удалось изменить настройку звука"
        } else if state.sinks.is_empty() && state.sources.is_empty() {
            "Аудиоустройства не найдены"
        } else {
            ""
        };
        self.status.set_visible(!message.is_empty());
        self.status.set_text(message);
        if self.pending.get() {
            return;
        }
        self.syncing.set(true);
        Self::sync_dropdown(&self.sink, &state.sinks, &state.default_sink);
        Self::sync_dropdown(&self.source, &state.sources, &state.default_source);
        self.sink.set_sensitive(!state.sinks.is_empty());
        self.source.set_sensitive(!state.sources.is_empty());
        let sink = state
            .sinks
            .iter()
            .find(|device| device.name == state.default_sink);
        let source = state
            .sources
            .iter()
            .find(|device| device.name == state.default_source);
        self.sink_mute.set_sensitive(sink.is_some());
        self.source_mute.set_sensitive(source.is_some());
        self.sink_mute
            .set_active(sink.is_some_and(|device| device.muted));
        self.source_mute
            .set_active(source.is_some_and(|device| device.muted));
        let volume = sink.and_then(|device| device.volume);
        self.volume.set_sensitive(volume.is_some());
        let input_volume = source.and_then(|device| device.volume);
        self.input_volume.set_sensitive(input_volume.is_some());
        let balance = sink.and_then(|device| device.balance);
        self.balance_row.set_visible(balance.is_some());
        {
            let edit = self.volume_timer.borrow();
            let pending = edit.as_ref().map(|edit| &edit.target);
            if !matches!(pending, Some(VolumeTarget::Output(_))) {
                self.volume
                    .set_value(f64::from(volume.unwrap_or(0).min(100)));
            }
            if !matches!(pending, Some(VolumeTarget::Input(_))) {
                self.input_volume
                    .set_value(f64::from(input_volume.unwrap_or(0).min(100)));
            }
            if !matches!(pending, Some(VolumeTarget::Balance(_))) {
                self.balance.set_value(f64::from(balance.unwrap_or(100)));
            }
        }
        self.sync_playback(&state);
        self.sync_hardware(&state);
        self.syncing.set(false);
    }
}

impl HardwareRow {
    fn new(pane: &Rc<AudioPane>, action: &'static str, name: &str) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 5);
        let title = gtk::Label::new(None);
        title.set_xalign(0.);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        widget.append(&title);
        let selection = gtk::DropDown::from_strings(&[]);
        selection.set_hexpand(true);
        widget.append(&selection);
        let options = Rc::<RefCell<Vec<u32>>>::default();
        let weak = Rc::downgrade(pane);
        let lookup = options.clone();
        let target = name.to_owned();
        selection.connect_selected_notify(move |dropdown| {
            let Some(pane) = weak.upgrade().filter(|pane| !pane.syncing.get()) else {
                return;
            };
            let value = lookup.borrow().get(dropdown.selected() as usize).copied();
            if let Some(value) = value {
                pane.apply(action, target.clone(), value);
            }
        });
        Self {
            action,
            name: name.into(),
            widget,
            title,
            selection,
            options,
        }
    }

    fn sync(&self, title: &str, choices: &[AudioChoice], active: &str) {
        if self.title.text().as_str() != title {
            self.title.set_label(title);
            self.title.set_tooltip_text(Some(title));
            self.selection
                .update_property(&[gtk::accessible::Property::Label(title)]);
        }
        let visible: Vec<_> = choices
            .iter()
            .enumerate()
            .filter(|(_, choice)| choice.available || choice.name == active)
            .collect();
        let labels: Vec<_> = visible
            .iter()
            .map(|(_, choice)| {
                if choice.available {
                    choice.description.clone()
                } else {
                    format!("{} (недоступен)", choice.description)
                }
            })
            .collect();
        *self.options.borrow_mut() = visible.iter().map(|(i, _)| *i as u32).collect();
        let model = self
            .selection
            .model()
            .and_downcast::<gtk::StringList>()
            .unwrap();
        if model.n_items() as usize != labels.len()
            || labels
                .iter()
                .enumerate()
                .any(|(i, label)| model.string(i as u32).as_deref() != Some(label.as_str()))
        {
            let labels: Vec<_> = labels.iter().map(String::as_str).collect();
            model.splice(0, model.n_items(), &labels);
        }
        self.selection.set_selected(
            visible
                .iter()
                .position(|(_, choice)| choice.name == active)
                .map_or(gtk::INVALID_LIST_POSITION, |i| i as u32),
        );
        self.selection.set_sensitive(
            visible
                .iter()
                .any(|(_, choice)| choice.available && choice.name != active),
        );
    }
}

impl Drop for AudioPane {
    fn drop(&mut self) {
        self.cancel_volume();
    }
}

impl StreamRow {
    fn new(pane: &Rc<AudioPane>, index: u32) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 5);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        let title = gtk::Label::new(None);
        title.set_xalign(0.);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        widget.append(&title);
        let output = gtk::DropDown::from_strings(&[]);
        output.set_hexpand(true);
        widget.append(&output);

        let level = gtk::Box::new(gtk::Orientation::Horizontal, 10);
        let volume = AudioPane::volume_scale("Громкость приложения");
        level.append(&volume);
        let mute = gtk::CheckButton::with_label("Без звука");
        level.append(&mute);
        widget.append(&level);

        let weak = Rc::downgrade(pane);
        output.connect_selected_notify(move |dropdown| {
            let Some(pane) = weak.upgrade().filter(|p| !p.syncing.get()) else {
                return;
            };
            let output = pane
                .state
                .borrow()
                .sinks
                .get(dropdown.selected() as usize)
                .map(|device| device.name.clone());
            if let Some(output) = output {
                pane.apply("stream-output", output, index);
            }
        });
        let weak = Rc::downgrade(pane);
        volume.connect_value_changed(move |slider| {
            if let Some(pane) = weak
                .upgrade()
                .filter(|p| !p.syncing.get() && !p.pending.get())
            {
                pane.schedule_volume(VolumeTarget::Stream(index), slider.value());
            }
        });
        let weak = Rc::downgrade(pane);
        mute.connect_toggled(move |toggle| {
            if let Some(pane) = weak.upgrade().filter(|p| !p.syncing.get()) {
                pane.apply(
                    "stream-mute",
                    index.to_string(),
                    u32::from(toggle.is_active()),
                );
            }
        });
        Self {
            index,
            widget,
            title,
            output,
            volume,
            mute,
        }
    }

    fn sync(&self, stream: &Playback, sinks: &[Device], volume_pending: bool) {
        let label = match (stream.application.as_str(), stream.media.as_str()) {
            ("", "") => format!("Поток {}", stream.index),
            (app, "") | ("", app) => app.to_owned(),
            (app, media) if app == media => app.to_owned(),
            (app, media) => format!("{app} — {media}"),
        };
        if self.title.text().as_str() != label {
            self.title.set_label(&label);
            self.title.set_tooltip_text(Some(&label));
            self.output
                .update_property(&[gtk::accessible::Property::Label(&format!(
                    "Выход — {label}"
                ))]);
            self.volume
                .update_property(&[gtk::accessible::Property::Label(&format!(
                    "Громкость — {label}"
                ))]);
            self.mute
                .update_property(&[gtk::accessible::Property::Label(&format!(
                    "Без звука — {label}"
                ))]);
        }
        AudioPane::sync_dropdown(&self.output, sinks, &stream.output);
        self.output.set_sensitive(!sinks.is_empty());
        self.volume.set_sensitive(stream.volume.is_some());
        if !volume_pending {
            self.volume
                .set_value(f64::from(stream.volume.unwrap_or(0).min(100)));
        }
        self.mute.set_active(stream.muted);
    }
}
