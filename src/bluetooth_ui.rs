use crate::{
    audio::{BUS, PATH},
    bluetooth::Snapshot,
};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

mod pairing;
use pairing::Pairing;

#[derive(Clone, PartialEq, Eq)]
struct DeviceTarget {
    owner: String,
    adapter: String,
    path: String,
    address: String,
    identity: String,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DeviceAction {
    Pair,
    Connect,
    Disconnect,
    Remove,
}

impl DeviceAction {
    fn for_device(device: &crate::bluetooth::Device) -> Self {
        if device.connected {
            Self::Disconnect
        } else if device.paired {
            Self::Connect
        } else {
            Self::Pair
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pair => "Сопрячь",
            Self::Connect => "Подключить",
            Self::Disconnect => "Отключить",
            Self::Remove => "Удалить",
        }
    }
}

pub struct BluetoothPane {
    pub widget: gtk::Box,
    adapters: gtk::DropDown,
    power: gtk::Button,
    search: gtk::Button,
    status: gtk::Label,
    inline: gtk::Box,
    devices: gtk::Box,
    snapshot: RefCell<Option<Snapshot>>,
    syncing: Cell<bool>,
    reading: Cell<bool>,
    pending: Cell<bool>,
    generation: Cell<u64>,
    message: Cell<Option<&'static str>>,
    timer: RefCell<Option<glib::SourceId>>,
    removal: RefCell<Option<DeviceTarget>>,
    pairing: RefCell<Option<Pairing>>,
}

impl BluetoothPane {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.set_margin_top(16);
        widget.set_margin_bottom(16);
        widget.set_margin_start(20);
        widget.set_margin_end(20);
        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let adapters = gtk::DropDown::from_strings(&[]);
        adapters.update_property(&[gtk::accessible::Property::Label("Адаптер Bluetooth")]);
        adapters.set_hexpand(true);
        let power = gtk::Button::with_label("Включить");
        let search = gtk::Button::with_label("Найти устройства");
        controls.append(&adapters);
        controls.append(&power);
        controls.append(&search);
        widget.append(&controls);
        let status = gtk::Label::new(Some("Проверка BlueZ…"));
        status.set_xalign(0.);
        status.set_wrap(true);
        widget.append(&status);
        let inline = gtk::Box::new(gtk::Orientation::Vertical, 6);
        inline.set_visible(false);
        widget.append(&inline);
        let devices = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.append(
            &gtk::ScrolledWindow::builder()
                .child(&devices)
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .build(),
        );
        let pane = Rc::new(Self {
            widget,
            adapters,
            power,
            search,
            status,
            inline,
            devices,
            snapshot: RefCell::new(None),
            syncing: Cell::new(false),
            reading: Cell::new(false),
            pending: Cell::new(false),
            generation: Cell::new(0),
            message: Cell::new(None),
            timer: RefCell::new(None),
            removal: RefCell::new(None),
            pairing: RefCell::new(None),
        });
        pane.adapters.set_sensitive(false);
        pane.power.set_sensitive(false);
        pane.search.set_sensitive(false);
        let weak = Rc::downgrade(&pane);
        pane.adapters.connect_selected_notify(move |_| {
            if let Some(pane) = weak.upgrade().filter(|pane| !pane.syncing.get()) {
                pane.message.set(None);
                pane.removal.replace(None);
                pane.render();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.power.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.action(true);
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.search.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.action(false);
            }
        });
        pane.refresh();
        let weak = Rc::downgrade(&pane);
        *pane.timer.borrow_mut() =
            Some(glib::timeout_add_local(Duration::from_secs(2), move || {
                if let Some(pane) = weak.upgrade() {
                    pane.refresh();
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            }));
        pane
    }

    fn refresh(self: &Rc<Self>) {
        if self.pending.get() || self.reading.replace(true) {
            return;
        }
        let generation = self.generation.get();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let snapshot = read_snapshot().await;
            if let Some(pane) = weak.upgrade() {
                pane.reading.set(false);
                if generation == pane.generation.get() {
                    pane.receive(snapshot);
                }
            }
        });
    }

    fn receive(self: &Rc<Self>, snapshot: Snapshot) -> bool {
        if self.snapshot.borrow().as_ref() == Some(&snapshot) {
            return false;
        }
        let previous = self.snapshot.replace(Some(snapshot));
        let state = self.snapshot.borrow();
        let snapshot = state.as_ref().unwrap();
        let selected = previous
            .as_ref()
            .and_then(|state| state.adapters.get(self.adapters.selected() as usize))
            .map(|adapter| adapter.path.as_str());
        let model = self
            .adapters
            .model()
            .and_downcast::<gtk::StringList>()
            .unwrap();
        let names = snapshot
            .adapters
            .iter()
            .map(|adapter| {
                let id = adapter.path.rsplit('/').next().unwrap_or_default();
                if id == adapter.name {
                    adapter.name.clone()
                } else {
                    format!("{} ({id})", adapter.name)
                }
            })
            .collect::<Vec<_>>();
        self.syncing.set(true);
        if model.n_items() as usize != names.len()
            || names
                .iter()
                .enumerate()
                .any(|(i, name)| model.string(i as u32).as_deref() != Some(name.as_str()))
        {
            model.splice(
                0,
                model.n_items(),
                &names.iter().map(String::as_str).collect::<Vec<_>>(),
            );
        }
        let next = snapshot
            .adapters
            .iter()
            .position(|adapter| Some(adapter.path.as_str()) == selected)
            .or_else(|| (!snapshot.adapters.is_empty()).then_some(0));
        self.adapters
            .set_selected(next.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
        self.syncing.set(false);
        if previous.is_some_and(|previous| previous.owner != snapshot.owner) {
            self.message.set(None);
        }
        drop(state);
        self.render();
        true
    }

    fn render(self: &Rc<Self>) {
        let removal_stale = self
            .removal
            .borrow()
            .as_ref()
            .is_some_and(|target| !self.can_change(target, DeviceAction::Remove));
        if removal_stale {
            self.removal.replace(None);
            self.message.set(Some("Устройство Bluetooth изменилось"));
        }
        self.render_inline();
        let state = self.snapshot.borrow();
        let Some(snapshot) = state.as_ref() else {
            return;
        };
        let adapter = snapshot.adapters.get(self.adapters.selected() as usize);
        let pending = self.pending.get();
        let controls_enabled = !pending && self.removal.borrow().is_none();
        self.adapters
            .set_sensitive(snapshot.adapters.len() > 1 && controls_enabled);
        self.power
            .set_sensitive(adapter.is_some() && controls_enabled);
        self.search
            .set_sensitive(adapter.is_some_and(|adapter| adapter.powered) && controls_enabled);
        while let Some(row) = self.devices.first_child() {
            self.devices.remove(&row);
        }
        self.status.set_tooltip_text(
            (adapter.is_some() && !snapshot.pairing_enabled)
                .then_some("Для сопряжения запустите Mosaic с MOSAIC_BLUETOOTH_PAIRING=1"),
        );
        let Some(adapter) = adapter else {
            self.status.set_text(if snapshot.available {
                "Адаптеры Bluetooth не найдены"
            } else {
                "BlueZ недоступен"
            });
            return;
        };
        self.power.set_label(if adapter.powered {
            "Выключить"
        } else {
            "Включить"
        });
        self.search.set_label(if adapter.searching {
            "Остановить поиск"
        } else {
            "Найти устройства"
        });
        self.status.set_text(if pending {
            "Ожидание ответа BlueZ…"
        } else if let Some(message) = self.message.get() {
            message
        } else if !adapter.powered {
            "Bluetooth выключен на этом адаптере"
        } else if adapter.searching {
            "Идёт поиск устройств…"
        } else if adapter.discovering {
            "Поиск запущен другим приложением"
        } else if adapter.devices.is_empty() {
            "Устройства Bluetooth не найдены"
        } else if !snapshot.pairing_enabled
            && adapter
                .devices
                .iter()
                .any(|device| !device.paired && !device.connected)
        {
            "Агент сопряжения выключен"
        } else {
            "Устройства Bluetooth"
        });
        for device in &adapter.devices {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let labels = gtk::Box::new(gtk::Orientation::Vertical, 3);
            labels.set_hexpand(true);
            let name = gtk::Label::new(Some(&device.name));
            name.set_xalign(0.);
            name.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let connection = if device.connected {
                "Подключено"
            } else if device.paired {
                "Сопряжено"
            } else {
                "Обнаружено"
            };
            let mut details = connection.to_owned();
            if let Some(battery) = device.battery {
                details.push_str(&format!(" · Заряд {battery}%"));
            }
            if !device.address.is_empty() && device.name != device.address {
                details.push_str(&format!(" · {}", device.address));
            }
            let detail = gtk::Label::new(Some(&details));
            detail.set_xalign(0.);
            detail.set_ellipsize(gtk::pango::EllipsizeMode::End);
            detail.set_tooltip_text(Some(&details));
            detail.add_css_class("dim-label");
            labels.append(&name);
            labels.append(&detail);
            row.append(&labels);
            let target = DeviceTarget {
                owner: snapshot.owner.clone(),
                adapter: adapter.path.clone(),
                path: device.path.clone(),
                address: device.address.clone(),
                identity: if device.address.is_empty() || device.name == device.address {
                    device.name.clone()
                } else {
                    format!("{} ({})", device.name, device.address)
                },
            };
            let action = DeviceAction::for_device(device);
            let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
            actions.set_valign(gtk::Align::Center);
            let enabled = controls_enabled && !target.path.is_empty() && !target.address.is_empty();
            let can_use_action = enabled
                && adapter.powered
                && (action != DeviceAction::Pair || snapshot.pairing_enabled);
            actions.append(&self.device_button(&target, action, can_use_action));
            if action != DeviceAction::Pair {
                actions.append(&self.device_button(&target, DeviceAction::Remove, enabled));
            }
            row.append(&actions);
            self.devices.append(&row);
        }
    }

    fn device_button(
        self: &Rc<Self>,
        target: &DeviceTarget,
        action: DeviceAction,
        enabled: bool,
    ) -> gtk::Button {
        let button = gtk::Button::with_label(action.label());
        let accessible = format!("{} — {}", action.label(), target.identity);
        button.update_property(&[gtk::accessible::Property::Label(&accessible)]);
        button.set_sensitive(enabled);
        let weak = Rc::downgrade(self);
        let target = target.clone();
        button.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.device_action(&target, action);
            }
        });
        button
    }

    fn can_change(&self, target: &DeviceTarget, action: DeviceAction) -> bool {
        let state = self.snapshot.borrow();
        let Some(snapshot) = state
            .as_ref()
            .filter(|snapshot| snapshot.owner == target.owner)
        else {
            return false;
        };
        let Some(adapter) = snapshot.adapters.get(self.adapters.selected() as usize) else {
            return false;
        };
        adapter.path == target.adapter
            && !target.path.is_empty()
            && !target.address.is_empty()
            && (action != DeviceAction::Pair || snapshot.pairing_enabled)
            && adapter.devices.iter().any(|device| {
                device.path == target.path
                    && device.address == target.address
                    && if action == DeviceAction::Remove {
                        device.paired || device.connected
                    } else {
                        adapter.powered && DeviceAction::for_device(device) == action
                    }
            })
    }

    fn device_action(self: &Rc<Self>, target: &DeviceTarget, action: DeviceAction) {
        if self.pending.get() || self.removal.borrow().is_some() || !self.can_change(target, action)
        {
            return;
        }
        match action {
            DeviceAction::Pair => self.start_pairing(target),
            DeviceAction::Remove => {
                self.removal.replace(Some(target.clone()));
                self.render();
            }
            _ => self.change_device(target, action),
        }
    }

    fn change_device(self: &Rc<Self>, target: &DeviceTarget, action: DeviceAction) {
        let action = match action {
            DeviceAction::Connect => "connect",
            DeviceAction::Disconnect => "disconnect",
            DeviceAction::Remove => "remove",
            DeviceAction::Pair => return,
        };
        self.run_action(
            "ChangeBluetoothDevice",
            (
                target.owner.as_str(),
                target.adapter.as_str(),
                target.path.as_str(),
                target.address.as_str(),
                action,
            )
                .to_variant(),
            40000,
        );
    }

    fn render_inline(self: &Rc<Self>) {
        while let Some(child) = self.inline.first_child() {
            self.inline.remove(&child);
        }
        self.inline.set_visible(false);
        let removal = self.removal.borrow();
        let Some(target) = removal.as_ref() else {
            self.render_pairing();
            return;
        };
        let (description, actions) =
            self.inline_controls(&format!("Удалить устройство «{}»?", target.identity));
        let cancel = gtk::Button::with_label("Отмена");
        let weak = Rc::downgrade(self);
        cancel.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.removal.replace(None);
                pane.render();
            }
        });
        let confirm = gtk::Button::with_label("Удалить");
        confirm.add_css_class("destructive-action");
        let weak = Rc::downgrade(self);
        let target = target.clone();
        confirm.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                if pane.removal.borrow().as_ref() != Some(&target) || pane.pending.get() {
                    return;
                }
                pane.removal.replace(None);
                if pane.can_change(&target, DeviceAction::Remove) {
                    pane.change_device(&target, DeviceAction::Remove);
                } else {
                    pane.message.set(Some("Устройство Bluetooth изменилось"));
                    pane.render();
                }
            }
        });
        actions.append(&cancel);
        actions.append(&confirm);
        self.inline.append(&description);
        self.inline.append(&actions);
        self.inline.set_visible(true);
    }

    fn inline_controls(&self, text: &str) -> (gtk::Label, gtk::Box) {
        let description = gtk::Label::new(Some(text));
        description.set_xalign(0.);
        description.set_wrap(true);
        description.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.set_halign(gtk::Align::End);
        (description, actions)
    }

    fn action(self: &Rc<Self>, change_power: bool) {
        if self.pending.get() || self.removal.borrow().is_some() {
            return;
        }
        let state = self.snapshot.borrow();
        let Some(snapshot) = state.as_ref() else {
            return;
        };
        let Some(adapter) = snapshot.adapters.get(self.adapters.selected() as usize) else {
            return;
        };
        let arguments = (snapshot.owner.as_str(), adapter.path.as_str());
        let (method, parameters) = if change_power {
            (
                "SetBluetoothPower",
                (arguments.0, arguments.1, !adapter.powered).to_variant(),
            )
        } else if adapter.searching {
            ("StopBluetoothScan", arguments.to_variant())
        } else {
            ("ScanBluetooth", arguments.to_variant())
        };
        drop(state);
        self.run_action(method, parameters, 12000);
    }

    fn begin_action(self: &Rc<Self>) {
        self.pending.set(true);
        self.generation.set(self.generation.get().wrapping_add(1));
        self.message.set(None);
        self.render();
    }

    fn run_action(self: &Rc<Self>, method: &'static str, parameters: glib::Variant, timeout: i32) {
        self.begin_action();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let reply = call(method, Some(parameters), timeout).await;
            Self::complete_action(weak, reply.err().as_ref().map(action_error)).await;
        });
    }

    async fn complete_action(weak: std::rc::Weak<Self>, message: Option<&'static str>) {
        let snapshot = read_snapshot().await;
        if let Some(pane) = weak.upgrade() {
            pane.pending.set(false);
            pane.message.set(message);
            if !pane.receive(snapshot) {
                pane.render();
            }
        }
    }
}

fn action_error(error: &glib::Error) -> &'static str {
    let name = gio::DBusError::remote_error(error);
    match name.as_deref().and_then(|name| name.rsplit('.').next()) {
        Some("PermissionDenied") => "BlueZ запретил действие",
        Some("Conflict") => "Состояние Bluetooth изменилось",
        Some("Cancelled") => "Сопряжение отменено",
        Some("Timeout") => "Истекло время ожидания Bluetooth",
        _ => "Не удалось выполнить действие Bluetooth",
    }
}

async fn read_snapshot() -> Snapshot {
    call("GetBluetooth", None, 7000)
        .await
        .ok()
        .and_then(|reply| reply.get::<(String,)>())
        .and_then(|(text,)| toml::from_str(&text).ok())
        .unwrap_or_default()
}

impl Drop for BluetoothPane {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.get_mut().take() {
            timer.remove();
        }
        if let Some(pairing) = self.pairing.get_mut().take() {
            pairing.cancel_on_close();
        }
    }
}

async fn call(
    method: &str,
    parameters: Option<glib::Variant>,
    timeout: i32,
) -> Result<glib::Variant, glib::Error> {
    let connection = gio::bus_get_future(gio::BusType::Session).await?;
    call_on(&connection, method, parameters, timeout).await
}

async fn call_on(
    connection: &gio::DBusConnection,
    method: &str,
    parameters: Option<glib::Variant>,
    timeout: i32,
) -> Result<glib::Variant, glib::Error> {
    connection
        .call_future(
            Some(BUS),
            PATH,
            BUS,
            method,
            parameters.as_ref(),
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            timeout,
        )
        .await
}
