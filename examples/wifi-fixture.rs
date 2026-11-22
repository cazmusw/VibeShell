use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use zbus::{
    fdo::ManagedObjects,
    names::OwnedInterfaceName,
    zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value},
};

const NM: &str = "org.freedesktop.NetworkManager";
const MAIN: &str = "/org/freedesktop/NetworkManager";
const WIFI: &str = "org.freedesktop.NetworkManager.Device.Wireless";
const DEVICE: &str = "org.freedesktop.NetworkManager.Device";
const AP: &str = "org.freedesktop.NetworkManager.AccessPoint";
const ADAPTER: &str = "/org/freedesktop/NetworkManager/Devices/1";
const UNMANAGED: &str = "/org/freedesktop/NetworkManager/Devices/2";
const CONNECTED: &str = "/org/freedesktop/NetworkManager/AccessPoint/1";
const REPEAT: &str = "/org/freedesktop/NetworkManager/AccessPoint/2";
const OPEN: &str = "/org/freedesktop/NetworkManager/AccessPoint/3";
const FOUND: &str = "/org/freedesktop/NetworkManager/AccessPoint/4";
const PERSONAL: &str = "/org/freedesktop/NetworkManager/AccessPoint/5";
const NEEDS_AGENT: &str = "/org/freedesktop/NetworkManager/AccessPoint/6";
const SETTINGS: &str = "/org/freedesktop/NetworkManager/Settings";
const ENTERPRISE_PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/20";
const WIRED_PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/21";
const OFFICE_PROFILE: &str = "/org/freedesktop/NetworkManager/Settings/22";

type Properties = HashMap<String, OwnedValue>;
fn value<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(value.into()).unwrap()
}
fn path(text: &str) -> OwnedObjectPath {
    ObjectPath::try_from(text).unwrap().into()
}
fn properties(items: &[(&str, OwnedValue)]) -> Properties {
    items
        .iter()
        .map(|(name, value)| (name.to_string(), value.try_clone().unwrap()))
        .collect()
}

struct State {
    last_scan: i64,
    enabled: bool,
    device_state: u32,
    reason: u32,
    saved: std::collections::HashSet<&'static str>,
    agent_available: bool,
    access_point: &'static str,
    active: String,
    sequence: u32,
}
type Shared = Arc<Mutex<State>>;
struct Manager(Shared);

#[zbus::interface(name = "org.freedesktop.DBus.ObjectManager")]
impl Manager {
    fn get_managed_objects(&self) -> ManagedObjects {
        let state = self.0.lock().unwrap();
        let mut objects = ManagedObjects::new();
        let mut attach = |object, interface, props| {
            objects
                .entry(path(object))
                .or_default()
                .insert(OwnedInterfaceName::try_from(interface).unwrap(), props);
        };
        attach(
            MAIN,
            NM,
            properties(&[
                ("WirelessEnabled", value(state.enabled)),
                ("WirelessHardwareEnabled", value(true)),
                ("Devices", value(vec![path(ADAPTER), path(UNMANAGED)])),
            ]),
        );
        let mut aps = vec![path(CONNECTED), path(REPEAT), path(OPEN)];
        if state.last_scan >= 0 {
            aps.extend([path(FOUND), path(PERSONAL), path(NEEDS_AGENT)]);
        }
        for (device, name, managed, points, active) in [
            (ADAPTER, "wlan0", true, aps, state.access_point),
            (UNMANAGED, "wlan1", false, Vec::new(), "/"),
        ] {
            attach(
                device,
                DEVICE,
                properties(&[
                    ("Interface", value(name)),
                    ("Managed", value(managed)),
                    (
                        "State",
                        value(if managed { state.device_state } else { 10_u32 }),
                    ),
                    (
                        "StateReason",
                        value(zbus::zvariant::Structure::from((
                            if managed { state.device_state } else { 10_u32 },
                            if managed { state.reason } else { 0_u32 },
                        ))),
                    ),
                    (
                        "ActiveConnection",
                        value(if managed {
                            path(&state.active)
                        } else {
                            path("/")
                        }),
                    ),
                ]),
            );
            attach(
                device,
                WIFI,
                properties(&[
                    ("ActiveAccessPoint", value(path(active))),
                    ("LastScan", value(state.last_scan)),
                    ("AccessPoints", value(points)),
                ]),
            );
        }
        for (object, ssid, strength, rsn) in [
            (CONNECTED, "Mosaic Lab", 46_u8, 0x100_u32),
            (REPEAT, "Mosaic Lab", 86, 0x100),
            (OPEN, "Guest", 63, 0),
            (FOUND, "New Test Network", 72, 0x100),
            (PERSONAL, "Personal Test", 51, 0x100),
            (NEEDS_AGENT, "Needs Agent", 42, 0x400),
        ] {
            attach(
                object,
                AP,
                properties(&[
                    ("Ssid", value(ssid.as_bytes().to_vec())),
                    ("Strength", value(strength)),
                    ("Flags", value(u32::from(rsn != 0))),
                    ("WpaFlags", value(0_u32)),
                    ("RsnFlags", value(rsn)),
                ]),
            );
        }
        objects
    }
}

struct Radio(Shared);
#[zbus::interface(name = "org.freedesktop.NetworkManager.Device.Wireless")]
impl Radio {
    fn request_scan(&self, _options: Properties) {
        let state = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(400));
            let mut state = state.lock().unwrap();
            state.last_scan += 1000;
        });
    }
}

struct TestControl(Shared);

struct SettingsInventory(SavedProfiles);
type SavedProfiles = Arc<Mutex<HashMap<&'static str, ProfileRecord>>>;

struct ProfileRecord {
    name: &'static str,
    uuid: &'static str,
    ssid: Option<&'static str>,
}

#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings")]
impl SettingsInventory {
    fn list_connections(&self) -> Vec<OwnedObjectPath> {
        self.0.lock().unwrap().keys().map(|id| path(id)).collect()
    }
}

struct SavedProfile(SavedProfiles, &'static str);
#[zbus::interface(name = "org.freedesktop.NetworkManager.Settings.Connection")]
impl SavedProfile {
    fn get_settings(&self) -> Result<HashMap<String, Properties>, FixtureError> {
        let records = self.0.lock().unwrap();
        let record = records
            .get(self.1)
            .ok_or_else(|| FixtureError::UnknownConnection("fixture profile was removed".into()))?;
        let kind = if record.ssid.is_some() {
            "802-11-wireless"
        } else {
            "802-3-ethernet"
        };
        let mut settings = HashMap::from([(
            "connection".into(),
            properties(&[
                ("id", value(record.name)),
                ("uuid", value(record.uuid)),
                ("type", value(kind)),
            ]),
        )]);
        if let Some(ssid) = record.ssid {
            settings.insert(
                kind.into(),
                properties(&[("ssid", value(ssid.as_bytes().to_vec()))]),
            );
        }
        Ok(settings)
    }

    fn delete(&self) -> Result<(), FixtureError> {
        self.0
            .lock()
            .unwrap()
            .remove(self.1)
            .map(|_| ())
            .ok_or_else(|| FixtureError::UnknownConnection("fixture profile was removed".into()))
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.NetworkManager")]
enum FixtureError {
    #[zbus(error)]
    ZBus(zbus::Error),
    UnknownConnection(String),
    InvalidArguments(String),
}

struct Activation(Shared);
#[zbus::interface(name = "org.freedesktop.NetworkManager")]
impl Activation {
    fn activate_connection(
        &self,
        profile: ObjectPath<'_>,
        device: ObjectPath<'_>,
        point: ObjectPath<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<OwnedObjectPath, FixtureError> {
        if profile.as_str() != "/" || device.as_str() != ADAPTER {
            return Err(FixtureError::InvalidArguments(
                "unexpected profile or target device".into(),
            ));
        }
        let situation = self.0.lock().unwrap();
        let target = match point.as_str() {
            CONNECTED => CONNECTED,
            REPEAT => REPEAT,
            FOUND => FOUND,
            PERSONAL if situation.saved.contains(PERSONAL) => PERSONAL,
            NEEDS_AGENT if situation.saved.contains(NEEDS_AGENT) => NEEDS_AGENT,
            _ => return Err(FixtureError::UnknownConnection("no saved profile".into())),
        };
        drop(situation);
        Ok(self.begin_activation(connection, target, 40))
    }

    #[zbus(out_args("profile", "active", "result"))]
    fn add_and_activate_connection2(
        &self,
        settings: HashMap<String, Properties>,
        device: ObjectPath<'_>,
        point: ObjectPath<'_>,
        options: Properties,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(OwnedObjectPath, OwnedObjectPath, Properties), FixtureError> {
        let (target, ssid, mode, profile) = match point.as_str() {
            PERSONAL => (PERSONAL, "Personal Test", "wpa-psk", "5"),
            NEEDS_AGENT => (NEEDS_AGENT, "Needs Agent", "sae", "6"),
            _ => {
                return Err(FixtureError::InvalidArguments(
                    "unexpected new network".into(),
                ));
            }
        };
        let get = |group: &str, key: &str| settings.get(group)?.get(key);
        let section = "802-11-wireless-security";
        let bytes = get("802-11-wireless", "ssid")
            .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok());
        let strings = |group, key| {
            get(group, key).and_then(|value| Vec::<String>::try_from(value.try_clone().ok()?).ok())
        };
        let invalid = device.as_str() != ADAPTER
            || bytes.as_deref() != Some(ssid.as_bytes())
            || get("connection", "type").and_then(|value| <&str>::try_from(value).ok())
                != Some("802-11-wireless")
            || strings("connection", "permissions").as_deref()
                != Some(&[format!(
                    "user:{}:",
                    gtk::glib::user_name().to_string_lossy()
                )])
            || get(section, "key-mgmt").and_then(|value| <&str>::try_from(value).ok())
                != Some(mode)
            || strings(section, "proto").as_deref() != Some(&["rsn".into()])
            || get(section, "psk-flags").and_then(|value| u32::try_from(value).ok()) != Some(1)
            || get(section, "psk").is_some()
            || options
                .get("persist")
                .and_then(|value| <&str>::try_from(value).ok())
                != Some("disk");
        if invalid {
            return Err(FixtureError::InvalidArguments(
                "unexpected profile settings".into(),
            ));
        }

        if !self.0.lock().unwrap().saved.insert(target) {
            return Err(FixtureError::InvalidArguments(
                "profile already exists".into(),
            ));
        }
        Ok((
            path(&format!("{MAIN}/Settings/{profile}")),
            self.begin_activation(connection, target, 60),
            Properties::new(),
        ))
    }
}

impl Activation {
    fn begin_activation(
        &self,
        connection: &zbus::Connection,
        target: &'static str,
        starting: u32,
    ) -> OwnedObjectPath {
        let mut state = self.0.lock().unwrap();
        state.sequence += 1;
        state.active = format!("{MAIN}/ActiveConnection/{}", state.sequence);
        state.access_point = "/";
        state.device_state = starting;
        state.reason = 1;
        let active = path(&state.active);
        let failure = target == NEEDS_AGENT && !state.agent_available;
        let shared = self.0.clone();
        let connection = zbus::blocking::Connection::from(connection.clone());
        std::thread::spawn(move || {
            let signal = |current, previous, reason| {
                connection
                    .emit_signal(
                        None::<&str>,
                        ADAPTER,
                        DEVICE,
                        "StateChanged",
                        &(current, previous, reason),
                    )
                    .unwrap();
            };
            std::thread::sleep(Duration::from_millis(if starting == 60 {
                1000
            } else {
                650
            }));
            if failure {
                {
                    let mut state = shared.lock().unwrap();
                    state.device_state = 120;
                    state.reason = 7;
                }
                signal(120_u32, starting, 7_u32);
                {
                    let mut state = shared.lock().unwrap();
                    state.active = "/".into();
                    state.device_state = 30;
                    state.reason = 1;
                }
                signal(30, 120, 1);
            } else {
                let mut state = shared.lock().unwrap();
                state.access_point = target;
                state.device_state = 100;
                drop(state);
                signal(100, starting, 1);
            }
        });
        active
    }
}

struct DeviceAction(Shared);
#[zbus::interface(name = "org.freedesktop.NetworkManager.Device")]
impl DeviceAction {
    fn disconnect(&self) {
        self.0.lock().unwrap().device_state = 110;
        let state = self.0.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(650));
            let mut state = state.lock().unwrap();
            state.device_state = 30;
            state.access_point = "/";
            state.active = "/".into();
        });
    }
}

#[zbus::interface(name = "org.mosaic.WifiFixture")]
impl TestControl {
    fn set_enabled(&self, enabled: bool) {
        self.0.lock().unwrap().enabled = enabled;
    }

    fn enable_agent(&self) {
        self.0.lock().unwrap().agent_available = true;
    }
}

fn main() -> zbus::Result<()> {
    let profiles = Arc::new(Mutex::new(HashMap::from([
        (
            ENTERPRISE_PROFILE,
            ProfileRecord {
                name: "Enterprise Lab",
                uuid: "076f40cd-da3a-4a77-9900-a23107857820",
                ssid: Some("Office 802.1X"),
            },
        ),
        (
            WIRED_PROFILE,
            ProfileRecord {
                name: "Wired profile",
                uuid: "076f40cd-da3a-4a77-9900-a23107857821",
                ssid: None,
            },
        ),
        (
            OFFICE_PROFILE,
            ProfileRecord {
                name: "Office Backup",
                uuid: "076f40cd-da3a-4a77-9900-a23107857822",
                ssid: Some("Office 802.1X"),
            },
        ),
    ])));
    let state = Arc::new(Mutex::new(State {
        last_scan: -1,
        enabled: true,
        device_state: 100,
        reason: 0,
        saved: std::collections::HashSet::new(),
        agent_available: false,
        access_point: CONNECTED,
        active: format!("{MAIN}/ActiveConnection/1"),
        sequence: 1,
    }));
    let _connection = zbus::blocking::connection::Builder::session()?
        .name(NM)?
        .serve_at(SETTINGS, SettingsInventory(profiles.clone()))?
        .serve_at(
            ENTERPRISE_PROFILE,
            SavedProfile(profiles.clone(), ENTERPRISE_PROFILE),
        )?
        .serve_at(WIRED_PROFILE, SavedProfile(profiles.clone(), WIRED_PROFILE))?
        .serve_at(OFFICE_PROFILE, SavedProfile(profiles, OFFICE_PROFILE))?
        .serve_at("/org/freedesktop", Manager(state.clone()))?
        .serve_at(ADAPTER, Radio(state.clone()))?
        .serve_at(ADAPTER, DeviceAction(state.clone()))?
        .serve_at(MAIN, Activation(state.clone()))?
        .serve_at(MAIN, TestControl(state))?
        .build()?;
    loop {
        std::thread::park();
    }
}
