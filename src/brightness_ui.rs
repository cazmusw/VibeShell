use crate::{
    audio::{BUS, PATH},
    brightness::{Access, Snapshot},
};
use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    time::Duration,
};

const EDIT_DELAY: Duration = Duration::from_millis(150);
const POLL_INTERVAL: Duration = Duration::from_secs(1);

thread_local! {
    // The previous pane's write can still be running after the window closes.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

#[derive(Clone, PartialEq, Eq)]
struct Target {
    owner: String,
    session: String,
    name: String,
    identity: String,
    max: u32,
}

struct Change {
    target: Target,
    percent: u32,
}

#[derive(Clone, Copy)]
enum WriteError {
    PermissionDenied,
    Conflict,
    Failed,
}

pub struct BrightnessPane {
    pub widget: gtk::Box,
    devices: gtk::DropDown,
    controls: gtk::Box,
    slider: gtk::Scale,
    confirmed: gtk::Label,
    status: gtk::Label,
    snapshot: RefCell<Snapshot>,
    selected_name: RefCell<Option<String>>,
    queued: RefCell<Option<Change>>,
    in_flight: RefCell<Option<Change>>,
    denied: RefCell<Vec<Target>>,
    message: Cell<Option<&'static str>>,
    syncing: Cell<bool>,
    reading: Cell<bool>,
    debounce: RefCell<Option<glib::SourceId>>,
    poll: RefCell<Option<glib::SourceId>>,
}

impl BrightnessPane {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        widget.set_margin_top(16);
        widget.set_margin_bottom(16);
        widget.set_margin_start(20);
        widget.set_margin_end(20);
        let devices = gtk::DropDown::from_strings(&[]);
        devices.update_property(&[gtk::accessible::Property::Label("Устройство подсветки")]);
        devices.set_visible(false);
        widget.append(&devices);

        let controls = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let slider = gtk::Scale::with_range(gtk::Orientation::Horizontal, 1., 100., 1.);
        slider.set_draw_value(false);
        slider.set_increments(1., 10.);
        slider.set_round_digits(0);
        slider.set_hexpand(true);
        slider.update_property(&[gtk::accessible::Property::Label("Яркость экрана")]);
        let confirmed = gtk::Label::new(None);
        confirmed.set_width_chars(4);
        confirmed.set_xalign(1.);
        controls.append(&slider);
        controls.append(&confirmed);
        controls.set_visible(false);
        widget.append(&controls);

        let status = gtk::Label::new(Some("Проверка подсветки…"));
        status.set_xalign(0.);
        status.set_wrap(true);
        widget.append(&status);
        let pane = Rc::new(Self {
            widget,
            devices,
            controls,
            slider,
            confirmed,
            status,
            snapshot: RefCell::new(Snapshot::default()),
            selected_name: RefCell::new(None),
            queued: RefCell::new(None),
            in_flight: RefCell::new(None),
            denied: RefCell::new(Vec::new()),
            message: Cell::new(None),
            syncing: Cell::new(false),
            reading: Cell::new(false),
            debounce: RefCell::new(None),
            poll: RefCell::new(None),
        });
        let weak = Rc::downgrade(&pane);
        pane.devices.connect_selected_notify(move |_| {
            if let Some(pane) = weak.upgrade().filter(|pane| !pane.syncing.get()) {
                if let Some((target, _)) = pane.current() {
                    pane.selected_name.replace(Some(target.name));
                }
                pane.cancel_edit();
                pane.message.set(None);
                pane.render();
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.slider.connect_value_changed(move |slider| {
            if let Some(pane) = weak.upgrade().filter(|pane| !pane.syncing.get()) {
                pane.queue_edit(slider.value().round() as u32);
            }
        });
        let weak = Rc::downgrade(&pane);
        pane.slider.connect_change_value(move |slider, _, value| {
            // At a confirmed zero, the thumb is already at the minimum.
            if value <= 1.
                && slider.value() == 1.
                && let Some(pane) = weak.upgrade()
                && pane.current().is_some_and(|(_, confirmed)| confirmed == 0)
            {
                pane.queue_edit(1);
            }
            glib::Propagation::Proceed
        });
        pane.refresh();
        let weak = Rc::downgrade(&pane);
        pane.poll
            .replace(Some(glib::timeout_add_local(POLL_INTERVAL, move || {
                let Some(pane) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                pane.refresh();
                glib::ControlFlow::Continue
            })));
        pane
    }

    fn current(&self) -> Option<(Target, u32)> {
        let snapshot = self.snapshot.borrow();
        if !matches!(snapshot.access, Access::Ready) {
            return None;
        }
        let device = snapshot.devices.get(self.devices.selected() as usize)?;
        Some((
            Target {
                owner: snapshot.owner.clone(),
                session: snapshot.session.clone(),
                name: device.name.clone(),
                identity: device.identity.clone(),
                max: device.max,
            },
            device.percent,
        ))
    }

    fn denied(&self, target: &Target) -> bool {
        self.denied.borrow().contains(target)
    }

    fn refresh(self: &Rc<Self>) {
        if WRITING.get() || self.reading.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let snapshot = read_snapshot().await;
            if let Some(pane) = weak.upgrade() {
                pane.reading.set(false);
                pane.receive(snapshot);
                pane.try_write();
            }
        });
    }

    fn receive(&self, snapshot: Snapshot) {
        let previous_target = self.current().map(|(target, _)| target);
        let model = self
            .devices
            .model()
            .and_downcast::<gtk::StringList>()
            .unwrap();
        let names = snapshot
            .devices
            .iter()
            .map(|device| device.name.as_str())
            .collect::<Vec<_>>();
        let selected = self
            .selected_name
            .borrow()
            .as_deref()
            .and_then(|name| names.iter().position(|item| *item == name))
            .or_else(|| (!names.is_empty()).then_some(0));
        self.syncing.set(true);
        if model.n_items() as usize != names.len()
            || names
                .iter()
                .enumerate()
                .any(|(index, name)| model.string(index as u32).as_deref() != Some(*name))
        {
            model.splice(0, model.n_items(), &names);
        }
        self.devices
            .set_selected(selected.map_or(gtk::INVALID_LIST_POSITION, |index| index as u32));
        if let Some(index) = selected {
            self.selected_name.replace(Some(names[index].to_owned()));
        }
        self.syncing.set(false);

        let previous = self.snapshot.replace(snapshot);
        let snapshot = self.snapshot.borrow();
        if (previous.owner != snapshot.owner || previous.session != snapshot.session)
            && matches!(snapshot.access, Access::Ready)
        {
            self.denied.borrow_mut().clear();
        }
        drop(snapshot);
        let current = self.current();
        if previous_target.as_ref() != current.as_ref().map(|(target, _)| target) {
            self.message.set(None);
        }
        let edit_stale = self.queued.borrow().as_ref().is_some_and(|edit| {
            !current
                .as_ref()
                .is_some_and(|(target, _)| *target == edit.target && !self.denied(target))
        });
        if edit_stale {
            self.cancel_edit();
        }
        self.render();
    }

    fn render(&self) {
        let snapshot = self.snapshot.borrow();
        let current = self.current();
        self.devices
            .set_visible(current.is_some() && snapshot.devices.len() > 1);
        let mut status = Some(
            match snapshot.access {
                Access::NoDevice | Access::Ready => "Устройства подсветки не найдены",
                Access::NoSession => "Яркость доступна только в активном локальном сеансе",
                Access::NoService => "Служба яркости недоступна",
            }
            .to_owned(),
        );
        let show_controls = if let Some((target, percent)) = current {
            if self.denied(&target) {
                status = Some(format!(
                    "Нет разрешения на изменение яркости. Текущий уровень {percent}%"
                ));
                false
            } else {
                let queued = self.queued.borrow();
                let in_flight = self.in_flight.borrow();
                let preview = queued
                    .as_ref()
                    .filter(|edit| edit.target == target)
                    .or_else(|| in_flight.as_ref().filter(|edit| edit.target == target));
                self.syncing.set(true);
                self.slider.set_value(f64::from(
                    preview.map_or(percent, |edit| edit.percent).max(1),
                ));
                self.syncing.set(false);
                self.confirmed.set_text(&format!("{percent}%"));
                status = self.message.get().map(str::to_owned);
                true
            }
        } else {
            false
        };
        self.controls.set_visible(show_controls);
        self.status.set_text(status.as_deref().unwrap_or_default());
        self.status.set_visible(status.is_some());
    }

    fn queue_edit(self: &Rc<Self>, percent: u32) {
        let Some((target, _)) = self.current().filter(|(target, _)| !self.denied(target)) else {
            return;
        };
        self.cancel_edit();
        self.queued.replace(Some(Change { target, percent }));
        self.message.set(None);
        self.render();
        let weak = Rc::downgrade(self);
        self.debounce
            .replace(Some(glib::timeout_add_local_once(EDIT_DELAY, move || {
                if let Some(pane) = weak.upgrade() {
                    pane.debounce.take();
                    pane.try_write();
                }
            })));
    }

    fn cancel_edit(&self) {
        self.queued.take();
        if let Some(source) = self.debounce.take() {
            source.remove();
        }
    }

    fn try_write(self: &Rc<Self>) {
        if self.debounce.borrow().is_some() || self.reading.get() || WRITING.get() {
            return;
        }
        let Some(edit) = self.queued.take() else {
            return;
        };
        let eligible = self.current().is_some_and(|(target, percent)| {
            edit.target == target && edit.percent != percent && !self.denied(&target)
        });
        if !eligible {
            self.render();
            return;
        }
        let parameters = (
            edit.target.owner.as_str(),
            edit.target.session.as_str(),
            edit.target.name.as_str(),
            edit.target.identity.as_str(),
            edit.target.max,
            edit.percent,
        )
            .to_variant();
        self.in_flight.replace(Some(edit));
        WRITING.set(true);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = call("SetBrightness", Some(parameters), 12000).await;
            let snapshot = if weak.strong_count() > 0 {
                Some(read_snapshot().await)
            } else {
                None
            };
            WRITING.set(false);
            if let (Some(pane), Some(snapshot)) = (weak.upgrade(), snapshot) {
                pane.finish_write(snapshot, result.as_ref().err().map(write_error));
            }
        });
    }

    fn finish_write(self: &Rc<Self>, snapshot: Snapshot, error: Option<WriteError>) {
        let Some(edit) = self.in_flight.take() else {
            return;
        };
        self.receive(snapshot);
        if let Some(error) = error {
            let current = self.current();
            if matches!(error, WriteError::PermissionDenied) {
                if current.is_some_and(|(target, _)| target == edit.target) {
                    self.cancel_edit();
                }
                self.denied.borrow_mut().push(edit.target);
            } else if current.is_some_and(|(target, _)| target == edit.target) {
                self.cancel_edit();
                self.message.set(Some(match error {
                    WriteError::PermissionDenied => "Нет разрешения на изменение яркости",
                    WriteError::Conflict => "Состояние подсветки изменилось",
                    WriteError::Failed => "Не удалось изменить яркость",
                }));
            }
            self.render();
        }
        self.try_write();
    }
}

impl Drop for BrightnessPane {
    fn drop(&mut self) {
        self.cancel_edit();
        if let Some(poll) = self.poll.get_mut().take() {
            poll.remove();
        }
    }
}

fn write_error(error: &glib::Error) -> WriteError {
    let name = gio::DBusError::remote_error(error);
    match name.as_deref().and_then(|name| name.rsplit('.').next()) {
        Some("PermissionDenied" | "AccessDenied" | "NotAuthorized") => WriteError::PermissionDenied,
        Some("Conflict") => WriteError::Conflict,
        _ => WriteError::Failed,
    }
}

async fn read_snapshot() -> Snapshot {
    call("GetBrightness", None, 7000)
        .await
        .ok()
        .and_then(|reply| reply.get::<(String,)>())
        .and_then(|(text,)| toml::from_str(&text).ok())
        .unwrap_or_default()
}

async fn call(
    method: &str,
    parameters: Option<glib::Variant>,
    timeout: i32,
) -> Result<glib::Variant, glib::Error> {
    let connection = gio::bus_get_future(gio::BusType::Session).await?;
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
