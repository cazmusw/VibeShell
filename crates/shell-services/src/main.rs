use anyhow::Result;
use shell_services::{NewNotification, NotificationStore};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zbus::{message::Header, object_server::SignalEmitter, zvariant::OwnedValue};
#[derive(Clone, Default)]
struct Notifications {
    store: Arc<Mutex<NotificationStore>>,
}
#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn get_capabilities(&self) -> Vec<&str> {
        vec!["body", "persistence"]
    }
    fn get_server_information(&self) -> (&str, &str, &str, &str) {
        ("Mosaic", "Mosaic Shell", env!("CARGO_PKG_VERSION"), "1.2")
    }
    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        app_name: String,
        replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<u32> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let transient = hints
            .get("transient")
            .and_then(|v| bool::try_from(v).ok())
            .unwrap_or(false);
        let urgency = hints
            .get("urgency")
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(1);
        let (id, evicted) = {
            let mut store = self.store.lock().unwrap();
            let id = store.notify(
                NewNotification {
                    sender,
                    app: app_name,
                    summary,
                    body,
                    replaces_id,
                    transient,
                    urgency,
                    timeout_ms: expire_timeout,
                },
                now(),
            );
            (id, store.take_evicted())
        };
        for evicted_id in evicted {
            Self::notification_closed(&emitter, evicted_id, 4)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        }
        Ok(id)
    }
    async fn close_notification(
        &self,
        id: u32,
        #[zbus(header)] header: Header<'_>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let sender = header
            .sender()
            .ok_or_else(|| zbus::fdo::Error::Failed("Missing sender".into()))?
            .to_string();
        let closed = self.store.lock().unwrap().close(id, &sender).is_some();
        if closed {
            Self::notification_closed(&emitter, id, 3)
                .await
                .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?
        }
        Ok(())
    }
    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;
}
#[derive(Clone)]
struct Services {
    store: Arc<Mutex<NotificationStore>>,
}
#[zbus::interface(name = "org.mosaic.Services1")]
impl Services {
    fn get_notifications(&self) -> String {
        serde_json::to_string(&self.store.lock().unwrap().items()).unwrap_or_else(|_| "[]".into())
    }
    fn set_do_not_disturb(&self, enabled: bool) {
        self.store.lock().unwrap().dnd = enabled;
    }
    fn get_do_not_disturb(&self) -> bool {
        self.store.lock().unwrap().dnd
    }
    fn clear_history(&self) {
        self.store.lock().unwrap().clear();
    }
}
fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn main() {
    if let Err(e) = run() {
        eprintln!("mosaic-services: {e:#}");
        std::process::exit(1)
    }
}
fn run() -> Result<()> {
    let notifications = Notifications::default();
    let services = Services {
        store: notifications.store.clone(),
    };
    let connection = zbus::blocking::connection::Builder::session()?
        .name("org.mosaic.Services1")?
        .name("org.freedesktop.Notifications")?
        .serve_at("/org/freedesktop/Notifications", notifications.clone())?
        .serve_at("/org/mosaic/Services1", services)?
        .build()?;
    loop {
        std::thread::sleep(Duration::from_millis(250));
        let expired = notifications.store.lock().unwrap().expire(now());
        for id in expired {
            connection.emit_signal(
                None::<&str>,
                "/org/freedesktop/Notifications",
                "org.freedesktop.Notifications",
                "NotificationClosed",
                &(id, 1u32),
            )?;
        }
    }
}
