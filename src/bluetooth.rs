mod pairing;

use crate::ipc::ApiError;
use futures_lite::future;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, HashSet, hash_map::Entry},
    sync::{Arc, Mutex},
    time::Duration,
};
use zbus::{
    Connection, Proxy,
    fdo::{DBusProxy, ManagedObjects},
    names::OwnedUniqueName,
    proxy::MethodFlags,
    zvariant::{ObjectPath, OwnedValue, Value},
};

const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const SEARCH_DURATION: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Snapshot {
    pub available: bool,
    #[serde(default)]
    pub pairing_enabled: bool,
    pub owner: String,
    pub adapters: Vec<Adapter>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Adapter {
    pub path: String,
    pub name: String,
    pub powered: bool,
    pub discovering: bool,
    pub searching: bool,
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub address: String,
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<u8>,
}

type Properties = HashMap<String, OwnedValue>;

fn get<'a, T: TryFrom<&'a OwnedValue>>(properties: &'a Properties, key: &str) -> Option<T> {
    properties.get(key)?.try_into().ok()
}

fn label(properties: &Properties, default: &str) -> String {
    let name = get::<&str>(properties, "Alias")
        .or_else(|| get(properties, "Name"))
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(default);
    crate::network::ssid_label(name.as_bytes())
        .chars()
        .take(96)
        .collect()
}

fn decode(owner: &str, objects: &ManagedObjects) -> Snapshot {
    let mut adapters = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let props = interfaces.get(ADAPTER)?;
            Some(Adapter {
                path: path.to_string(),
                name: label(props, path.rsplit('/').next().unwrap_or("Bluetooth")),
                powered: get(props, "Powered").unwrap_or(false),
                discovering: get(props, "Discovering").unwrap_or(false),
                searching: false,
                devices: Vec::new(),
            })
        })
        .collect::<Vec<_>>();
    adapters.sort_by(|a, b| a.path.cmp(&b.path));
    for (path, interfaces) in objects {
        let Some(device) = interfaces.get(DEVICE) else {
            continue;
        };
        let Some(adapter_path) = get::<&ObjectPath<'_>>(device, "Adapter") else {
            continue;
        };
        let Some(adapter) = adapters
            .iter_mut()
            .find(|adapter| adapter.path == adapter_path.as_str())
        else {
            continue;
        };
        let address = get::<&str>(device, "Address").unwrap_or_default();
        adapter.devices.push(Device {
            path: path.to_string(),
            name: label(
                device,
                if address.is_empty() {
                    "Устройство Bluetooth"
                } else {
                    address
                },
            ),
            address: address.into(),
            paired: get(device, "Paired").unwrap_or(false),
            connected: get(device, "Connected").unwrap_or(false),
            battery: interfaces
                .get(BATTERY)
                .and_then(|props| get::<u8>(props, "Percentage"))
                .filter(|&percent| percent <= 100),
        });
    }
    for adapter in &mut adapters {
        adapter.devices.sort_by(|a, b| {
            b.connected
                .cmp(&a.connected)
                .then_with(|| b.paired.cmp(&a.paired))
                .then_with(|| a.name.cmp(&b.name))
                .then_with(|| a.address.cmp(&b.address))
        });
    }
    Snapshot {
        available: true,
        pairing_enabled: false,
        owner: owner.into(),
        adapters,
    }
}

type ScanKey = (String, String);
type Searches = Arc<Mutex<HashMap<ScanKey, async_channel::Sender<()>>>>;

#[derive(Default)]
pub struct Service {
    system: Mutex<Option<Connection>>,
    searches: Searches,
    pairing_enabled: bool,
    pairing: Mutex<Option<Arc<pairing::Attempt>>>,
    devices: Mutex<HashSet<ScanKey>>,
}

pub struct DeviceTarget<'a> {
    pub owner: &'a str,
    pub adapter: &'a str,
    pub path: &'a str,
    pub address: &'a str,
}

struct DeviceLock<'a> {
    devices: &'a Mutex<HashSet<ScanKey>>,
    key: ScanKey,
}

impl Drop for DeviceLock<'_> {
    fn drop(&mut self) {
        self.devices.lock().unwrap().remove(&self.key);
    }
}

struct SearchLease {
    searches: Searches,
    key: ScanKey,
    stop: async_channel::Sender<()>,
}

impl Drop for SearchLease {
    fn drop(&mut self) {
        let mut searches = self.searches.lock().unwrap();
        if searches
            .get(&self.key)
            .is_some_and(|stop| stop.same_channel(&self.stop))
        {
            searches.remove(&self.key);
        }
    }
}

async fn system_bus() -> zbus::Result<Connection> {
    zbus::connection::Builder::system()?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await
}

async fn owner(connection: &Connection) -> zbus::Result<OwnedUniqueName> {
    Ok(DBusProxy::new(connection)
        .await?
        .get_name_owner(BLUEZ.try_into()?)
        .await?)
}

async fn query(connection: &Connection, owner: &OwnedUniqueName) -> zbus::Result<Snapshot> {
    let proxy = Proxy::new(
        connection,
        owner.as_str(),
        "/",
        "org.freedesktop.DBus.ObjectManager",
    )
    .await?;
    let objects = proxy
        .call_with_flags::<_, _, ManagedObjects>(
            "GetManagedObjects",
            MethodFlags::NoAutoStart.into(),
            &(),
        )
        .await?
        .ok_or(zbus::Error::InvalidReply)?;
    Ok(decode(owner.as_str(), &objects))
}

impl Service {
    pub fn with_pairing(pairing_enabled: bool) -> Self {
        Self {
            pairing_enabled,
            ..Self::default()
        }
    }

    fn lock_device(&self, target: &DeviceTarget<'_>) -> Result<DeviceLock<'_>, ApiError> {
        if target.path.len() > 192 || target.owner.len() > 255 || target.address.is_empty() {
            return Err(ApiError::InvalidArgument(
                "invalid Bluetooth device identity".into(),
            ));
        }
        let key = (target.owner.into(), target.path.into());
        if !self.devices.lock().unwrap().insert(key.clone()) {
            return Err(ApiError::Conflict(
                "operation already pending on this Bluetooth device".into(),
            ));
        }
        Ok(DeviceLock {
            devices: &self.devices,
            key,
        })
    }

    async fn device_target(
        &self,
        bus: &Connection,
        target: &DeviceTarget<'_>,
    ) -> Result<(OwnedUniqueName, Adapter, Device), ApiError> {
        let (owner, adapter) = self.adapter(bus, target.owner, target.adapter).await?;
        let device = adapter
            .devices
            .iter()
            .find(|device| device.path == target.path && device.address == target.address)
            .cloned()
            .ok_or_else(|| ApiError::Conflict("Bluetooth device changed".into()))?;
        Ok((owner, adapter, device))
    }

    pub async fn change_device(
        &self,
        target: DeviceTarget<'_>,
        action: &str,
    ) -> Result<(), ApiError> {
        if !matches!(action, "connect" | "disconnect" | "remove") {
            return Err(ApiError::InvalidArgument(
                "unknown Bluetooth device action".into(),
            ));
        }
        let _lock = self.lock_device(&target)?;
        let state_bus = self.bus().await?;
        let (owner, adapter, device) = self.device_target(&state_bus, &target).await?;
        let bus = zbus::connection::Builder::system()?
            .method_timeout(Duration::from_secs(30))
            .build()
            .await?;
        let flags = MethodFlags::NoAutoStart.into();
        let reply = if action == "remove" {
            if !device.paired && !device.connected {
                return Err(ApiError::Conflict(
                    "Bluetooth device has no saved pairing".into(),
                ));
            }
            let path = ObjectPath::try_from(device.path.as_str()).map_err(zbus::Error::from)?;
            Proxy::new(&bus, owner.as_str(), target.adapter, ADAPTER)
                .await?
                .call_with_flags::<_, _, ()>("RemoveDevice", flags, &(path,))
                .await
        } else {
            if !adapter.powered || (action == "connect" && !device.paired) {
                return Err(ApiError::Conflict("Bluetooth device state changed".into()));
            }
            let connect = action == "connect";
            if device.connected == connect {
                return Ok(());
            }
            let method = if connect { "Connect" } else { "Disconnect" };
            let reply = Proxy::new(&bus, owner.as_str(), device.path.as_str(), DEVICE)
                .await?
                .call_with_flags::<_, _, ()>(method, flags, &())
                .await;
            if connect
                && matches!(&reply, Err(zbus::Error::MethodError(name, _, _))
                    if matches!(name.as_str(),
                        "org.bluez.Error.InProgress" | "org.bluez.Error.AlreadyConnected"))
            {
                return self.wait_for_connection(&state_bus, &target).await;
            }
            reply
        };
        reply
            .map_err(bluez_error)?
            .ok_or_else(|| zbus::Error::InvalidReply.into())
    }

    async fn wait_for_connection(
        &self,
        state_bus: &Connection,
        target: &DeviceTarget<'_>,
    ) -> Result<(), ApiError> {
        future::race(
            async {
                loop {
                    let (_, adapter, device) = self.device_target(state_bus, target).await?;
                    if !adapter.powered || !device.paired {
                        return Err(ApiError::Conflict("Bluetooth device state changed".into()));
                    }
                    if device.connected {
                        return Ok(());
                    }
                    async_io::Timer::after(Duration::from_millis(200)).await;
                }
            },
            async {
                async_io::Timer::after(Duration::from_secs(6)).await;
                Err(ApiError::Timeout(
                    "Bluetooth device still did not connect".into(),
                ))
            },
        )
        .await
    }

    async fn bus(&self) -> zbus::Result<Connection> {
        if let Some(connection) = self.system.lock().unwrap().clone() {
            return Ok(connection);
        }
        let connection = system_bus().await?;
        Ok(self
            .system
            .lock()
            .unwrap()
            .get_or_insert(connection)
            .clone())
    }

    async fn read_snapshot(&self) -> zbus::Result<Snapshot> {
        let before_query = self.searches.lock().unwrap().clone();
        let connection = self.bus().await?;
        let owner = owner(&connection).await?;
        let mut snapshot = query(&connection, &owner).await?;
        let mut searches = self.searches.lock().unwrap();
        searches.retain(|(owner, path), stop| {
            let observed = before_query
                .get(&(owner.clone(), path.clone()))
                .is_some_and(|previous| previous.same_channel(stop));
            let powered = owner == &snapshot.owner
                && snapshot
                    .adapters
                    .iter()
                    .any(|adapter| &adapter.path == path && adapter.powered);
            if observed && !powered {
                let _ = stop.try_send(());
                false
            } else {
                true
            }
        });
        for adapter in &mut snapshot.adapters {
            adapter.searching = adapter.powered
                && searches.contains_key(&(snapshot.owner.clone(), adapter.path.clone()));
        }
        Ok(snapshot)
    }

    pub async fn snapshot(&self) -> Snapshot {
        let mut snapshot = match self.read_snapshot().await {
            Ok(snapshot) => snapshot,
            Err(error) => {
                if matches!(error, zbus::Error::InputOutput(_)) {
                    self.system.lock().unwrap().take();
                }
                Snapshot::default()
            }
        };
        snapshot.pairing_enabled = self.pairing_enabled;
        snapshot
    }

    async fn adapter(
        &self,
        bus: &Connection,
        expected_owner: &str,
        path: &str,
    ) -> Result<(OwnedUniqueName, Adapter), ApiError> {
        let owner = owner(bus).await.map_err(bluez_error)?;
        if owner.as_str() != expected_owner {
            return Err(ApiError::Conflict("BlueZ restarted".into()));
        }
        let adapter = query(bus, &owner)
            .await
            .map_err(bluez_error)?
            .adapters
            .into_iter()
            .find(|adapter| adapter.path == path)
            .ok_or_else(|| ApiError::InvalidArgument("unknown Bluetooth adapter".into()))?;
        Ok((owner, adapter))
    }

    pub async fn set_power(
        &self,
        expected_owner: &str,
        path: &str,
        powered: bool,
    ) -> Result<(), ApiError> {
        let bus = self.bus().await.map_err(bluez_error)?;
        let (owner, adapter) = self.adapter(&bus, expected_owner, path).await?;
        if adapter.powered == powered {
            return Ok(());
        }
        let proxy = Proxy::new(
            &bus,
            owner.as_str(),
            path,
            "org.freedesktop.DBus.Properties",
        )
        .await?;
        proxy
            .call_with_flags::<_, _, ()>(
                "Set",
                MethodFlags::NoAutoStart.into(),
                &(ADAPTER, "Powered", Value::from(powered)),
            )
            .await
            .map_err(bluez_error)?
            .ok_or(zbus::Error::InvalidReply)?;
        if !powered {
            self.cancel_scan(expected_owner, path);
        }
        Ok(())
    }

    pub async fn scan(&self, expected_owner: &str, path: &str) -> Result<(), ApiError> {
        // This connection owns only this scan. Closing it releases the lease even if StopDiscovery fails.
        let bus = system_bus().await.map_err(bluez_error)?;
        let (owner, adapter) = self.adapter(&bus, expected_owner, path).await?;
        if !adapter.powered {
            return Err(ApiError::Conflict("Bluetooth adapter is off".into()));
        }
        let key = (owner.to_string(), path.to_owned());
        let (stop, cancelled) = async_channel::bounded(1);
        {
            let mut searches = self.searches.lock().unwrap();
            match searches.entry(key.clone()) {
                Entry::Vacant(entry) => {
                    entry.insert(stop.clone());
                }
                Entry::Occupied(_) => {
                    return Err(ApiError::Conflict(
                        "Bluetooth search is already running".into(),
                    ));
                }
            }
        }
        let lease = SearchLease {
            searches: self.searches.clone(),
            key,
            stop,
        };
        let proxy = Proxy::new_owned(bus.clone(), owner, path.to_owned(), ADAPTER).await?;
        proxy
            .call_with_flags::<_, _, ()>("StartDiscovery", MethodFlags::NoAutoStart.into(), &())
            .await
            .map_err(bluez_error)?
            .ok_or(zbus::Error::InvalidReply)?;
        let executor = bus.executor().clone();
        executor
            .spawn(
                async move {
                    futures_lite::future::race(
                        async {
                            let _ = cancelled.recv().await;
                        },
                        async {
                            async_io::Timer::after(SEARCH_DURATION).await;
                        },
                    )
                    .await;
                    let _ = proxy
                        .call_with_flags::<_, _, ()>(
                            "StopDiscovery",
                            MethodFlags::NoAutoStart.into(),
                            &(),
                        )
                        .await;
                    drop(proxy);
                    let _ = bus.close().await;
                    drop(lease);
                    drop(cancelled);
                },
                "bluetooth discovery",
            )
            .detach();
        Ok(())
    }

    fn cancel_scan(&self, owner: &str, path: &str) -> Option<async_channel::Sender<()>> {
        let stop = self
            .searches
            .lock()
            .unwrap()
            .get(&(owner.into(), path.into()))?
            .clone();
        let _ = stop.try_send(());
        Some(stop)
    }

    pub async fn stop_scan(&self, owner: &str, path: &str) {
        if let Some(stop) = self.cancel_scan(owner, path) {
            stop.closed().await;
        }
    }
}

fn bluez_error(error: zbus::Error) -> ApiError {
    if let zbus::Error::MethodError(name, _, _) = &error {
        match name.as_str().rsplit('.').next() {
            Some("AccessDenied" | "NotAuthorized" | "NotPermitted") => {
                return ApiError::PermissionDenied("BlueZ refused the request".into());
            }
            Some("Blocked") => return ApiError::Failed("Bluetooth adapter is blocked".into()),
            Some("NotReady" | "InProgress") => {
                return ApiError::Conflict("Bluetooth adapter is not ready".into());
            }
            Some("AuthenticationCanceled" | "AuthenticationRejected") => {
                return ApiError::Cancelled("Bluetooth pairing cancelled or rejected".into());
            }
            Some("AuthenticationTimeout") => {
                return ApiError::Timeout("Bluetooth pairing timed out".into());
            }
            _ => (),
        }
    }
    ApiError::ZBus(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::OwnedObjectPath;

    #[test]
    fn devices_are_grouped_by_adapter_and_report_zero_battery() {
        let hci0 = "/org/bluez/hci0";
        let hci1 = "/org/bluez/hci1";
        let dev = "/org/bluez/hci0/dev_01";
        let mut managed = ManagedObjects::new();
        let mut put = |path, interface, properties| {
            managed
                .entry(OwnedObjectPath::try_from(path).unwrap())
                .or_default()
                .insert(
                    zbus::names::OwnedInterfaceName::try_from(interface).unwrap(),
                    properties,
                );
        };
        for adapter in [hci0, hci1] {
            put(
                adapter,
                ADAPTER,
                HashMap::from([("Powered".into(), OwnedValue::from(true))]),
            );
        }
        put(
            dev,
            DEVICE,
            HashMap::from([
                (
                    "Adapter".into(),
                    OwnedValue::try_from(Value::from(ObjectPath::try_from(hci0).unwrap())).unwrap(),
                ),
                ("Paired".into(), OwnedValue::from(true)),
            ]),
        );
        put(
            dev,
            BATTERY,
            HashMap::from([("Percentage".into(), OwnedValue::from(0_u8))]),
        );

        let snapshot = decode(":1.2", &managed);
        assert_eq!(snapshot.adapters.len(), 2);
        assert_eq!(snapshot.adapters[0].devices[0].battery, Some(0));
        assert!(snapshot.adapters[0].devices[0].paired);
        assert!(snapshot.adapters[1].devices.is_empty());
    }
}
