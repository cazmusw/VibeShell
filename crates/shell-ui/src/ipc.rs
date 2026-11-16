use std::sync::mpsc::{self, Receiver, SyncSender};
#[derive(Clone, Copy)]
pub enum UiCommand {
    Launcher,
    Applications,
    Settings,
    Notifications,
    Reload,
    ShowDock,
}
struct Interface {
    tx: SyncSender<UiCommand>,
}
impl Interface {
    fn send(&self, c: UiCommand) -> zbus::fdo::Result<()> {
        self.tx
            .try_send(c)
            .map_err(|_| zbus::fdo::Error::Failed("UI занят или недоступен".into()))
    }
}
#[zbus::interface(name = "org.mosaic.Shell1")]
impl Interface {
    fn show_launcher(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::Launcher)
    }
    fn show_applications(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::Applications)
    }
    fn open_settings(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::Settings)
    }
    fn toggle_notifications(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::Notifications)
    }
    fn show_dock(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::ShowDock)
    }
    fn reload(&self) -> zbus::fdo::Result<()> {
        self.send(UiCommand::Reload)
    }
}
pub fn start() -> Receiver<UiCommand> {
    let (tx, rx) = mpsc::sync_channel(32);
    std::thread::spawn(move || {
        let result = (|| -> anyhow::Result<_> {
            Ok(zbus::blocking::connection::Builder::session()?
                .name("org.mosaic.Shell1")?
                .serve_at("/org/mosaic/Shell1", Interface { tx })?
                .build()?)
        })();
        match result {
            Ok(connection) => {
                let _keep = connection;
                loop {
                    std::thread::park()
                }
            }
            Err(e) => eprintln!("Mosaic IPC недоступен: {e}"),
        }
    });
    rx
}
