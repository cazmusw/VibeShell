mod profiles;

use crate::{
    audio::{BUS, PATH},
    network::{PersonalMode, Protection, WifiAdapter, WifiNetwork, WifiSnapshot},
};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::{Duration, Instant},
};

enum Operation {
    Scan(Option<i64>),
    Connect {
        network: WifiNetwork,
        active: String,
    },
    Disconnect,
}

impl Operation {
    fn busy_message(&self) -> &'static str {
        match self {
            Self::Scan(_) => "Поиск сетей…",
            Self::Connect { .. } => "Подключение…",
            Self::Disconnect => "Отключение…",
        }
    }

    fn outcome(&self, adapter: &WifiAdapter) -> Option<&'static str> {
        match self {
            Self::Scan(Some(previous)) if adapter.last_scan > *previous => {
                Some("Список сетей обновлён")
            }
            Self::Connect {
                active,
                network: target,
            } => {
                if adapter.active_connection == *active
                    && adapter.networks.iter().any(|network| {
                        network.connected
                            && network.ssid == target.ssid
                            && network.protection == target.protection
                    })
                {
                    Some("Подключено")
                } else if let Some(failure) = &adapter.activation_failure
                    && failure.active == *active
                {
                    if failure.reason == 7 {
                        Some("NetworkManager не получил пароль. Проверьте агент секретов.")
                    } else {
                        Some("NetworkManager не смог подключиться")
                    }
                } else {
                    None
                }
            }
            Self::Disconnect if adapter.active_connection.is_empty() => Some("Отключено"),
            _ => None,
        }
    }
}

struct PendingOperation {
    path: String,
    operation: Operation,
    started: Instant,
    accepted: bool,
}

struct ProfileOffer {
    path: String,
    network: WifiNetwork,
    mode: PersonalMode,
}

pub struct WifiPane {
    widget: gtk::Box,
    status: gtk::Label,
    adapters: gtk::DropDown,
    scan: gtk::Button,
    results: gtk::ListBox,
    offer_box: gtk::Box,
    offer_title: gtk::Label,
    offer: RefCell<Option<ProfileOffer>>,
    snapshot: RefCell<Option<WifiSnapshot>>,
    connection: RefCell<Option<gio::DBusConnection>>,
    pending: RefCell<Option<PendingOperation>>,
    message: RefCell<String>,
    sequence: Cell<u64>,
    ticking: RefCell<Option<glib::SourceId>>,
    refreshing: Cell<bool>,
    syncing: Cell<bool>,
}

impl WifiPane {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.set_margin_top(16);
        widget.set_margin_bottom(16);
        widget.set_margin_start(20);
        widget.set_margin_end(20);
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let networks = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);
        stack.add_named(&networks, Some("networks"));
        widget.append(&stack);
        let adapters = gtk::DropDown::from_strings(&[]);
        adapters.set_hexpand(true);
        adapters.update_property(&[gtk::accessible::Property::Label("Адаптер Wi-Fi")]);
        let scan = gtk::Button::with_label("Обновить");
        scan.set_sensitive(false);
        let profiles_button = gtk::Button::with_label("Профили");
        row.append(&adapters);
        row.append(&profiles_button);
        row.append(&scan);
        networks.append(&row);

        let status = gtk::Label::new(Some("Загрузка…"));
        status.set_xalign(0.);
        status.set_wrap(true);
        networks.append(&status);
        let offer_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        offer_box.set_visible(false);
        let offer_title = gtk::Label::new(None);
        offer_title.set_wrap(true);
        offer_title.set_xalign(0.);
        offer_box.append(&offer_title);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let confirm = gtk::Button::with_label("Создать профиль");
        let cancel = gtk::Button::with_label("Отмена");
        actions.append(&confirm);
        actions.append(&cancel);
        offer_box.append(&actions);
        networks.append(&offer_box);
        let results = gtk::ListBox::new();
        results.set_selection_mode(gtk::SelectionMode::None);
        let scroll = gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&results)
            .build();
        networks.append(&scroll);

        let profiles = profiles::ProfilePane::new();
        stack.add_named(&profiles.widget, Some("profiles"));
        let weak_stack = stack.downgrade();
        profiles.back.connect_clicked(move |_| {
            if let Some(stack) = weak_stack.upgrade() {
                stack.set_visible_child_name("networks");
            }
        });
        let weak_stack = stack.downgrade();
        profiles_button.connect_clicked(move |_| {
            if let Some(stack) = weak_stack.upgrade() {
                stack.set_visible_child_name("profiles");
                profiles.load(None);
            }
        });

        let pane = Rc::new(Self {
            widget,
            status,
            adapters,
            scan,
            results,
            offer_box,
            offer_title,
            offer: RefCell::new(None),
            snapshot: RefCell::new(None),
            connection: RefCell::new(None),
            pending: RefCell::new(None),
            message: RefCell::new(String::new()),
            sequence: Cell::new(0),
            ticking: RefCell::new(None),
            refreshing: Cell::new(false),
            syncing: Cell::new(false),
        });
        pane.adapters.set_sensitive(false);
        let weak = Rc::downgrade(&pane);
        pane.adapters.connect_selected_notify(move |_| {
            if let Some(pane) = weak.upgrade().filter(|pane| !pane.syncing.get()) {
                pane.message.borrow_mut().clear();
                pane.offer.borrow_mut().take();
                pane.render_networks();
                pane.render_status();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.scan.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.request_scan();
            }
        });
        let weak = Rc::downgrade(&pane);
        confirm.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.accept_profile();
            }
        });
        let weak = Rc::downgrade(&pane);
        cancel.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.offer.borrow_mut().take();
                pane.render_status();
            }
        });
        let weak = Rc::downgrade(&pane);
        glib::MainContext::default().spawn_local(async move {
            let connection = gio::bus_get_future(gio::BusType::Session).await;
            let Some(pane) = weak.upgrade() else { return };
            if let Ok(connection) = connection {
                *pane.connection.borrow_mut() = Some(connection);
                pane.refresh();
                let weak = Rc::downgrade(&pane);
                *pane.ticking.borrow_mut() =
                    Some(glib::timeout_add_local(Duration::from_secs(2), move || {
                        let Some(pane) = weak.upgrade() else {
                            return glib::ControlFlow::Break;
                        };
                        pane.refresh();
                        glib::ControlFlow::Continue
                    }));
            } else {
                pane.receive(WifiSnapshot::default());
            }
        });
        pane
    }

    pub fn widget(&self) -> &gtk::Box {
        &self.widget
    }

    fn selected<'a>(&self, snapshot: &'a WifiSnapshot) -> Option<&'a WifiAdapter> {
        snapshot.adapters.get(self.adapters.selected() as usize)
    }

    fn refresh(self: &Rc<Self>) {
        if self.refreshing.replace(true) {
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
                    "GetWifi",
                    None,
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    3500,
                )
                .await;
            if let Some(pane) = weak.upgrade() {
                pane.refreshing.set(false);
                let snapshot = result
                    .ok()
                    .and_then(|reply| reply.get::<(String,)>())
                    .and_then(|(text,)| toml::from_str(&text).ok())
                    .unwrap_or_default();
                pane.receive(snapshot);
            }
        });
    }

    fn receive(self: &Rc<Self>, snapshot: WifiSnapshot) {
        {
            let pending = self.pending.borrow();
            if let Some(operation) = pending.as_ref() {
                let adapter = snapshot
                    .adapters
                    .iter()
                    .find(|adapter| adapter.path == operation.path);
                let deadline = match operation.operation {
                    Operation::Scan(_) => Duration::from_secs(30),
                    Operation::Connect { .. } => Duration::from_secs(120),
                    Operation::Disconnect => Duration::from_secs(40),
                };
                let outcome =
                    if !snapshot.available || !snapshot.enabled || !snapshot.hardware_enabled {
                        Some("Wi-Fi недоступен")
                    } else if let Some(adapter) = adapter {
                        if operation.accepted {
                            operation.operation.outcome(adapter).or_else(|| {
                                (operation.started.elapsed() >= deadline)
                                    .then_some("NetworkManager не подтвердил завершение операции")
                            })
                        } else {
                            None
                        }
                    } else {
                        Some("Адаптер отключён")
                    };
                if let Some(outcome) = outcome {
                    *self.message.borrow_mut() = outcome.into();
                    drop(pending);
                    self.pending.borrow_mut().take();
                }
            }
        }
        let previous = self.snapshot.replace(Some(snapshot));
        let current = self.snapshot.borrow();
        let snapshot = current.as_ref().unwrap();
        if previous.as_ref() != Some(snapshot) {
            let old_path = previous
                .as_ref()
                .and_then(|old| self.selected(old))
                .map(|old| &old.path);
            let model = self
                .adapters
                .model()
                .and_downcast::<gtk::StringList>()
                .unwrap();
            let names = snapshot
                .adapters
                .iter()
                .map(|adapter| adapter.name.as_str())
                .collect::<Vec<_>>();
            self.syncing.set(true);
            if model.n_items() as usize != names.len()
                || names
                    .iter()
                    .enumerate()
                    .any(|(i, name)| model.string(i as u32).as_deref() != Some(name))
            {
                model.splice(0, model.n_items(), &names);
            }
            let selection = snapshot
                .adapters
                .iter()
                .position(|adapter| Some(&adapter.path) == old_path)
                .or_else(|| snapshot.adapters.iter().position(|adapter| adapter.managed))
                .or_else(|| (!names.is_empty()).then_some(0));
            self.adapters
                .set_selected(selection.map_or(gtk::INVALID_LIST_POSITION, |i| i as u32));
            self.syncing.set(false);
            drop(current);
            self.render_networks();
        }
        self.render_status();
    }

    fn render_networks(self: &Rc<Self>) {
        while let Some(row) = self.results.first_child() {
            self.results.remove(&row);
        }
        let state = self.snapshot.borrow();
        let Some(adapter) = state.as_ref().and_then(|snapshot| self.selected(snapshot)) else {
            return;
        };
        for network in &adapter.networks {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.set_margin_top(5);
            row.set_margin_bottom(5);
            let title = gtk::Label::new(Some(&network.name));
            title.set_hexpand(true);
            title.set_xalign(0.);
            title.set_ellipsize(gtk::pango::EllipsizeMode::End);
            title.set_tooltip_text(Some(&network.name));
            row.append(&title);
            let protection = match network.protection {
                Protection::Open => "Открытая",
                Protection::Secured => "Защищённая",
            };
            let detail = if network.connected {
                "Подключено"
            } else {
                protection
            };
            let detail = gtk::Label::new(Some(detail));
            detail.add_css_class("dim-label");
            row.append(&detail);
            let signal = gtk::Label::new(Some(&format!("{}%", network.strength)));
            signal.set_width_chars(4);
            signal.set_xalign(1.);
            row.append(&signal);
            if adapter.managed && (network.connected || !network.ssid.is_empty()) {
                let action = if network.connected {
                    "Отключить"
                } else {
                    "Подключить"
                };
                let button = gtk::Button::with_label(action);
                let accessible_name = format!("{action} — {}", network.name);
                button.update_property(&[gtk::accessible::Property::Label(&accessible_name)]);
                let weak = Rc::downgrade(self);
                let network = network.clone();
                button.connect_clicked(move |_| {
                    if let Some(pane) = weak.upgrade() {
                        pane.request_connection(&network);
                    }
                });
                row.append(&button);
            }
            self.results.append(&row);
            if let Some(parent) = row.parent().and_downcast::<gtk::ListBoxRow>() {
                parent.set_focusable(false);
                parent.set_activatable(false);
            }
        }
    }

    fn render_status(&self) {
        let state = self.snapshot.borrow();
        let Some(snapshot) = state.as_ref() else {
            return;
        };
        let adapter = self.selected(snapshot);
        let pending = self.pending.borrow();
        let message = self.message.borrow();
        let usable = snapshot.enabled
            && snapshot.hardware_enabled
            && adapter.is_some_and(|adapter| adapter.managed);
        self.adapters
            .set_sensitive(snapshot.adapters.len() > 1 && pending.is_none());
        self.results.set_sensitive(usable && pending.is_none());
        let offer = self.offer.borrow();
        let active_offer = offer.as_ref().filter(|offer| {
            usable && pending.is_none() && adapter.is_some_and(|adapter| adapter.path == offer.path)
        });
        if let Some(offer) = active_offer {
            self.offer_title.set_text(&format!(
                "Создать профиль для «{}»? Пароль запросит агент NetworkManager.",
                offer.network.name,
            ));
        }
        self.offer_box.set_visible(active_offer.is_some());
        self.scan.set_sensitive(
            snapshot.enabled
                && snapshot.hardware_enabled
                && adapter.is_some_and(|adapter| adapter.managed)
                && pending.is_none(),
        );
        let text = if !snapshot.available {
            "NetworkManager недоступен"
        } else if snapshot.adapters.is_empty() {
            "Адаптеры Wi-Fi не найдены"
        } else if !snapshot.hardware_enabled {
            "Wi-Fi отключён аппаратно"
        } else if !snapshot.enabled {
            "Wi-Fi выключен"
        } else if adapter.is_some_and(|adapter| !adapter.managed) {
            "NetworkManager не управляет этим адаптером"
        } else if let Some(pending) = pending.as_ref() {
            pending.operation.busy_message()
        } else if !message.is_empty() {
            message.as_str()
        } else if adapter.is_some_and(|adapter| adapter.networks.is_empty()) {
            "Сети не найдены"
        } else {
            ""
        };
        self.status.set_text(text);
        self.status.set_visible(!text.is_empty());
    }

    fn request_scan(self: &Rc<Self>) {
        let Some(path) = self
            .snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| self.selected(snapshot))
            .map(|adapter| adapter.path.clone())
        else {
            return;
        };
        let parameters = (path.as_str(),).to_variant();
        self.request(
            path,
            Operation::Scan(None),
            "ScanWifiWithBaseline",
            parameters,
        );
    }

    fn request_connection(self: &Rc<Self>, network: &WifiNetwork) {
        let Some(adapter) = self
            .snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| self.selected(snapshot))
            .cloned()
        else {
            return;
        };
        if network.connected {
            let parameters = (adapter.path.as_str(), adapter.active_connection).to_variant();
            self.request(
                adapter.path,
                Operation::Disconnect,
                "DisconnectWifi",
                parameters,
            );
        } else {
            let parameters = (
                adapter.path.as_str(),
                network.ssid.as_slice(),
                network.protection == Protection::Secured,
            )
                .to_variant();
            let operation = Operation::Connect {
                network: network.clone(),
                active: String::new(),
            };
            self.request(adapter.path, operation, "ConnectWifi", parameters);
        }
    }

    fn accept_profile(self: &Rc<Self>) {
        let Some(offer) = self.offer.borrow_mut().take() else {
            return;
        };
        let parameters = (
            offer.path.as_str(),
            offer.network.ssid.as_slice(),
            offer.mode.key_mgmt(),
        )
            .to_variant();
        let operation = Operation::Connect {
            network: offer.network,
            active: String::new(),
        };
        self.request(offer.path, operation, "JoinWifi", parameters);
    }

    fn request(
        self: &Rc<Self>,
        path: String,
        operation: Operation,
        method: &'static str,
        parameters: glib::Variant,
    ) {
        let Some(connection) = self.connection.borrow().clone() else {
            return;
        };
        if self.pending.borrow().is_some() {
            return;
        }
        let sequence = self.sequence.get().wrapping_add(1);
        self.sequence.set(sequence);
        self.offer.borrow_mut().take();
        *self.pending.borrow_mut() = Some(PendingOperation {
            path,
            operation,
            started: Instant::now(),
            accepted: false,
        });
        self.message.borrow_mut().clear();
        self.render_status();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = connection
                .call_future(
                    Some(BUS),
                    PATH,
                    BUS,
                    method,
                    Some(&parameters),
                    None,
                    gio::DBusCallFlags::NO_AUTO_START,
                    50000,
                )
                .await;
            let Some(pane) = weak
                .upgrade()
                .filter(|pane| pane.sequence.get() == sequence)
            else {
                return;
            };
            let mut slot = pane.pending.borrow_mut();
            if let Some(pending) = slot.as_mut() {
                let error = match result {
                    Ok(reply) => {
                        if let Operation::Scan(baseline) = &mut pending.operation {
                            *baseline = reply.get::<(i64,)>().map(|(last_scan,)| last_scan);
                        }
                        if let Operation::Connect { active, .. } = &mut pending.operation {
                            *active = reply
                                .get::<(String,)>()
                                .map(|(path,)| path)
                                .unwrap_or_default();
                        }
                        pending.accepted = true;
                        pending.started = Instant::now();
                        matches!(pending.operation, Operation::Scan(None))
                            .then_some("Не удалось подтвердить запрос NetworkManager")
                    }
                    Err(error) => {
                        let detail = error.to_string();
                        let message = if detail.contains("NoSavedProfile") {
                            if method == "ConnectWifi"
                                && let Operation::Connect { network, .. } = &pending.operation
                                && let Some(mode) = network.personal
                            {
                                *pane.offer.borrow_mut() = Some(ProfileOffer {
                                    path: pending.path.clone(),
                                    network: network.clone(),
                                    mode,
                                });
                            }
                            "Для этой сети нет сохранённого профиля"
                        } else if detail.contains("SecretsUnavailable") {
                            "NetworkManager не получил пароль. Проверьте агент секретов."
                        } else if detail.contains("PermissionDenied") {
                            "NetworkManager запретил операцию"
                        } else if detail.contains("Conflict") {
                            "Состояние изменилось или операция уже выполняется"
                        } else {
                            "Не удалось подтвердить запрос NetworkManager"
                        };
                        Some(message)
                    }
                };
                if let Some(error) = error {
                    slot.take();
                    *pane.message.borrow_mut() = error.into();
                }
            }
            drop(slot);
            pane.render_status();
            pane.refresh();
        });
    }
}

impl Drop for WifiPane {
    fn drop(&mut self) {
        if let Some(timer) = self.ticking.get_mut().take() {
            timer.remove();
        }
    }
}
