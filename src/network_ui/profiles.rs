use crate::{
    audio::{BUS, PATH},
    network::{WifiProfile, WifiProfiles},
};
use gtk::{gio, glib, prelude::*};
use gtk4_layer_shell::{KeyboardMode, Layer, LayerShell};
use std::{
    cell::{Cell, RefCell},
    ffi::OsStr,
    rc::Rc,
};

pub struct ProfilePane {
    pub widget: gtk::Box,
    pub back: gtk::Button,
    list: gtk::ListBox,
    status: gtk::Label,
    refresh: gtk::Button,
    busy: Cell<bool>,
    confirmation: gtk::Box,
    prompt: gtk::Label,
    offer: RefCell<Option<(String, WifiProfile)>>,
}

impl ProfilePane {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 10);
        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let back = gtk::Button::with_label("К сетям");
        let editor = gtk::Button::with_label("Внешний редактор");
        let refresh = gtk::Button::with_label("Обновить");
        actions.append(&back);
        actions.append(&editor);
        actions.append(&refresh);
        widget.append(&actions);
        let status = gtk::Label::new(None);
        status.set_xalign(0.);
        status.set_wrap(true);
        widget.append(&status);
        let confirmation = gtk::Box::new(gtk::Orientation::Vertical, 8);
        confirmation.set_visible(false);
        let prompt = gtk::Label::new(None);
        prompt.set_xalign(0.);
        prompt.set_wrap(true);
        confirmation.append(&prompt);
        let choices = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let cancel = gtk::Button::with_label("Отмена");
        let delete = gtk::Button::with_label("Удалить профиль");
        delete.add_css_class("destructive-action");
        choices.append(&cancel);
        choices.append(&delete);
        confirmation.append(&choices);
        widget.append(&confirmation);
        let list = gtk::ListBox::new();
        list.set_selection_mode(gtk::SelectionMode::None);
        widget.append(
            &gtk::ScrolledWindow::builder()
                .vexpand(true)
                .hscrollbar_policy(gtk::PolicyType::Never)
                .child(&list)
                .build(),
        );

        let pane = Rc::new(Self {
            widget,
            back,
            list,
            status,
            refresh,
            busy: Cell::new(false),
            confirmation,
            prompt,
            offer: RefCell::new(None),
        });
        let weak = Rc::downgrade(&pane);
        pane.refresh.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.load(None);
            }
        });
        let weak = Rc::downgrade(&pane);
        editor.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.launch_editor(None);
            }
        });
        let weak = Rc::downgrade(&pane);
        cancel.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.offer.borrow_mut().take();
                pane.update_controls();
            }
        });
        let weak = Rc::downgrade(&pane);
        delete.connect_clicked(move |_| {
            if let Some(pane) = weak.upgrade() {
                pane.delete();
            }
        });
        pane
    }

    pub fn load(self: &Rc<Self>, notice: Option<&'static str>) {
        if self.busy.get() {
            return;
        }
        self.offer.borrow_mut().take();
        while let Some(row) = self.list.first_child() {
            self.list.remove(&row);
        }
        self.status.set_text(notice.unwrap_or("Загрузка профилей…"));
        self.set_busy(true);
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = call("GetWifiProfiles", None, 6500).await.and_then(|reply| {
                reply
                    .get::<(String,)>()
                    .and_then(|(text,)| toml::from_str::<WifiProfiles>(&text).ok())
                    .ok_or_else(|| {
                        glib::Error::new(gio::IOErrorEnum::InvalidData, "invalid profile list")
                    })
            });
            if let Some(pane) = weak.upgrade() {
                pane.set_busy(false);
                match result {
                    Ok(profiles) => pane.render(profiles, notice),
                    Err(_) => pane.status.set_text(if notice.is_some() {
                        "Профиль удалён. Не удалось обновить список"
                    } else {
                        "Не удалось загрузить профили NetworkManager"
                    }),
                }
            }
        });
    }

    fn render(self: &Rc<Self>, profiles: WifiProfiles, notice: Option<&str>) {
        self.status.set_text(if profiles.truncated {
            "Показаны первые 128 профилей Wi-Fi. Полный список доступен во внешнем редакторе"
        } else if let Some(notice) = notice {
            notice
        } else if profiles.profiles.is_empty() {
            "Сохранённых профилей Wi-Fi нет"
        } else {
            "Сохранённые профили Wi-Fi"
        });
        for profile in profiles.profiles {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 10);
            row.set_margin_top(6);
            row.set_margin_bottom(6);
            row.set_tooltip_text(Some(&profile.uuid));
            let summary = gtk::Box::new(gtk::Orientation::Vertical, 3);
            for text in [&profile.name, &profile.ssid] {
                if !text.is_empty() {
                    let label = gtk::Label::new(Some(text));
                    label.set_xalign(0.);
                    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
                    summary.append(&label);
                }
            }
            summary.set_hexpand(true);
            row.append(&summary);
            let edit = gtk::Button::with_label("Изменить…");
            edit.set_valign(gtk::Align::Center);
            row.append(&edit);
            let weak = Rc::downgrade(self);
            let uuid = profile.uuid.clone();
            edit.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.launch_editor(Some(&uuid));
                }
            });
            let forget = gtk::Button::with_label("Удалить…");
            forget.update_property(&[gtk::accessible::Property::Label(&format!(
                "Удалить профиль {}",
                profile.name
            ))]);
            forget.set_valign(gtk::Align::Center);
            row.append(&forget);
            self.list.append(&row);
            let weak = Rc::downgrade(self);
            let owner = profiles.owner.clone();
            forget.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.confirm(owner.clone(), profile.clone());
                }
            });
        }
    }

    fn set_busy(&self, busy: bool) {
        self.busy.set(busy);
        self.update_controls();
    }

    fn update_controls(&self) {
        let confirming = self.offer.borrow().is_some();
        self.confirmation.set_visible(confirming);
        self.refresh.set_sensitive(!self.busy.get());
        self.list.set_sensitive(!self.busy.get() && !confirming);
    }

    fn confirm(&self, owner: String, profile: WifiProfile) {
        let identification = if profile.ssid.is_empty() || profile.ssid == profile.name {
            format!("Удалить профиль «{}»?", profile.name)
        } else {
            format!(
                "Удалить профиль «{}» для сети «{}»?",
                profile.name, profile.ssid
            )
        };
        self.prompt.set_text(&format!(
            "{identification}\nЕсли профиль используется, подключение прервётся.\nUUID: {}",
            profile.uuid
        ));
        *self.offer.borrow_mut() = Some((owner, profile));
        self.update_controls();
    }

    fn delete(self: &Rc<Self>) {
        let Some((owner, profile)) = self.offer.borrow_mut().take() else {
            return;
        };
        let parameters = (
            owner,
            profile.path,
            profile.uuid,
            profile.name,
            profile.ssid,
        )
            .to_variant();
        self.set_busy(true);
        self.status.set_text("Удаление профиля…");
        // Give a system authorization dialog access to the keyboard and the foreground.
        let surface = self
            .widget
            .root()
            .and_downcast::<gtk::Window>()
            .map(|window| {
                let previous = (window.layer(), window.keyboard_mode());
                window.set_keyboard_mode(KeyboardMode::None);
                window.set_layer(Layer::Bottom);
                (window.downgrade(), previous)
            });
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let result = call("ForgetWifi", Some(parameters), 55000).await;
            if let Some((surface, (layer, keyboard))) = surface
                && let Some(window) = surface.upgrade().filter(|window| window.is_visible())
            {
                window.set_layer(layer);
                window.set_keyboard_mode(keyboard);
            }
            if let Some(pane) = weak.upgrade() {
                pane.set_busy(false);
                match result {
                    Ok(_) => pane.load(Some("Профиль удалён")),
                    Err(error) => {
                        let name = gio::DBusError::remote_error(&error);
                        let detail = match name.as_deref().and_then(|name| name.rsplit('.').next())
                        {
                            Some("PermissionDenied") => "NetworkManager запретил удаление",
                            Some("Conflict" | "NoSavedProfile" | "UnknownObject") => {
                                "Профиль изменился. Обновите список"
                            }
                            _ => "Не удалось подтвердить удаление. Обновите список",
                        };
                        pane.status.set_text(detail);
                    }
                }
            }
        });
    }

    fn launch_editor(&self, uuid: Option<&str>) {
        let Some(editor) = gio_unix::DesktopAppInfo::new("nm-connection-editor.desktop") else {
            self.status
                .set_text("Установите nm-connection-editor для расширенной настройки");
            return;
        };
        let result = if let Some(uuid) = uuid {
            let Some(executable) = editor.executable() else {
                self.status.set_text("Не найден исполняемый файл редактора");
                return;
            };
            gio::Subprocess::newv(
                &[
                    executable.as_os_str(),
                    OsStr::new("--edit"),
                    OsStr::new(uuid),
                ],
                gio::SubprocessFlags::NONE,
            )
            .map(|_| ())
        } else {
            let context = self.widget.display().app_launch_context();
            editor.launch(&[], Some(&context))
        };
        if result.is_err() {
            self.status
                .set_text("Не удалось открыть редактор подключений");
        } else if let Some(window) = self.widget.root().and_downcast::<gtk::Window>() {
            window.close();
        }
    }
}

async fn call(
    method: &str,
    parameters: Option<glib::Variant>,
    timeout: i32,
) -> Result<glib::Variant, glib::Error> {
    gio::bus_get_future(gio::BusType::Session)
        .await?
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
