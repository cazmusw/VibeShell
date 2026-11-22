mod profiles;
use futures_lite::{StreamExt, future};
pub use profiles::{WifiProfile, WifiProfiles};
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, HashMap, HashSet},
    fmt::Write,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use zbus::{
    Connection, Proxy, Task,
    fdo::{DBusProxy, ManagedObjects},
    names::OwnedUniqueName,
    proxy::MethodFlags,
    zvariant::{OwnedObjectPath, OwnedValue, Value},
};

const NM: &str = "org.freedesktop.NetworkManager";
const NM_ROOT: &str = "/org/freedesktop/NetworkManager";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const WIRELESS: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const ACCESS_POINT: &str = "org.freedesktop.NetworkManager.AccessPoint";
const MAX_ADAPTERS: usize = 16;
const MAX_NETWORKS: usize = 128;
const SCAN_INTERVAL: Duration = Duration::from_secs(8);
const ACTIVATION_WATCH: Duration = Duration::from_secs(125);
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Default, Eq, PartialEq, Deserialize, Serialize)]
pub struct WifiSnapshot {
    pub available: bool,
    pub enabled: bool,
    pub hardware_enabled: bool,
    pub adapters: Vec<WifiAdapter>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WifiAdapter {
    pub path: String,
    pub name: String,
    pub managed: bool,
    pub last_scan: i64,
    pub networks: Vec<WifiNetwork>,
    #[serde(default)]
    pub state: u32,
    #[serde(default)]
    pub active_connection: String,
    #[serde(default)]
    pub activation_failure: Option<WifiActivationFailure>,
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WifiActivationFailure {
    pub active: String,
    pub reason: u32,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Protection {
    Open,
    Secured,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PersonalMode {
    WpaPsk,
    Sae,
}

impl PersonalMode {
    pub fn key_mgmt(self) -> &'static str {
        match self {
            Self::WpaPsk => "wpa-psk",
            Self::Sae => "sae",
        }
    }
}

pub fn personal_mode(rsn_flags: u32) -> Option<PersonalMode> {
    if rsn_flags & 0x100 != 0 {
        Some(PersonalMode::WpaPsk)
    } else if rsn_flags & 0x400 != 0 {
        Some(PersonalMode::Sae)
    } else {
        None
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Deserialize, Serialize)]
pub struct WifiNetwork {
    pub ssid: Vec<u8>,
    pub name: String,
    pub protection: Protection,
    pub strength: u8,
    pub connected: bool,
    #[serde(default)]
    pub access_point: String,
    #[serde(default)]
    pub personal: Option<PersonalMode>,
}

pub fn ssid_label(raw: &[u8]) -> String {
    if raw.is_empty() {
        return "Скрытая сеть".into();
    }
    let mut label = String::new();
    if let Ok(text) = std::str::from_utf8(raw) {
        for c in text.chars() {
            if c.is_control()
                || matches!(c, '\u{061c}' | '\u{200e}'..='\u{200f}' |
                '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}' | '\\')
            {
                label.extend(c.escape_default());
            } else {
                label.push(c);
            }
        }
    } else {
        for &byte in raw {
            if byte.is_ascii_graphic() && byte != b'\\' || byte == b' ' {
                label.push(char::from(byte));
            } else {
                let _ = write!(label, "\\x{byte:02x}");
            }
        }
    }
    label
}

pub fn group_networks(networks: impl IntoIterator<Item = WifiNetwork>) -> Vec<WifiNetwork> {
    let mut groups = BTreeMap::new();
    for network in networks {
        if network.ssid.is_empty() && !network.connected {
            continue;
        }
        let entry = groups
            .entry((network.ssid.clone(), network.protection))
            .or_insert_with(|| network.clone());
        if (network.connected, network.strength) > (entry.connected, entry.strength) {
            *entry = network;
        }
    }
    let mut networks = groups.into_values().collect::<Vec<_>>();
    networks.sort_by(|a, b| {
        Reverse((a.connected, a.strength))
            .cmp(&Reverse((b.connected, b.strength)))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.protection.cmp(&b.protection))
    });
    networks.truncate(MAX_NETWORKS);
    networks
}

type Properties = HashMap<String, OwnedValue>;

fn props<'a>(objects: &'a ManagedObjects, path: &str, interface: &str) -> Option<&'a Properties> {
    // The object keys are D-Bus paths, and names are validated by zvariant.
    let path = zbus::zvariant::ObjectPath::try_from(path).ok()?;
    let path = OwnedObjectPath::from(path);
    objects
        .get(&path)?
        .iter()
        .find(|(name, _)| name.as_str() == interface)
        .map(|(_, p)| p)
}

fn property<'a, T: TryFrom<&'a OwnedValue>>(properties: &'a Properties, name: &str) -> Option<T> {
    T::try_from(properties.get(name)?).ok()
}

fn array<T: TryFrom<OwnedValue>>(properties: &Properties, name: &str) -> Option<T> {
    T::try_from(properties.get(name)?.try_clone().ok()?).ok()
}

pub fn decode_snapshot(objects: &ManagedObjects) -> WifiSnapshot {
    let Some(manager) = props(objects, NM_ROOT, NM) else {
        return WifiSnapshot::default();
    };
    let enabled = property(manager, "WirelessEnabled").unwrap_or(false);
    let hardware_enabled = property(manager, "WirelessHardwareEnabled").unwrap_or(false);
    let mut adapters = Vec::new();
    for path in array::<Vec<OwnedObjectPath>>(manager, "Devices")
        .unwrap_or_default()
        .into_iter()
        .take(64)
    {
        let (Some(device), Some(wifi)) = (
            props(objects, path.as_str(), DEVICE),
            props(objects, path.as_str(), WIRELESS),
        ) else {
            continue;
        };
        let active = property::<&zbus::zvariant::ObjectPath<'_>>(wifi, "ActiveAccessPoint");
        let state = property::<u32>(device, "State").unwrap_or(0);
        let mut points = array::<Vec<OwnedObjectPath>>(wifi, "AccessPoints").unwrap_or_default();
        points.truncate(512);
        if let Some(active) = active
            && active.as_str() != "/"
            && !points.iter().any(|path| path.as_str() == active.as_str())
        {
            points.push(OwnedObjectPath::from(active.clone()));
        }
        let networks = group_networks(points.into_iter().filter_map(|point| {
            if !enabled || !hardware_enabled {
                return None;
            }
            let access = props(objects, point.as_str(), ACCESS_POINT)?;
            let ssid = array::<Vec<u8>>(access, "Ssid")?;
            if ssid.len() > 32 {
                return None;
            }
            let rsn = property::<u32>(access, "RsnFlags").unwrap_or(0);
            let private = property::<u32>(access, "Flags").unwrap_or(0) & 1 != 0
                || property::<u32>(access, "WpaFlags").unwrap_or(0) != 0
                || rsn != 0;
            Some(WifiNetwork {
                name: ssid_label(&ssid),
                ssid,
                protection: if private {
                    Protection::Secured
                } else {
                    Protection::Open
                },
                strength: property::<u8>(access, "Strength").unwrap_or(0).min(100),
                connected: state == 100
                    && active.is_some_and(|active| active.as_str() == point.as_str()),
                access_point: point.to_string(),
                personal: personal_mode(rsn),
            })
        }));
        let name = property::<&str>(device, "Interface").unwrap_or("Wi-Fi");
        adapters.push(WifiAdapter {
            path: path.to_string(),
            name: name.chars().filter(|c| !c.is_control()).take(48).collect(),
            managed: property(device, "Managed").unwrap_or(false),
            last_scan: property(wifi, "LastScan").unwrap_or(-1),
            networks,
            state,
            activation_failure: None,
            active_connection: property::<&zbus::zvariant::ObjectPath<'_>>(
                device,
                "ActiveConnection",
            )
            .filter(|path| path.as_str() != "/")
            .map(ToString::to_string)
            .unwrap_or_default(),
        });
    }
    adapters.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.path.cmp(&b.path)));
    adapters.truncate(MAX_ADAPTERS);
    WifiSnapshot {
        available: true,
        enabled,
        hardware_enabled,
        adapters,
    }
}

#[derive(Default)]
pub struct NetworkService {
    connection: Mutex<Option<Connection>>,
    scans: Mutex<HashMap<String, Instant>>,
    connection_actions: Mutex<HashSet<String>>,
    activations: Mutex<HashMap<String, ActivationWatch>>,
}

struct ActivationWatch {
    active: String,
    started: Instant,
    reason: Arc<Mutex<Option<u32>>>,
    _task: Task<()>,
}

struct ActionGuard<'a>(&'a Mutex<HashSet<String>>, &'a str);

impl Drop for ActionGuard<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().remove(self.1);
    }
}

impl NetworkService {
    async fn connection(&self) -> zbus::Result<Connection> {
        if let Some(connection) = self.connection.lock().unwrap().clone() {
            return Ok(connection);
        }
        let connection = zbus::connection::Builder::system()?
            .method_timeout(Duration::from_secs(45))
            .build()
            .await?;
        let mut cached = self.connection.lock().unwrap();
        Ok(cached.get_or_insert(connection).clone())
    }

    async fn query(
        &self,
        connection: &Connection,
        destination: &str,
    ) -> zbus::Result<WifiSnapshot> {
        let proxy = Proxy::new(
            connection,
            destination,
            "/org/freedesktop",
            "org.freedesktop.DBus.ObjectManager",
        )
        .await?;
        let request = proxy.call_with_flags::<_, _, ManagedObjects>(
            "GetManagedObjects",
            MethodFlags::NoAutoStart.into(),
            &(),
        );
        let reply = future::race(request, async {
            async_io::Timer::after(SNAPSHOT_TIMEOUT).await;
            Err(std::io::Error::from(std::io::ErrorKind::TimedOut).into())
        })
        .await?
        .ok_or(zbus::Error::InvalidReply)?;
        Ok(decode_snapshot(&reply))
    }

    pub async fn snapshot(&self) -> WifiSnapshot {
        let Ok(connection) = self.connection().await else {
            return WifiSnapshot::default();
        };
        match self.query(&connection, NM).await {
            Ok(mut snapshot) => {
                let mut watches = self.activations.lock().unwrap();
                watches.retain(|_, watch| watch.started.elapsed() < ACTIVATION_WATCH);
                for adapter in &mut snapshot.adapters {
                    if let Some(watch) = watches.get(&adapter.path)
                        && let Some(reason) = *watch.reason.lock().unwrap()
                    {
                        adapter.activation_failure = Some(WifiActivationFailure {
                            active: watch.active.clone(),
                            reason,
                        });
                    }
                }
                snapshot
            }
            Err(error) => {
                self.activations.lock().unwrap().clear();
                if !matches!(error, zbus::Error::MethodError(..)) {
                    self.connection.lock().unwrap().take();
                }
                WifiSnapshot::default()
            }
        }
    }

    async fn action_target(
        &self,
        path: &str,
    ) -> Result<(Connection, OwnedUniqueName, WifiAdapter), crate::ipc::ApiError> {
        use crate::ipc::ApiError;
        if path.len() > 160 || !path.starts_with("/org/freedesktop/NetworkManager/Devices/") {
            return Err(ApiError::InvalidArgument("invalid adapter path".into()));
        }
        let (connection, owner) = self.network_manager().await?;
        let snapshot = self
            .query(&connection, owner.as_str())
            .await
            .map_err(ApiError::ZBus)?;
        if !snapshot.enabled || !snapshot.hardware_enabled {
            return Err(ApiError::Failed("Wi-Fi is disabled or unavailable".into()));
        }
        let adapter = snapshot
            .adapters
            .into_iter()
            .find(|adapter| adapter.path == path && adapter.managed)
            .ok_or_else(|| {
                ApiError::InvalidArgument("unknown or unmanaged Wi-Fi adapter".into())
            })?;
        Ok((connection, owner, adapter))
    }

    async fn network_manager(&self) -> zbus::Result<(Connection, OwnedUniqueName)> {
        let connection = self.connection().await?;
        let dbus = DBusProxy::new(&connection).await?;
        let owner = dbus.get_name_owner(NM.try_into()?).await?;
        Ok((connection, owner))
    }

    fn lock_action<'a>(&'a self, path: &'a str) -> Result<ActionGuard<'a>, crate::ipc::ApiError> {
        if path.len() > 160 || !path.starts_with("/org/freedesktop/NetworkManager/Devices/") {
            return Err(crate::ipc::ApiError::InvalidArgument(
                "invalid adapter path".into(),
            ));
        }
        self.lock_path(path)
    }

    fn lock_path<'a>(&'a self, path: &'a str) -> Result<ActionGuard<'a>, crate::ipc::ApiError> {
        if !self
            .connection_actions
            .lock()
            .unwrap()
            .insert(path.to_owned())
        {
            return Err(crate::ipc::ApiError::Conflict(
                "connection action already pending".into(),
            ));
        }
        Ok(ActionGuard(&self.connection_actions, path))
    }

    pub async fn connect_saved(
        &self,
        path: &str,
        ssid: &[u8],
        secured: bool,
    ) -> Result<String, crate::ipc::ApiError> {
        self.connect(path, ssid, secured, None).await
    }

    pub async fn join_personal(
        &self,
        path: &str,
        ssid: &[u8],
        mode: &str,
    ) -> Result<String, crate::ipc::ApiError> {
        let mode = match mode {
            "wpa-psk" => PersonalMode::WpaPsk,
            "sae" => PersonalMode::Sae,
            _ => {
                return Err(crate::ipc::ApiError::InvalidArgument(
                    "unsupported Wi-Fi security".into(),
                ));
            }
        };
        self.connect(path, ssid, true, Some(mode)).await
    }

    async fn connect(
        &self,
        path: &str,
        ssid: &[u8],
        secured: bool,
        new_profile: Option<PersonalMode>,
    ) -> Result<String, crate::ipc::ApiError> {
        use crate::ipc::ApiError;
        if ssid.is_empty() || ssid.len() > 32 {
            return Err(ApiError::InvalidArgument("invalid SSID length".into()));
        }
        let _guard = self.lock_action(path)?;
        let (connection, owner, adapter) = self.action_target(path).await?;
        if (40..100).contains(&adapter.state) || adapter.state == 110 {
            return Err(ApiError::Conflict(
                "connection action already pending".into(),
            ));
        }
        let network = adapter
            .networks
            .iter()
            .find(|network| {
                network.ssid == ssid && (network.protection == Protection::Secured) == secured
            })
            .ok_or_else(|| ApiError::Conflict("selected Wi-Fi network disappeared".into()))?;
        if new_profile.is_some() && network.personal != new_profile {
            return Err(ApiError::Conflict(
                "selected WPA-Personal security changed".into(),
            ));
        }
        if network.connected && !adapter.active_connection.is_empty() {
            return Ok(adapter.active_connection);
        }
        let proxy = Proxy::new(&connection, owner.as_str(), NM_ROOT, NM).await?;
        let device = Proxy::new(&connection, owner.as_str(), path, DEVICE).await?;
        let changes = device.receive_signal("StateChanged").await?;
        let default_profile = zbus::zvariant::ObjectPath::try_from("/").unwrap();
        let adapter = zbus::zvariant::ObjectPath::try_from(path).map_err(zbus::Error::from)?;
        let access_point = zbus::zvariant::ObjectPath::try_from(network.access_point.as_str())
            .map_err(zbus::Error::from)?;
        let active = proxy
            .call_with_flags::<_, _, OwnedObjectPath>(
                "ActivateConnection",
                MethodFlags::NoAutoStart | MethodFlags::AllowInteractiveAuth,
                &(default_profile, &adapter, &access_point),
            )
            .await
            .map_err(network_error);
        let active = match (active, new_profile) {
            (Ok(Some(active)), _) => active,
            (Err(ApiError::NoSavedProfile(_)), Some(mode)) => {
                Self::create_personal_profile(&proxy, network, mode, adapter, access_point).await?
            }
            (Err(error), _) => return Err(error),
            (Ok(None), _) => return Err(zbus::Error::InvalidReply.into()),
        };
        let active = active.to_string();
        self.watch_activation(&connection, path, &active, changes);
        Ok(active)
    }

    fn watch_activation(
        &self,
        connection: &Connection,
        path: &str,
        active: &str,
        changes: zbus::proxy::SignalStream<'static>,
    ) {
        let reason = Arc::new(Mutex::new(None));
        let task_reason = reason.clone();
        let task = connection.executor().spawn(
            async move {
                // A failed device state can last less than one snapshot interval.
                let read_changes = async move {
                    let mut changes = changes;
                    while let Some(change) = changes.next().await {
                        let Ok((state, previous, reason)) =
                            change.body().deserialize::<(u32, u32, u32)>()
                        else {
                            continue;
                        };
                        match state {
                            100 => break,
                            120 if (40..=90).contains(&previous) => {
                                *task_reason.lock().unwrap() = Some(reason);
                                break;
                            }
                            _ => (),
                        }
                    }
                };
                future::race(read_changes, async {
                    async_io::Timer::after(ACTIVATION_WATCH).await;
                })
                .await;
            },
            "wifi activation",
        );
        let mut watches = self.activations.lock().unwrap();
        watches.retain(|_, watch| watch.started.elapsed() < ACTIVATION_WATCH);
        watches.insert(
            path.to_owned(),
            ActivationWatch {
                active: active.to_owned(),
                started: Instant::now(),
                reason,
                _task: task,
            },
        );
    }

    async fn create_personal_profile(
        proxy: &Proxy<'_>,
        network: &WifiNetwork,
        mode: PersonalMode,
        adapter: zbus::zvariant::ObjectPath<'_>,
        point: zbus::zvariant::ObjectPath<'_>,
    ) -> Result<OwnedObjectPath, crate::ipc::ApiError> {
        use crate::ipc::ApiError;
        let username = gtk::glib::user_name();
        let username = username
            .to_str()
            .filter(|name| {
                !name.is_empty() && !name.contains(':') && !name.chars().any(char::is_control)
            })
            .ok_or_else(|| ApiError::Failed("cannot identify desktop user".into()))?;
        let settings = HashMap::from([
            (
                "connection",
                HashMap::from([
                    ("type", Value::from("802-11-wireless")),
                    ("id", Value::from(network.name.as_str())),
                    (
                        "permissions",
                        Value::from(vec![format!("user:{username}:")]),
                    ),
                ]),
            ),
            (
                "802-11-wireless",
                HashMap::from([
                    ("ssid", Value::from(network.ssid.clone())),
                    ("mode", Value::from("infrastructure")),
                ]),
            ),
            (
                "802-11-wireless-security",
                HashMap::from([
                    ("key-mgmt", Value::from(mode.key_mgmt())),
                    ("proto", Value::from(vec!["rsn"])),
                    ("psk-flags", Value::from(1_u32)),
                ]),
            ),
        ]);
        let options = HashMap::from([("persist", Value::from("disk"))]);
        type Activation = (OwnedObjectPath, OwnedObjectPath, Properties);
        let (_, active, _) = proxy
            .call_with_flags::<_, _, Activation>(
                "AddAndActivateConnection2",
                MethodFlags::NoAutoStart | MethodFlags::AllowInteractiveAuth,
                &(settings, adapter, point, options),
            )
            .await
            .map_err(network_error)?
            .ok_or(zbus::Error::InvalidReply)?;
        Ok(active)
    }

    pub async fn disconnect(&self, path: &str, expected: &str) -> Result<(), crate::ipc::ApiError> {
        use crate::ipc::ApiError;
        let _guard = self.lock_action(path)?;
        let (connection, owner, adapter) = self.action_target(path).await?;
        if expected.is_empty() || adapter.active_connection != expected || adapter.state != 100 {
            return Err(ApiError::Conflict("active Wi-Fi connection changed".into()));
        }
        let proxy = Proxy::new(&connection, owner.as_str(), path, DEVICE).await?;
        proxy
            .call_with_flags::<_, _, ()>("Disconnect", MethodFlags::NoAutoStart.into(), &())
            .await
            .map_err(network_error)?;
        Ok(())
    }

    pub async fn scan(&self, path: &str) -> Result<(), crate::ipc::ApiError> {
        use crate::ipc::ApiError;
        let (connection, owner, _) = self.action_target(path).await?;
        {
            let now = Instant::now();
            let mut scans = self.scans.lock().unwrap();
            scans.retain(|_, previous| now.saturating_duration_since(*previous) < SCAN_INTERVAL);
            if scans.contains_key(path) {
                return Err(ApiError::Conflict("scan requested too recently".into()));
            }
            scans.insert(path.to_owned(), now);
        }
        let proxy = Proxy::new(&connection, owner.as_str(), path, WIRELESS)
            .await
            .map_err(ApiError::ZBus)?;
        let options: HashMap<&str, Value<'_>> = HashMap::new();
        match proxy
            .call_with_flags::<_, _, ()>(
                "RequestScan",
                MethodFlags::NoAutoStart.into(),
                &(options,),
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(error) => {
                self.scans.lock().unwrap().remove(path);
                Err(network_error(error))
            }
        }
    }
}

fn network_error(error: zbus::Error) -> crate::ipc::ApiError {
    use crate::ipc::ApiError;
    if let zbus::Error::MethodError(name, _, _) = &error {
        match name.as_str().rsplit('.').next() {
            Some("PermissionDenied" | "AccessDenied") => {
                return ApiError::PermissionDenied("NetworkManager refused the operation".into());
            }
            Some("UnknownConnection") => {
                return ApiError::NoSavedProfile("no compatible saved profile was found".into());
            }
            Some("NoSecrets" | "NoAgents") => {
                return ApiError::SecretsUnavailable(
                    "NetworkManager did not receive credentials".into(),
                );
            }
            _ => (),
        }
    }
    ApiError::ZBus(error)
}
