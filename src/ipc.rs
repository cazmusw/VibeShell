use crate::desktop::Window;
use crate::settings::Settings;
use std::{
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.mosaic.Error1")]
pub enum ApiError {
    #[zbus(error)]
    ZBus(zbus::Error),
    InvalidArgument(String),
    Conflict(String),
    PermissionDenied(String),
    NoSavedProfile(String),
    SecretsUnavailable(String),
    Timeout(String),
    Cancelled(String),
    Failed(String),
}
pub type Reply<T> = mpsc::SyncSender<Result<T, ApiError>>;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub struct DockInfo {
    pub visible: bool,
    pub auto_hide: bool,
    pub icon_size: u32,
    pub reduced_motion: bool,
    pub magnification: bool,
    pub animation_active: bool,
    pub frames: u64,
    pub layout_revision: u64,
    pub width: i32,
    pub height: i32,
    pub exclusive_zone: i32,
    pub max_scale: f64,
    pub trigger_mapped: bool,
    pub trigger_width: i32,
    pub trigger_height: i32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub struct OutputInfo {
    pub connector: String,
    pub manufacturer: String,
    pub model: String,
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
    pub scale: f64,
    pub panel_mapped: bool,
    pub panel_width: i32,
    pub panel_height: i32,
    pub dock_selected: bool,
    pub dock_mapped: bool,
}
#[derive(serde::Serialize)]
struct OutputSnapshot {
    outputs: Vec<OutputInfo>,
}
#[derive(Default, Debug, serde::Serialize, serde::Deserialize, zbus::zvariant::Type)]
pub struct DialogInfo {
    pub kind: String,
    pub connector: String,
    pub mapped: bool,
}
#[derive(Debug)]
pub enum Command {
    TrashStatus(Reply<String>),
    TrayState(Reply<String>),
    Outputs(Reply<Vec<OutputInfo>>),
    Launcher(String),
    ControlCenter(String),
    Wifi(String),
    Bluetooth(String),
    Brightness(String),
    Temperatures(String),
    Power(String),
    Settings,
    DialogInfo(Reply<DialogInfo>),
    CloseDialog,
    Dock(String, Reply<()>),
    Quit,
    Launch(String),
    Pin(String, bool),
    Window(u64, u32, String),
    ReadConfig(Reply<Settings>),
    ValidatePatch(String, Reply<String>),
    ApplyPatch(u64, String, Instant, Reply<Settings>),
    Reorder(u64, String, String, Instant, Reply<Settings>),
    Reload(Reply<Settings>),
    DockInfo(String, Reply<DockInfo>),
    DockCards(String, Reply<Vec<u32>>),
}
pub type Shared = Arc<Mutex<Vec<Window>>>;
pub struct ShellApi {
    pub tx: async_channel::Sender<Command>,
    pub windows: Shared,
}
impl ShellApi {
    fn request<T>(&self, build: impl FnOnce(Reply<T>, Instant) -> Command) -> Result<T, ApiError> {
        let (tx, rx) = mpsc::sync_channel(1);
        let deadline = Instant::now() + Duration::from_secs(2);
        self.tx
            .try_send(build(tx, deadline))
            .map_err(|_| ApiError::Failed("UI queue unavailable or full".into()))?;
        rx.recv_timeout(Duration::from_secs(2))
            .map_err(|_| ApiError::Timeout("UI deadline exceeded".into()))?
    }
    fn send(&self, command: Command) -> zbus::fdo::Result<()> {
        self.tx
            .try_send(command)
            .map_err(|_| zbus::fdo::Error::Failed("UI queue unavailable or full".into()))
    }
    fn validate_output_hint(output_hint: &str) -> zbus::fdo::Result<()> {
        if output_hint.len() > 256 || output_hint.chars().any(char::is_control) {
            return Err(zbus::fdo::Error::InvalidArgs(
                "invalid output connector".into(),
            ));
        }
        Ok(())
    }
    fn launcher_at(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Launcher(output_hint.into()))
    }
}
#[zbus::interface(name = "org.mosaic.Shell1")]
impl ShellApi {
    fn show_launcher(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        self.launcher_at(output_hint)
    }
    fn show_applications(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        self.launcher_at(output_hint)
    }
    fn toggle_control_center(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::ControlCenter(output_hint.into()))
    }
    fn show_wifi(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Wifi(output_hint.into()))
    }
    fn show_bluetooth(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Bluetooth(output_hint.into()))
    }
    fn show_brightness(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Brightness(output_hint.into()))
    }
    fn show_temperatures(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Temperatures(output_hint.into()))
    }
    fn show_power(&self, output_hint: &str) -> zbus::fdo::Result<()> {
        Self::validate_output_hint(output_hint)?;
        self.send(Command::Power(output_hint.into()))
    }
    fn show_dock(&self, output_hint: &str) -> Result<(), ApiError> {
        Self::validate_output_hint(output_hint)
            .map_err(|_| ApiError::InvalidArgument("invalid output connector".into()))?;
        self.request(|reply, _| Command::Dock(output_hint.into(), reply))
    }
    fn open_settings(&self, _section: &str) -> zbus::fdo::Result<()> {
        self.send(Command::Settings)
    }
    fn get_dialog_state(&self) -> Result<DialogInfo, ApiError> {
        self.request(|reply, _| Command::DialogInfo(reply))
    }
    fn close_dialog(&self) -> zbus::fdo::Result<()> {
        self.send(Command::CloseDialog)
    }
    fn quit(&self) -> zbus::fdo::Result<()> {
        self.send(Command::Quit)
    }
    fn launch(&self, desktop_id: &str) -> zbus::fdo::Result<()> {
        self.send(Command::Launch(desktop_id.into()))
    }
    fn pin(&self, desktop_id: &str, pinned: bool) -> zbus::fdo::Result<()> {
        self.send(Command::Pin(desktop_id.into(), pinned))
    }
    fn request_window(&self, epoch: u64, id: u32, action: &str) -> zbus::fdo::Result<()> {
        if ![
            "activate",
            "minimize",
            "close",
            "fullscreen",
            "unfullscreen",
        ]
        .contains(&action)
        {
            return Err(zbus::fdo::Error::InvalidArgs("unknown action".into()));
        }
        if !crate::desktop::valid_window(
            &self
                .windows
                .lock()
                .map_err(|_| zbus::fdo::Error::Failed("state unavailable".into()))?,
            epoch,
            id,
        ) {
            return Err(zbus::fdo::Error::InvalidArgs("stale window".into()));
        }
        self.send(Command::Window(epoch, id, action.into()))
    }
    fn get_windows(&self) -> Vec<(u64, u32, String, bool, bool)> {
        self.windows
            .lock()
            .map(|w| {
                w.iter()
                    .map(|w| (w.epoch, w.id, w.app_id.clone(), w.active, w.minimized))
                    .collect()
            })
            .unwrap_or_default()
    }
    fn get_config(&self) -> Result<(u64, String), ApiError> {
        let settings = self.request(|reply, _| Command::ReadConfig(reply))?;
        Ok((
            settings.revision,
            toml::to_string(&settings)
                .map_err(|_| ApiError::Failed("config serialization".into()))?,
        ))
    }
    fn validate_patch(&self, patch: &str) -> Result<String, ApiError> {
        if patch.len() > 8192 {
            return Err(ApiError::InvalidArgument("patch exceeds 8 KiB".into()));
        }
        self.request(|reply, _| Command::ValidatePatch(patch.into(), reply))
    }
    fn apply_patch(&self, expected_revision: u64, patch: &str) -> Result<(u64, String), ApiError> {
        if patch.len() > 8192 {
            return Err(ApiError::InvalidArgument("patch exceeds 8 KiB".into()));
        }
        let settings = self.request(|reply, deadline| {
            Command::ApplyPatch(expected_revision, patch.into(), deadline, reply)
        })?;
        Ok((
            settings.revision,
            toml::to_string(&settings)
                .map_err(|_| ApiError::Failed("config serialization".into()))?,
        ))
    }
    fn reorder_pin(
        &self,
        expected_revision: u64,
        source: &str,
        before: &str,
    ) -> Result<(u64, String), ApiError> {
        if source.len() > 256 || before.len() > 256 {
            return Err(ApiError::InvalidArgument("desktop ID too long".into()));
        }
        let settings = self.request(|reply, deadline| {
            Command::Reorder(
                expected_revision,
                source.into(),
                before.into(),
                deadline,
                reply,
            )
        })?;
        Ok((
            settings.revision,
            toml::to_string(&settings)
                .map_err(|_| ApiError::Failed("config serialization".into()))?,
        ))
    }
    fn reload(&self) -> Result<(), ApiError> {
        self.request(|reply, _| Command::Reload(reply)).map(|_| ())
    }
    fn get_tray_state(&self) -> Result<String, ApiError> {
        self.request(|reply, _| Command::TrayState(reply))
    }
    fn get_trash_status(&self) -> Result<String, ApiError> {
        self.request(|reply, _| Command::TrashStatus(reply))
    }
    fn get_outputs(&self) -> Result<Vec<OutputInfo>, ApiError> {
        self.request(|reply, _| Command::Outputs(reply))
    }
    fn get_window_outputs(&self) -> zbus::fdo::Result<Vec<(u64, u32, Vec<String>)>> {
        Ok(self
            .windows
            .lock()
            .map_err(|_| zbus::fdo::Error::Failed("state unavailable".into()))?
            .iter()
            .map(|w| (w.epoch, w.id, w.outputs.clone()))
            .collect())
    }
    fn get_dock_cards(&self) -> Result<Vec<u32>, ApiError> {
        self.get_dock_cards_for_output("")
    }
    fn get_dock_cards_for_output(&self, output_hint: &str) -> Result<Vec<u32>, ApiError> {
        Self::validate_output_hint(output_hint)
            .map_err(|_| ApiError::InvalidArgument("invalid output connector".into()))?;
        self.request(|reply, _| Command::DockCards(output_hint.into(), reply))
    }
    fn get_dock_state(&self) -> Result<DockInfo, ApiError> {
        self.get_dock_state_for_output("")
    }
    fn get_dock_state_for_output(&self, output_hint: &str) -> Result<DockInfo, ApiError> {
        if Self::validate_output_hint(output_hint).is_err() {
            return Err(ApiError::InvalidArgument("invalid output connector".into()));
        }
        self.request(|reply, _| Command::DockInfo(output_hint.into(), reply))
    }
    fn get_health(&self) -> (&str, bool) {
        (env!("CARGO_PKG_VERSION"), true)
    }
}
pub fn ui_bus(
    tx: async_channel::Sender<Command>,
    windows: Shared,
) -> zbus::Result<zbus::blocking::Connection> {
    let bus = zbus::blocking::connection::Builder::session()?
        .method_timeout(std::time::Duration::from_secs(3))
        .serve_at("/org/mosaic/Shell1", ShellApi { tx, windows })?
        .build()?;
    bus.request_name_with_flags(
        "org.mosaic.Shell1",
        zbus::fdo::RequestNameFlags::DoNotQueue.into(),
    )?;
    Ok(bus)
}
pub struct Services {
    pub epoch: u64,
    pub stop: std::sync::mpsc::SyncSender<()>,
    pub audio: crate::audio::AudioService,
    pub network: crate::network::NetworkService,
    pub bluetooth: crate::bluetooth::Service,
    pub brightness: crate::brightness::Service,
    pub power: crate::power::Service,
}
#[zbus::interface(name = "org.mosaic.Services1")]
impl Services {
    fn get_snapshot(&self) -> (u64, u64, Vec<String>) {
        (
            self.epoch,
            1,
            vec![
                "health".into(),
                "tray-sni".into(),
                "audio".into(),
                "wifi".into(),
                "bluetooth".into(),
                "brightness".into(),
                "power".into(),
            ],
        )
    }
    fn get_health(&self) -> (&str, bool) {
        (env!("CARGO_PKG_VERSION"), true)
    }
    fn get_audio(&self) -> String {
        self.audio.snapshot()
    }
    async fn get_wifi(&self) -> String {
        toml::to_string(&self.network.snapshot().await).unwrap_or_default()
    }
    async fn get_bluetooth(&self) -> String {
        toml::to_string(&self.bluetooth.snapshot().await).unwrap_or_default()
    }
    async fn get_brightness(&self) -> String {
        toml::to_string(&self.brightness.snapshot().await).unwrap_or_default()
    }
    async fn get_power(&self) -> String {
        toml::to_string(&self.power.snapshot().await).unwrap_or_default()
    }
    async fn set_power_profile(
        &self,
        owner: &str,
        expected: &str,
        target: &str,
    ) -> Result<(), ApiError> {
        self.power.set(owner, expected, target).await
    }
    async fn set_brightness(
        &self,
        owner: &str,
        session: &str,
        device: &str,
        identity: &str,
        max: u32,
        percent: u32,
    ) -> Result<(), ApiError> {
        self.brightness
            .set(owner, session, device, identity, max, percent)
            .await
    }
    async fn set_bluetooth_power(
        &self,
        owner: &str,
        adapter: &str,
        powered: bool,
    ) -> Result<(), ApiError> {
        self.bluetooth.set_power(owner, adapter, powered).await
    }
    async fn scan_bluetooth(&self, owner: &str, adapter: &str) -> Result<(), ApiError> {
        self.bluetooth.scan(owner, adapter).await
    }
    async fn stop_bluetooth_scan(&self, owner: &str, adapter: &str) {
        self.bluetooth.stop_scan(owner, adapter).await;
    }
    async fn change_bluetooth_device(
        &self,
        owner: &str,
        adapter: &str,
        device: &str,
        address: &str,
        action: &str,
    ) -> Result<(), ApiError> {
        let target = crate::bluetooth::DeviceTarget {
            owner,
            adapter,
            path: device,
            address,
        };
        self.bluetooth.change_device(target, action).await
    }
    #[allow(clippy::too_many_arguments)]
    async fn pair_bluetooth(
        &self,
        owner: &str,
        adapter: &str,
        device: &str,
        address: &str,
        token: u64,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), ApiError> {
        let target = crate::bluetooth::DeviceTarget {
            owner,
            adapter,
            path: device,
            address,
        };
        self.bluetooth
            .pair(connection, pairing_sender(&header)?, token, target)
            .await
    }
    #[zbus(out_args("prompt", "kind", "code"))]
    fn get_bluetooth_pairing(
        &self,
        token: u64,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<(u64, String, String), ApiError> {
        Ok(self
            .bluetooth
            .pairing_prompt(pairing_sender(&header)?, token))
    }
    fn respond_bluetooth_pairing(
        &self,
        token: u64,
        prompt: u64,
        accept: bool,
        text: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<(), ApiError> {
        self.bluetooth
            .respond_pairing(pairing_sender(&header)?, token, prompt, accept, text)
    }
    fn cancel_bluetooth_pairing(
        &self,
        token: u64,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> Result<(), ApiError> {
        self.bluetooth
            .cancel_pairing(pairing_sender(&header)?, token);
        Ok(())
    }
    async fn get_wifi_profiles(&self) -> Result<String, ApiError> {
        toml::to_string(&self.network.wifi_profiles().await?)
            .map_err(|_| ApiError::Failed("cannot serialize Wi-Fi profiles".into()))
    }
    async fn forget_wifi(
        &self,
        owner: &str,
        path: &str,
        uuid: &str,
        name: &str,
        ssid: &str,
    ) -> Result<(), ApiError> {
        let profile = crate::network::WifiProfile {
            path: path.into(),
            uuid: uuid.into(),
            name: name.into(),
            ssid: ssid.into(),
        };
        self.network.forget_wifi(owner, profile).await
    }
    async fn scan_wifi(&self, adapter: &str) -> Result<(), ApiError> {
        self.network.scan(adapter).await.map(|_| ())
    }
    async fn scan_wifi_with_baseline(&self, adapter: &str) -> Result<i64, ApiError> {
        self.network.scan(adapter).await
    }
    async fn connect_wifi(
        &self,
        adapter: &str,
        ssid: Vec<u8>,
        secured: bool,
    ) -> Result<String, ApiError> {
        self.network.connect_saved(adapter, &ssid, secured).await
    }
    async fn disconnect_wifi(&self, adapter: &str, expected: &str) -> Result<(), ApiError> {
        self.network.disconnect(adapter, expected).await
    }
    async fn join_wifi(
        &self,
        adapter: &str,
        ssid: Vec<u8>,
        mode: &str,
    ) -> Result<String, ApiError> {
        self.network.join_personal(adapter, &ssid, mode).await
    }
    fn set_audio(
        &self,
        revision: u64,
        action: &str,
        device: &str,
        value: u32,
    ) -> Result<(), crate::audio::AudioError> {
        self.audio.apply(revision, action, device, value)
    }
    #[zbus(signal)]
    async fn audio_changed(emitter: &zbus::object_server::SignalEmitter<'_>) -> zbus::Result<()>;
    fn quit(&self) {
        let _ = self.stop.try_send(());
    }
}

fn pairing_sender<'a>(header: &'a zbus::message::Header<'_>) -> Result<&'a str, ApiError> {
    header
        .sender()
        .map(|sender| sender.as_str())
        .ok_or_else(|| {
            ApiError::PermissionDenied("pairing requests require a session bus sender".into())
        })
}

pub fn cli(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    let connection = zbus::blocking::connection::Builder::session()?
        .method_timeout(Duration::from_secs(5))
        .build()?;
    match args {
        [domain, status]
            if matches!(
                domain.as_str(),
                "wifi" | "bluetooth" | "brightness" | "power"
            ) && status == "status" =>
        {
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                crate::audio::BUS,
                crate::audio::PATH,
                crate::audio::BUS,
            )?;
            let method = match domain.as_str() {
                "wifi" => "GetWifi",
                "bluetooth" => "GetBluetooth",
                "power" => "GetPower",
                _ => "GetBrightness",
            };
            let text: String = proxy.call(method, &())?;
            print!("{text}");
            return Ok(());
        }
        [domain, action, owner, expected, target] if domain == "power" && action == "set" => {
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                crate::audio::BUS,
                crate::audio::PATH,
                crate::audio::BUS,
            )?;
            proxy.call::<_, _, ()>("SetPowerProfile", &(owner, expected, target))?;
            return Ok(());
        }
        [wifi, scan, adapter] if wifi == "wifi" && scan == "scan" => {
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                crate::audio::BUS,
                crate::audio::PATH,
                crate::audio::BUS,
            )?;
            proxy.call::<_, _, ()>("ScanWifi", &(adapter,))?;
            return Ok(());
        }
        [wifi, connect, adapter, protection, ssid] if wifi == "wifi" && connect == "connect" => {
            let secured = match protection.as_str() {
                "open" => false,
                "secured" => true,
                _ => return Err("use open or secured".into()),
            };
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                crate::audio::BUS,
                crate::audio::PATH,
                crate::audio::BUS,
            )?;
            let active: String = proxy.call("ConnectWifi", &(adapter, ssid.as_bytes(), secured))?;
            println!("{active}");
            return Ok(());
        }
        [wifi, disconnect, adapter, active] if wifi == "wifi" && disconnect == "disconnect" => {
            let proxy = zbus::blocking::Proxy::new(
                &connection,
                crate::audio::BUS,
                crate::audio::PATH,
                crate::audio::BUS,
            )?;
            proxy.call::<_, _, ()>("DisconnectWifi", &(adapter, active))?;
            return Ok(());
        }
        _ => (),
    }
    if args.first().is_some_and(|command| command == "audio") {
        let proxy = zbus::blocking::Proxy::new(
            &connection,
            crate::audio::BUS,
            crate::audio::PATH,
            crate::audio::BUS,
        )?;
        match args {
            [_, status] if status == "status" => {
                let output: String = proxy.call("GetAudio", &())?;
                print!("{output}");
            }
            [_, set, revision, action, device, value] if set == "set" => {
                proxy.call::<_, _, ()>(
                    "SetAudio",
                    &(
                        revision.parse::<u64>()?,
                        action,
                        device,
                        value.parse::<u32>()?,
                    ),
                )?;
            }
            _ => return Err("invalid audio command".into()),
        }
        return Ok(());
    }
    let proxy = zbus::blocking::Proxy::new(
        &connection,
        "org.mosaic.Shell1",
        "/org/mosaic/Shell1",
        "org.mosaic.Shell1",
    )?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["tray", "state"] => {
            let text: String = proxy.call("GetTrayState", &())?;
            print!("{text}");
        }
        ["tray", "menu", key] => {
            #[derive(serde::Serialize)]
            struct Reply {
                available: bool,
                token: u64,
                revision: u32,
                rows: Vec<crate::menu_model::MenuRow>,
            }
            let tray = zbus::blocking::Proxy::new(
                &connection,
                crate::tray_backend::API,
                crate::tray_backend::PATH,
                crate::tray_backend::API,
            )?;
            let (available, token, revision, rows): (
                bool,
                u64,
                u32,
                Vec<crate::menu_model::RowWire>,
            ) = tray.call("GetMenu", &(key, 0i32, true))?;
            print!(
                "{}",
                toml::to_string(&Reply {
                    available,
                    token,
                    revision,
                    rows: rows
                        .into_iter()
                        .map(crate::menu_model::MenuRow::from_wire)
                        .collect()
                })?
            );
        }
        ["tray", "menu-event", key, token, id] => {
            let tray = zbus::blocking::Proxy::new(
                &connection,
                crate::tray_backend::API,
                crate::tray_backend::PATH,
                crate::tray_backend::API,
            )?;
            tray.call::<_, _, ()>(
                "MenuEvent",
                &(
                    key,
                    token.parse::<u64>()?,
                    id.parse::<i32>()?,
                    "clicked",
                    (gtk::glib::monotonic_time() / 1000) as u32,
                ),
            )?;
        }
        ["tray", "snapshot"] => {
            #[derive(serde::Serialize)]
            struct Snapshot {
                revision: u64,
                items: Vec<crate::tray_model::TrayItem>,
            }
            let tray = zbus::blocking::Proxy::new(
                &connection,
                crate::tray_backend::API,
                crate::tray_backend::PATH,
                crate::tray_backend::API,
            )?;
            let (revision, items): (u64, Vec<crate::tray_model::Wire>) =
                tray.call("GetSnapshot", &())?;
            print!(
                "{}",
                toml::to_string(&Snapshot {
                    revision,
                    items: items
                        .into_iter()
                        .map(crate::tray_model::TrayItem::from_wire)
                        .collect()
                })?
            );
        }
        ["outputs"] => {
            let outputs: Vec<OutputInfo> = proxy.call("GetOutputs", &())?;
            print!("{}", toml::to_string(&OutputSnapshot { outputs })?);
        }
        ["window-outputs"] => {
            let windows: Vec<(u64, u32, Vec<String>)> = proxy.call("GetWindowOutputs", &())?;
            for (epoch, id, names) in windows {
                print!("{epoch}\t{id}");
                for name in names {
                    print!("\t{name}");
                }
                println!();
            }
        }
        ["launcher", hint @ ..] => {
            proxy.call::<_, _, ()>("ShowLauncher", &(hint.first().copied().unwrap_or(""),))?
        }
        ["applications", hint @ ..] => {
            proxy.call::<_, _, ()>("ShowApplications", &(hint.first().copied().unwrap_or(""),))?
        }
        ["settings"] => proxy.call::<_, _, ()>("OpenSettings", &("",))?,
        ["wifi", "show", hint @ ..] if hint.len() <= 1 => {
            proxy.call::<_, _, ()>("ShowWifi", &(hint.first().copied().unwrap_or(""),))?
        }
        ["bluetooth", "show", hint @ ..] if hint.len() <= 1 => {
            proxy.call::<_, _, ()>("ShowBluetooth", &(hint.first().copied().unwrap_or(""),))?
        }
        ["brightness", "show", hint @ ..] if hint.len() <= 1 => {
            proxy.call::<_, _, ()>("ShowBrightness", &(hint.first().copied().unwrap_or(""),))?
        }
        ["temperatures", "show", hint @ ..] if hint.len() <= 1 => {
            proxy.call::<_, _, ()>("ShowTemperatures", &(hint.first().copied().unwrap_or(""),))?
        }
        ["power", "show", hint @ ..] if hint.len() <= 1 => {
            proxy.call::<_, _, ()>("ShowPower", &(hint.first().copied().unwrap_or(""),))?
        }
        ["control-center", hint @ ..] if hint.len() <= 1 => proxy.call::<_, _, ()>(
            "ToggleControlCenter",
            &(hint.first().copied().unwrap_or(""),),
        )?,
        ["trash", "status"] => {
            let state: String = proxy.call("GetTrashStatus", &())?;
            println!("{state}");
        }
        ["dialog", "state"] => {
            let state: DialogInfo = proxy.call("GetDialogState", &())?;
            print!("{}", toml::to_string(&state)?);
        }
        ["dialog", "close"] => proxy.call::<_, _, ()>("CloseDialog", &())?,
        ["dock", "show", hint @ ..] => {
            proxy.call::<_, _, ()>("ShowDock", &(hint.first().copied().unwrap_or(""),))?
        }
        ["dock", "cards"] => {
            let ids: Vec<u32> = proxy.call("GetDockCards", &())?;
            for id in ids {
                println!("{id}");
            }
        }
        ["dock", "cards", output] => {
            let ids: Vec<u32> = proxy.call("GetDockCardsForOutput", &(*output,))?;
            for id in ids {
                println!("{id}");
            }
        }
        ["dock", "state"] => {
            let info: DockInfo = proxy.call("GetDockState", &())?;
            print!("{}", toml::to_string(&info)?);
        }
        ["dock", "state", output] => {
            let info: DockInfo = proxy.call("GetDockStateForOutput", &(*output,))?;
            print!("{}", toml::to_string(&info)?);
        }
        ["config", "get"] => {
            let (_, text): (u64, String) = proxy.call("GetConfig", &())?;
            print!("{text}");
        }
        ["config", "set", patch] => {
            let (revision, _): (u64, String) = proxy.call("GetConfig", &())?;
            let (_, text): (u64, String) = proxy.call("ApplyPatch", &(revision, patch))?;
            print!("{text}");
        }
        ["reorder", source, before] => {
            let (revision, _): (u64, String) = proxy.call("GetConfig", &())?;
            let (_, text): (u64, String) = proxy.call("ReorderPin", &(revision, source, before))?;
            print!("{text}");
        }
        ["reload"] => proxy.call::<_, _, ()>("Reload", &())?,
        ["quit"] => proxy.call::<_, _, ()>("Quit", &())?,
        ["launch", id] => proxy.call::<_, _, ()>("Launch", &(id,))?,
        ["pin", id] => proxy.call::<_, _, ()>("Pin", &(id, true))?,
        ["unpin", id] => proxy.call::<_, _, ()>("Pin", &(id, false))?,
        ["windows"] => {
            let windows: Vec<(u64, u32, String, bool, bool)> = proxy.call("GetWindows", &())?;
            for (epoch, id, app, active, minimized) in windows {
                println!("{epoch}\t{id}\t{app:?}\t{active}\t{minimized}");
            }
        }
        ["window", action, epoch, id] => proxy.call::<_, _, ()>(
            "RequestWindow",
            &(epoch.parse::<u64>()?, id.parse::<u32>()?, action),
        )?,
        _ => return Err("invalid command".into()),
    }
    Ok(())
}
