use super::{Battery, Profile, Profiles, Snapshot};
use crate::ipc::ApiError;
use std::{
    collections::HashMap,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use zbus::{
    Connection, Proxy,
    fdo::DBusProxy,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const UPOWER: &str = "org.freedesktop.UPower";
const UPOWER_PATH: &str = "/org/freedesktop/UPower";
const DEVICE: &str = "org.freedesktop.UPower.Device";
const PROFILE_APIS: [(&str, &str); 2] = [
    (
        "org.freedesktop.UPower.PowerProfiles",
        "/org/freedesktop/UPower/PowerProfiles",
    ),
    ("net.hadess.PowerProfiles", "/net/hadess/PowerProfiles"),
];
type Properties = HashMap<String, OwnedValue>;

#[derive(Default)]
pub struct Service {
    writing: AtomicBool,
}

async fn system() -> zbus::Result<Connection> {
    zbus::connection::Builder::system()?
        .method_timeout(Duration::from_millis(800))
        .build()
        .await
}

async fn properties(
    bus: &Connection,
    name: &str,
    path: &str,
    interface: &str,
) -> Option<(String, Properties)> {
    let proxy = Proxy::new(bus, name, path, "org.freedesktop.DBus.Properties")
        .await
        .ok()?;
    let reply = proxy.call_method("GetAll", &(interface,)).await.ok()?;
    let owner = reply.header().sender()?.as_str().to_owned();
    let fields = reply.body().deserialize().ok()?;
    Some((owner, fields))
}

fn read<'a, T: TryFrom<&'a OwnedValue>>(properties: &'a Properties, name: &str) -> Option<T> {
    T::try_from(properties.get(name)?).ok()
}

async fn battery(bus: &Connection) -> Snapshot {
    let Some((owner, fields)) = properties(bus, UPOWER, UPOWER_PATH, UPOWER).await else {
        return Snapshot::default();
    };
    let mut snapshot = Snapshot {
        upower_available: true,
        on_battery: read(&fields, "OnBattery"),
        ..Snapshot::default()
    };
    let Some(device) = display_device(bus, &owner).await else {
        return snapshot;
    };
    let Some((_, info)) = properties(bus, &owner, device.as_str(), DEVICE).await else {
        return snapshot;
    };
    snapshot.battery_present = read::<bool>(&info, "IsPresent");
    if snapshot.battery_present == Some(true) && read::<u32>(&info, "Type") == Some(2) {
        snapshot.battery = read(&info, "Percentage").and_then(|percent| {
            Battery::from_values(
                percent,
                read(&info, "State").unwrap_or_default(),
                read(&info, "TimeToEmpty").unwrap_or_default(),
                read(&info, "TimeToFull").unwrap_or_default(),
            )
        });
    } else if let Some(kind) = read::<u32>(&info, "Type")
        && kind != 2
    {
        snapshot.battery_present = Some(false);
    }
    snapshot
}

async fn display_device(bus: &Connection, owner: &str) -> Option<OwnedObjectPath> {
    Proxy::new(bus, owner, UPOWER_PATH, UPOWER)
        .await
        .ok()?
        .call("GetDisplayDevice", &())
        .await
        .ok()
}

async fn profile_state(bus: &Connection) -> Option<((&'static str, &'static str), Profiles)> {
    for api @ (name, path) in PROFILE_APIS {
        if let Some((owner, fields)) = properties(bus, name, path, name).await
            && let Some(value) = parse_profiles(owner, &fields)
        {
            return Some((api, value));
        }
    }
    None
}

fn parse_profiles(owner: String, fields: &Properties) -> Option<Profiles> {
    let active = Profile::parse(read(fields, "ActiveProfile")?)?;
    let raw = Vec::<Properties>::try_from(fields.get("Profiles")?.try_clone().ok()?).ok()?;
    let mut offered = Vec::new();
    for row in raw.iter().take(16) {
        if let Some(profile) = read(row, "Profile").and_then(Profile::parse)
            && !offered.contains(&profile)
        {
            offered.push(profile);
        }
    }
    if !offered.contains(&active) {
        return None;
    }
    let degraded =
        read::<&str>(fields, "PerformanceDegraded").is_some_and(|value| !value.is_empty());
    Some(Profiles {
        owner,
        offered,
        active,
        degraded,
    })
}

impl Service {
    pub async fn snapshot(&self) -> Snapshot {
        let Ok(bus) = system().await else {
            return Snapshot::default();
        };
        let (mut snapshot, profiles) =
            futures_lite::future::zip(battery(&bus), profile_state(&bus)).await;
        snapshot.profiles = profiles.map(|(_, profiles)| profiles);
        snapshot
    }

    pub async fn set(&self, owner: &str, expected: &str, target: &str) -> Result<(), ApiError> {
        let (Some(expected), Some(target)) = (Profile::parse(expected), Profile::parse(target))
        else {
            return Err(ApiError::InvalidArgument("unknown power profile".into()));
        };
        if self.writing.swap(true, Ordering::AcqRel) {
            return Err(conflict());
        }
        struct WriteGuard<'a>(&'a AtomicBool);
        impl Drop for WriteGuard<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _guard = WriteGuard(&self.writing);
        let bus = system().await?;
        let ((name, path), current) = profile_state(&bus).await.ok_or_else(conflict)?;
        if current.owner != owner || current.active != expected {
            return Err(conflict());
        }
        if !current.offered.contains(&target) {
            return Err(ApiError::InvalidArgument(
                "power profile is not available".into(),
            ));
        }
        let dbus = DBusProxy::new(&bus).await?;
        if dbus
            .get_name_owner(name.try_into().expect("valid profile bus name"))
            .await
            .map_err(|_| conflict())?
            .as_str()
            != owner
        {
            return Err(conflict());
        }
        let properties = Proxy::new(&bus, owner, path, name).await?;
        properties
            .set_property("ActiveProfile", target.id())
            .await
            .map_err(|error| match error {
                zbus::fdo::Error::AccessDenied(_) | zbus::fdo::Error::AuthFailed(_) => {
                    ApiError::PermissionDenied("power profile change denied".into())
                }
                _ => ApiError::Failed("power profile change failed".into()),
            })?;
        let (_, confirmed) = profile_state(&bus).await.ok_or_else(conflict)?;
        if confirmed.owner != owner || confirmed.active != target {
            return Err(conflict());
        }
        Ok(())
    }
}

fn conflict() -> ApiError {
    ApiError::Conflict("power state changed or is unavailable".into())
}
