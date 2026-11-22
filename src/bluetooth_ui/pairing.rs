use super::{BluetoothPane, DeviceTarget, action_error, call_on};
use gtk::{gio, glib, prelude::*};
use std::{
    rc::Rc,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

static NEXT_TOKEN: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, PartialEq, Eq)]
struct Prompt {
    id: u64,
    kind: String,
    code: String,
}

pub(super) struct Pairing {
    token: u64,
    identity: String,
    connection: Option<gio::DBusConnection>,
    prompt: Option<Prompt>,
    responding_to: Option<u64>,
    cancelling: bool,
    feedback: Option<&'static str>,
}

impl Pairing {
    pub(super) fn cancel_on_close(self) {
        if let Some(connection) = self.connection {
            connection.close(gio::Cancellable::NONE, |_| {});
        }
    }

    fn visible_prompt(&self) -> Option<&Prompt> {
        self.prompt
            .as_ref()
            .filter(|prompt| self.responding_to != Some(prompt.id) && !self.cancelling)
    }
}

impl BluetoothPane {
    pub(super) fn start_pairing(self: &Rc<Self>, target: &DeviceTarget) {
        let token = NEXT_TOKEN.fetch_add(1, Ordering::Relaxed);
        self.pairing.replace(Some(Pairing {
            token,
            identity: target.identity.clone(),
            connection: None,
            prompt: None,
            responding_to: None,
            cancelling: false,
            feedback: None,
        }));
        self.begin_action();
        let parameters = (
            target.owner.as_str(),
            target.adapter.as_str(),
            target.path.as_str(),
            target.address.as_str(),
            token,
        )
            .to_variant();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let mut message = match pairing_connection().await {
                Ok(connection) => {
                    let Some(pane) = weak.upgrade() else {
                        let _ = connection.close_future().await;
                        return;
                    };
                    let start = pane
                        .pairing
                        .borrow_mut()
                        .as_mut()
                        .filter(|pairing| pairing.token == token && !pairing.cancelling)
                        .map(|pairing| pairing.connection = Some(connection.clone()))
                        .is_some();
                    if start {
                        pane.poll_pairing(token, connection.clone());
                    }
                    drop(pane);
                    let message = if start {
                        let reply =
                            call_on(&connection, "PairBluetooth", Some(parameters), 100000).await;
                        reply.err().as_ref().map(action_error)
                    } else {
                        Some("Сопряжение отменено")
                    };
                    if !connection.is_closed() {
                        let _ = connection.close_future().await;
                    }
                    message
                }
                Err(error) => Some(action_error(&error)),
            };
            if let Some(pane) = weak.upgrade() {
                if pane
                    .pairing
                    .take()
                    .is_some_and(|pairing| pairing.cancelling)
                    && message.is_some()
                {
                    message = Some("Сопряжение отменено");
                }
                pane.render_inline();
            }
            Self::complete_action(weak, message).await;
        });
    }

    fn poll_pairing(self: &Rc<Self>, token: u64, connection: gio::DBusConnection) {
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            loop {
                glib::timeout_future(Duration::from_millis(250)).await;
                if !weak.upgrade().is_some_and(|pane| {
                    pane.pairing
                        .borrow()
                        .as_ref()
                        .is_some_and(|pairing| pairing.token == token)
                }) {
                    break;
                }
                let reply = call_on(
                    &connection,
                    "GetBluetoothPairing",
                    Some((token,).to_variant()),
                    2000,
                )
                .await;
                let Some(pane) = weak.upgrade() else {
                    break;
                };
                let Ok(reply) = reply else {
                    continue;
                };
                let Some((id, kind, code)) = reply.get::<(u64, String, String)>() else {
                    continue;
                };
                let prompt = (id != 0).then_some(Prompt { id, kind, code });
                let changed = {
                    let mut state = pane.pairing.borrow_mut();
                    let Some(pairing) = state.as_mut().filter(|pairing| pairing.token == token)
                    else {
                        break;
                    };
                    if pairing.prompt == prompt {
                        false
                    } else {
                        pairing.prompt = prompt;
                        pairing.feedback = None;
                        !pairing.cancelling
                    }
                };
                if changed {
                    pane.render_inline();
                }
            }
        });
    }

    pub(super) fn render_pairing(self: &Rc<Self>) {
        let state = self.pairing.borrow();
        let Some(pairing) = state.as_ref() else {
            return;
        };
        let token = pairing.token;
        let identity = &pairing.identity;
        let prompt = pairing.visible_prompt();
        let kind = prompt.map(|prompt| prompt.kind.as_str());
        let description = match kind {
            Some("confirm") => format!(
                "Подтвердите, что на устройстве «{identity}» показан код {}.",
                prompt.unwrap().code
            ),
            Some("authorize") => format!("Разрешить сопряжение с устройством «{identity}»?"),
            Some("pin") => format!("Введите PIN-код устройства «{identity}»."),
            Some("passkey") => format!("Введите цифровой код устройства «{identity}»."),
            Some("display") => format!(
                "Введите код {} на устройстве «{identity}» и подтвердите ввод на нём.",
                prompt.unwrap().code
            ),
            _ if pairing.cancelling => format!("Отмена сопряжения с устройством «{identity}»…"),
            _ => format!("Сопряжение с устройством «{identity}»…"),
        };
        let (description, actions) = self.inline_controls(&description);
        self.inline.append(&description);
        if let Some(feedback) = pairing.feedback {
            let (label, _) = self.inline_controls(feedback);
            self.inline.append(&label);
        }
        let mut focus = None;
        if let Some(prompt) = prompt.filter(|prompt| {
            matches!(
                prompt.kind.as_str(),
                "confirm" | "authorize" | "pin" | "passkey"
            )
        }) {
            let id = prompt.id;
            let reject = gtk::Button::with_label("Отклонить");
            let weak = Rc::downgrade(self);
            reject.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.respond_pairing(token, id, false, "");
                }
            });
            actions.append(&reject);
            let passkey = prompt.kind == "passkey";
            let entry = matches!(prompt.kind.as_str(), "pin" | "passkey").then(|| {
                let entry = gtk::Entry::new();
                let hint = if passkey {
                    "Цифровой код"
                } else {
                    "PIN-код"
                };
                entry.set_placeholder_text(Some(hint));
                entry.update_property(&[gtk::accessible::Property::Label(hint)]);
                entry.set_max_length(if passkey { 6 } else { 16 });
                if passkey {
                    entry.set_input_purpose(gtk::InputPurpose::Digits);
                }
                self.inline.append(&entry);
                entry
            });
            let accept = gtk::Button::with_label(if entry.is_some() {
                "Отправить"
            } else {
                "Подтвердить"
            });
            if let Some(entry) = &entry {
                accept.set_sensitive(false);
                let weak_button = accept.downgrade();
                entry.connect_changed(move |entry| {
                    if let Some(button) = weak_button.upgrade() {
                        button.set_sensitive(valid_code(&entry.text(), passkey));
                    }
                });
            }
            focus = entry.clone();
            let weak = Rc::downgrade(self);
            accept.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    let text = entry.as_ref().map(|entry| entry.text()).unwrap_or_default();
                    if entry.is_none() || valid_code(&text, passkey) {
                        pane.respond_pairing(token, id, true, &text);
                    }
                }
            });
            actions.append(&accept);
        } else {
            let cancel = gtk::Button::with_label("Отмена");
            cancel.set_sensitive(!pairing.cancelling);
            let weak = Rc::downgrade(self);
            cancel.connect_clicked(move |_| {
                if let Some(pane) = weak.upgrade() {
                    pane.cancel_pairing(token);
                }
            });
            actions.append(&cancel);
        }
        self.inline.append(&actions);
        self.inline.set_visible(true);
        if let Some(entry) = focus {
            entry.grab_focus();
        }
    }

    fn respond_pairing(self: &Rc<Self>, token: u64, id: u64, accept: bool, text: &str) {
        let connection = {
            let mut state = self.pairing.borrow_mut();
            let Some(pairing) = state.as_mut().filter(|pairing| {
                pairing.token == token
                    && pairing
                        .visible_prompt()
                        .is_some_and(|prompt| prompt.id == id)
            }) else {
                return;
            };
            let Some(connection) = pairing.connection.clone() else {
                return;
            };
            pairing.responding_to = Some(id);
            pairing.feedback = None;
            connection
        };
        self.render_inline();
        let parameters = (token, id, accept, text).to_variant();
        let weak = Rc::downgrade(self);
        glib::MainContext::default().spawn_local(async move {
            let reply = call_on(
                &connection,
                "RespondBluetoothPairing",
                Some(parameters),
                5000,
            )
            .await;
            if let (Some(pane), Err(error)) = (weak.upgrade(), reply) {
                {
                    let mut state = pane.pairing.borrow_mut();
                    let Some(pairing) = state.as_mut().filter(|pairing| {
                        pairing.token == token
                            && !pairing.cancelling
                            && pairing
                                .prompt
                                .as_ref()
                                .is_some_and(|prompt| prompt.id == id)
                            && pairing.responding_to == Some(id)
                    }) else {
                        return;
                    };
                    pairing.responding_to = None;
                    pairing.feedback = Some(action_error(&error));
                }
                pane.render_inline();
            }
        });
    }

    fn cancel_pairing(self: &Rc<Self>, token: u64) {
        let connection = {
            let mut state = self.pairing.borrow_mut();
            let Some(pairing) = state
                .as_mut()
                .filter(|pairing| pairing.token == token && !pairing.cancelling)
            else {
                return;
            };
            pairing.cancelling = true;
            pairing.feedback = None;
            pairing.connection.clone()
        };
        self.render_inline();
        let Some(connection) = connection else {
            return;
        };
        glib::MainContext::default().spawn_local(async move {
            let _ = call_on(
                &connection,
                "CancelBluetoothPairing",
                Some((token,).to_variant()),
                2000,
            )
            .await;
            if !connection.is_closed() {
                let _ = connection.close_future().await;
            }
        });
    }
}

async fn pairing_connection() -> Result<gio::DBusConnection, glib::Error> {
    let address =
        gio::dbus_address_get_for_bus_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let connection = gio::DBusConnection::for_address_future(
        &address,
        gio::DBusConnectionFlags::AUTHENTICATION_CLIENT
            | gio::DBusConnectionFlags::MESSAGE_BUS_CONNECTION,
        None,
    )
    .await?;
    connection.set_exit_on_close(false);
    Ok(connection)
}

fn valid_code(text: &str, passkey: bool) -> bool {
    !text.is_empty()
        && if passkey {
            text.len() <= 6 && text.bytes().all(|byte| byte.is_ascii_digit())
        } else {
            text.len() <= 16 && text.bytes().all(|byte| byte.is_ascii_alphanumeric())
        }
}
