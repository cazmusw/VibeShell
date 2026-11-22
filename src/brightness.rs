use crate::ipc::ApiError;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use zbus::{
    Connection, Proxy,
    fdo::DBusProxy,
    names::OwnedUniqueName,
    proxy::MethodFlags,
    zvariant::{OwnedObjectPath, OwnedValue},
};

const SYSFS: &str = "/sys/class/backlight";
const LOGIN: &str = "org.freedesktop.login1";
const MANAGER: &str = "/org/freedesktop/login1";
const AUTO: &str = "/org/freedesktop/login1/session/auto";
const SESSION: &str = "org.freedesktop.login1.Session";
const BUS_TIMEOUT: Duration = Duration::from_secs(2);

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    Ready,
    NoDevice,
    NoSession,
    #[default]
    NoService,
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct Snapshot {
    pub access: Access,
    pub owner: String,
    pub session: String,
    pub devices: Vec<Device>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Device {
    pub name: String,
    pub identity: String,
    pub level: u32,
    pub max: u32,
    pub percent: u32,
}

pub struct Service {
    root: PathBuf,
    bus: Mutex<Option<Connection>>,
    writing: AtomicBool,
}

struct WriteGuard<'a>(&'a AtomicBool);

impl Drop for WriteGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

impl Default for Service {
    fn default() -> Self {
        // The fixture substitutes a class directory only with an explicitly supplied
        // system-bus address. Production writes always go through logind.
        let root = if std::env::var_os("DBUS_SYSTEM_BUS_ADDRESS").is_some() {
            std::env::var_os("MOSAIC_BRIGHTNESS_FIXTURE_ROOT")
                .map(PathBuf::from)
                .filter(|path| path.is_absolute())
        } else {
            None
        }
        .unwrap_or_else(|| PathBuf::from(SYSFS));
        Self {
            root,
            bus: Mutex::new(None),
            writing: AtomicBool::new(false),
        }
    }
}

impl Service {
    async fn connection(&self) -> zbus::Result<Connection> {
        if let Some(bus) = self.bus.lock().unwrap().clone() {
            return Ok(bus);
        }
        let bus = zbus::connection::Builder::system()?
            .method_timeout(BUS_TIMEOUT)
            .build()
            .await?;
        Ok(self.bus.lock().unwrap().get_or_insert(bus).clone())
    }

    async fn login(&self) -> zbus::Result<(Connection, OwnedUniqueName)> {
        let bus = self.connection().await?;
        let owner = match DBusProxy::new(&bus)
            .await?
            .get_name_owner(LOGIN.try_into()?)
            .await
        {
            Ok(owner) => owner,
            Err(error) => {
                let error = zbus::Error::from(error);
                if matches!(error, zbus::Error::InputOutput(_)) {
                    self.bus.lock().unwrap().take();
                }
                return Err(error);
            }
        };
        Ok((bus, owner))
    }

    pub async fn snapshot(&self) -> Snapshot {
        let devices = devices(&self.root);
        if devices.is_empty() {
            return Snapshot {
                access: Access::NoDevice,
                ..Snapshot::default()
            };
        }
        let mut state = Snapshot {
            devices,
            ..Snapshot::default()
        };
        if let Ok((bus, owner)) = self.login().await {
            state.owner = owner.to_string();
            if let Ok(Some(session)) = active_session(&bus, &owner).await {
                state.access = Access::Ready;
                state.session = session;
            } else {
                state.access = Access::NoSession;
            }
        }
        state
    }

    pub async fn set(
        &self,
        expected_owner: &str,
        expected_session: &str,
        name: &str,
        identity: &str,
        max: u32,
        value: u32,
    ) -> Result<(), ApiError> {
        if !valid_name(name)
            || identity.is_empty()
            || identity.len() > 64
            || !(1..=100).contains(&value)
            || max == 0
            || expected_session.is_empty()
            || expected_session.len() > 128
        {
            return Err(ApiError::InvalidArgument(
                "invalid backlight request".into(),
            ));
        }
        self.writing
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .map_err(|_| ApiError::Conflict("backlight adjustment is already pending".into()))?;
        let _guard = WriteGuard(&self.writing);
        let (bus, owner) = self.login().await.map_err(login_error)?;
        if owner.as_str() != expected_owner {
            return Err(ApiError::Conflict("logind restarted".into()));
        }
        let current = active_session(&bus, &owner).await.map_err(login_error)?;
        let session =
            current.ok_or_else(|| ApiError::PermissionDenied("no active local session".into()))?;
        if session != expected_session {
            return Err(ApiError::Conflict("login session changed".into()));
        }
        let manager = Proxy::new(
            &bus,
            owner.as_str(),
            MANAGER,
            "org.freedesktop.login1.Manager",
        )
        .await?;
        let path: OwnedObjectPath = manager
            .call_with_flags("GetSession", MethodFlags::NoAutoStart.into(), &(&session,))
            .await
            .map_err(login_error)?
            .ok_or(zbus::Error::InvalidReply)?;
        read_device(&self.root, name)
            .filter(|device| device.max == max && device.identity == identity)
            .ok_or_else(|| ApiError::Conflict("backlight device changed".into()))?;
        let level = level_for_percent(value, max);
        let proxy = Proxy::new(&bus, owner.as_str(), path.as_str(), SESSION).await?;
        proxy
            .call_with_flags::<_, _, ()>(
                "SetBrightness",
                MethodFlags::NoAutoStart.into(),
                &("backlight", name, level),
            )
            .await
            .map_err(login_error)?
            .ok_or_else(|| zbus::Error::InvalidReply.into())
    }
}

async fn active_session(bus: &Connection, owner: &OwnedUniqueName) -> zbus::Result<Option<String>> {
    let proxy = Proxy::new(bus, owner.as_str(), AUTO, "org.freedesktop.DBus.Properties").await?;
    let props = proxy
        .call_with_flags::<_, _, HashMap<String, OwnedValue>>(
            "GetAll",
            MethodFlags::NoAutoStart.into(),
            &(SESSION,),
        )
        .await?
        .ok_or(zbus::Error::InvalidReply)?;
    let active = props
        .get("Active")
        .and_then(|value| bool::try_from(value).ok())
        == Some(true);
    let local = props
        .get("Remote")
        .and_then(|value| bool::try_from(value).ok())
        == Some(false);
    let seat = props
        .get("Seat")
        .and_then(|value| value.try_clone().ok())
        .and_then(|value| <(String, OwnedObjectPath)>::try_from(value).ok());
    if active && local && seat.is_some_and(|(name, _)| !name.is_empty()) {
        Ok(props
            .get("Id")
            .and_then(|value| <&str>::try_from(value).ok())
            .filter(|id| !id.is_empty())
            .map(str::to_owned))
    } else {
        Ok(None)
    }
}

fn devices(root: &Path) -> Vec<Device> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut devices: Vec<_> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str().filter(|name| valid_name(name))?;
            let device = read_device(root, name)?;
            let kind = fs::read_to_string(entry.path().join("type")).unwrap_or_default();
            let rank = match kind.trim() {
                "raw" => 0,
                "platform" => 1,
                "firmware" => 2,
                _ => 3,
            };
            Some((rank, device))
        })
        .collect();
    devices.sort_by(|(a, left), (b, right)| a.cmp(b).then_with(|| left.name.cmp(&right.name)));
    devices.into_iter().map(|(_, device)| device).collect()
}

fn read_device(root: &Path, name: &str) -> Option<Device> {
    let dir = root.join(name);
    let meta = dir.metadata().ok()?;
    let read = |attribute| {
        fs::read_to_string(dir.join(attribute))
            .ok()?
            .trim()
            .parse::<u32>()
            .ok()
    };
    let max = read("max_brightness")?;
    let level = read("actual_brightness")
        .filter(|&level| level <= max)
        .or_else(|| read("brightness"))?;
    if max == 0 || level > max {
        return None;
    }
    Some(Device {
        name: name.into(),
        identity: format!("{}:{}", meta.dev(), meta.ino()),
        level,
        max,
        percent: percent(level, max),
    })
}

fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 255
        && !matches!(name, "." | "..")
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.' | b':'))
}

fn percent(level: u32, max: u32) -> u32 {
    ((u64::from(level) * 100 + u64::from(max) / 2) / u64::from(max)) as u32
}

fn level_for_percent(value: u32, max: u32) -> u32 {
    ((u64::from(value) * u64::from(max) + 50) / 100).clamp(1, u64::from(max)) as u32
}

fn login_error(error: zbus::Error) -> ApiError {
    if let zbus::Error::MethodError(name, _, _) = &error {
        match name.as_str().rsplit('.').next() {
            Some("AccessDenied" | "NotYourDevice" | "AuthFailed") => {
                return ApiError::PermissionDenied("logind denied brightness adjustment".into());
            }
            Some("NoSessionForPID" | "NoSessionForUID" | "NoSuchSession" | "UnknownObject") => {
                return ApiError::Conflict("login session changed".into());
            }
            _ => (),
        }
    }
    ApiError::ZBus(error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_scales_and_preserves_external_zero() {
        assert_eq!(percent(0, 17), 0);
        assert_eq!(percent(9, 17), 53);
        assert_eq!(percent(u32::MAX, u32::MAX), 100);
        assert_eq!(level_for_percent(1, 17), 1);
        assert_eq!(level_for_percent(50, 17), 9);
        assert_eq!(level_for_percent(100, u32::MAX), u32::MAX);
    }

    #[test]
    fn accepts_class_filenames_without_path_traversal() {
        for name in ["intel_backlight", "acpi_video0", "panel0-backlight"] {
            assert!(valid_name(name));
        }
        for name in ["", ".", "..", "../device", "/dev/backlight", "alpha\n"] {
            assert!(!valid_name(name));
        }
    }
}
