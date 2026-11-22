use crate::{
    audio::{BUS, PATH},
    power::{BatteryState, Profile, Snapshot},
};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub struct PowerPane {
    pub widget: gtk::Box,
    battery: gtk::Label,
    line_power: gtk::Label,
    profiles: [(Profile, gtk::Button); 3],
    profile_status: gtk::Label,
    notice: gtk::Label,
    snapshot: RefCell<Option<Snapshot>>,
    reading: Cell<bool>,
    writing: Cell<bool>,
    revision: Cell<u32>,
    timer: RefCell<Option<glib::SourceId>>,
}

impl PowerPane {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.set_margin_top(16);
        widget.set_margin_bottom(16);
        widget.set_margin_start(20);
        widget.set_margin_end(20);
        let battery = label("Проверка питания…");
        let line_power = label("");
        widget.append(&battery);
        widget.append(&line_power);
        let heading = label("Профиль питания");
        heading.add_css_class("heading");
        heading.set_margin_top(8);
        widget.append(&heading);
        let profile_status = label("");
        widget.append(&profile_status);
        let profiles =
            [Profile::PowerSaver, Profile::Balanced, Profile::Performance].map(|profile| {
                let button = gtk::Button::with_label(profile.title());
                button.set_visible(false);
                widget.append(&button);
                (profile, button)
            });
        let notice = label("");
        widget.append(&notice);
        let pane = Rc::new(Self {
            widget,
            battery,
            line_power,
            profiles,
            profile_status,
            notice,
            snapshot: RefCell::new(None),
            reading: Cell::new(false),
            writing: Cell::new(false),
            revision: Cell::new(0),
            timer: RefCell::new(None),
        });
        for (profile, button) in &pane.profiles {
            let weak = Rc::downgrade(&pane);
            let profile = *profile;
            button.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.change(profile);
                }
            });
        }
        let weak = Rc::downgrade(&pane);
        *pane.timer.borrow_mut() = Some(glib::timeout_add_seconds_local(4, move || {
            let Some(pane) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            pane.refresh();
            glib::ControlFlow::Continue
        }));
        pane.refresh();
        pane
    }

    fn refresh(self: &Rc<Self>) {
        if self.writing.get() || self.reading.replace(true) {
            return;
        }
        let revision = self.revision.get();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let snapshot = read_snapshot().await;
            let Some(pane) = weak.upgrade() else { return };
            pane.reading.set(false);
            if pane.revision.get() == revision && pane.snapshot.borrow().as_ref() != Some(&snapshot)
            {
                pane.snapshot.replace(Some(snapshot));
                pane.render();
            }
        });
    }

    fn change(self: &Rc<Self>, profile: Profile) {
        let Some(current) = self
            .snapshot
            .borrow()
            .as_ref()
            .and_then(|snapshot| snapshot.profiles.clone())
        else {
            return;
        };
        if current.active == profile
            || !current.offered.contains(&profile)
            || self.writing.replace(true)
        {
            return;
        }
        self.revision.set(self.revision.get().wrapping_add(1));
        self.notice.set_text("Запрос смены профиля…");
        self.render();
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let parameters = (current.owner, current.active.id(), profile.id()).to_variant();
            let result = call("SetPowerProfile", Some(&parameters)).await;
            let snapshot = read_snapshot().await;
            let Some(pane) = weak.upgrade() else { return };
            let message = match result {
                Ok(_) => "",
                Err(error) => {
                    let remote = gio::DBusError::remote_error(&error);
                    match remote.as_deref() {
                        Some("org.mosaic.Error1.PermissionDenied") => {
                            "Нет разрешения на смену профиля"
                        }
                        Some("org.mosaic.Error1.Conflict") => {
                            "Состояние питания изменилось. Выберите профиль ещё раз."
                        }
                        _ => "Не удалось изменить профиль питания",
                    }
                }
            };
            pane.notice.set_text(message);
            pane.writing.set(false);
            pane.snapshot.replace(Some(snapshot));
            pane.render();
        });
    }

    fn render(&self) {
        let snapshot = self.snapshot.borrow();
        let Some(snapshot) = snapshot.as_ref() else {
            return;
        };
        let battery = match &snapshot.battery {
            Some(battery) => {
                let status = match battery.state {
                    BatteryState::Unknown => "Состояние неизвестно",
                    BatteryState::Charging => "Заряжается",
                    BatteryState::Discharging => "Разряжается",
                    BatteryState::Empty => "Разряжена",
                    BatteryState::Full => "Заряжена",
                    BatteryState::PendingCharge => "Ожидает зарядки",
                    BatteryState::PendingDischarge => "Ожидает разрядки",
                };
                let mut message = format!("Батарея {}% · {status}", battery.percent);
                if let Some(seconds) = battery.seconds {
                    let minutes = seconds.div_ceil(60);
                    let estimate = match battery.state {
                        BatteryState::Charging => "До полного заряда",
                        _ => "До разрядки",
                    };
                    message.push_str(&format!(
                        "\n{estimate} около {} ч {} мин",
                        minutes / 60,
                        minutes % 60
                    ));
                }
                message
            }
            None if !snapshot.upower_available => "UPower недоступен".into(),
            None if snapshot.battery_present == Some(false) => "Батарея не обнаружена".into(),
            None => "Показания батареи недоступны".into(),
        };
        self.battery.set_text(&battery);
        self.line_power.set_text(match snapshot.on_battery {
            Some(true) => "Питание от батареи",
            Some(false) => "Внешнее питание",
            None => "",
        });
        let mode = snapshot.profiles.as_ref();
        self.profile_status.set_text(match mode {
            None => "Служба профилей питания недоступна",
            Some(profile) if profile.degraded => "Производительность ограничена системой",
            Some(_) => "",
        });
        for (profile, button) in &self.profiles {
            let visible = mode.is_some_and(|mode| mode.offered.contains(profile));
            let active = mode.is_some_and(|mode| mode.active == *profile);
            button.set_visible(visible);
            button.set_sensitive(visible && !active && !self.writing.get());
            let title = if active {
                format!("{} · Активен", profile.title())
            } else {
                profile.title().into()
            };
            button.set_label(&title);
        }
    }
}

impl Drop for PowerPane {
    fn drop(&mut self) {
        if let Some(timer) = self.timer.get_mut().take() {
            timer.remove();
        }
    }
}

fn label(text: &str) -> gtk::Label {
    let label = gtk::Label::new(Some(text));
    label.set_xalign(0.0);
    label.set_wrap(true);
    label
}

async fn read_snapshot() -> Snapshot {
    call("GetPower", None)
        .await
        .ok()
        .and_then(|reply| reply.get::<(String,)>())
        .and_then(|(text,)| toml::from_str(&text).ok())
        .unwrap_or_default()
}

async fn call(
    method: &str,
    parameters: Option<&glib::Variant>,
) -> Result<glib::Variant, glib::Error> {
    gio::bus_get_future(gio::BusType::Session)
        .await?
        .call_future(
            Some(BUS),
            PATH,
            BUS,
            method,
            parameters,
            None,
            gio::DBusCallFlags::NO_AUTO_START,
            6000,
        )
        .await
}
