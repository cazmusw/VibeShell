use super::{MethodFlags, NetworkService, OwnedObjectPath, Properties, Proxy, property};
use crate::ipc::ApiError;
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, time::Duration};

const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings";
const PROFILE_INTERFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const WIFI_SETTING: &str = "802-11-wireless";
const MAX_PROFILES: usize = 128;
type Settings = HashMap<String, Properties>;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct WifiProfile {
    pub path: String,
    pub uuid: String,
    pub name: String,
    pub ssid: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct WifiProfiles {
    pub owner: String,
    pub profiles: Vec<WifiProfile>,
    pub truncated: bool,
}

fn decode_profile(path: &str, settings: &Settings) -> Option<WifiProfile> {
    let connection = settings.get("connection")?;
    if property::<&str>(connection, "type")? != WIFI_SETTING {
        return None;
    }
    let ssid = settings.get(WIFI_SETTING).and_then(|wifi| wifi.get("ssid"));
    let ssid = ssid
        .and_then(|value| Vec::<u8>::try_from(value.try_clone().ok()?).ok())
        .filter(|bytes| !bytes.is_empty())
        .map(|bytes| super::ssid_label(&bytes))
        .unwrap_or_default();
    Some(WifiProfile {
        path: path.into(),
        uuid: property::<&str>(connection, "uuid")?.into(),
        name: super::ssid_label(property::<&str>(connection, "id")?.as_bytes()),
        ssid,
    })
}

async fn read_profile(proxy: &Proxy<'_>) -> Result<Option<WifiProfile>, ApiError> {
    let settings = proxy
        .call_with_flags::<_, _, Settings>("GetSettings", MethodFlags::NoAutoStart.into(), &())
        .await
        .map_err(super::network_error)?
        .ok_or(zbus::Error::InvalidReply)?;
    Ok(decode_profile(proxy.path().as_str(), &settings))
}

impl NetworkService {
    pub async fn wifi_profiles(&self) -> Result<WifiProfiles, ApiError> {
        futures_lite::future::race(self.read_wifi_profiles(), async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            Err(ApiError::Failed(
                "NetworkManager profile query timed out".into(),
            ))
        })
        .await
    }

    async fn read_wifi_profiles(&self) -> Result<WifiProfiles, ApiError> {
        let (connection, owner) = self.network_manager().await?;
        let settings = Proxy::new(
            &connection,
            owner.as_str(),
            SETTINGS_PATH,
            SETTINGS_INTERFACE,
        )
        .await?;
        let paths = settings
            .call_with_flags::<_, _, Vec<OwnedObjectPath>>(
                "ListConnections",
                MethodFlags::NoAutoStart.into(),
                &(),
            )
            .await
            .map_err(super::network_error)?
            .ok_or(zbus::Error::InvalidReply)?;
        let mut profiles = Vec::new();
        let mut truncated = false;
        for path in paths {
            let proxy = Proxy::new(
                &connection,
                owner.as_str(),
                path.as_str(),
                PROFILE_INTERFACE,
            )
            .await?;
            match read_profile(&proxy).await {
                Ok(Some(profile)) if profiles.len() < MAX_PROFILES => profiles.push(profile),
                Ok(Some(_)) => {
                    truncated = true;
                    break;
                }
                Ok(None) | Err(ApiError::PermissionDenied(_) | ApiError::NoSavedProfile(_)) => (),
                Err(ApiError::ZBus(zbus::Error::MethodError(name, _, _)))
                    if name.as_str() == "org.freedesktop.DBus.Error.UnknownObject" => {}
                Err(error) => return Err(error),
            }
        }
        profiles.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.uuid.cmp(&b.uuid)));
        Ok(WifiProfiles {
            owner: owner.to_string(),
            profiles,
            truncated,
        })
    }

    pub async fn forget_wifi(
        &self,
        expected_owner: &str,
        expected: WifiProfile,
    ) -> Result<(), ApiError> {
        let leaf = expected
            .path
            .strip_prefix(SETTINGS_PATH)
            .and_then(|tail| tail.strip_prefix('/'))
            .filter(|leaf| {
                !leaf.is_empty() && leaf.len() <= 20 && leaf.bytes().all(|b| b.is_ascii_digit())
            });
        if leaf.is_none() {
            return Err(ApiError::InvalidArgument(
                "invalid Wi-Fi profile object path".into(),
            ));
        }
        let _guard = self.lock_path(&expected.path)?;
        let (connection, owner) = self.network_manager().await?;
        if owner.as_str() != expected_owner {
            return Err(ApiError::Conflict("NetworkManager restarted".into()));
        }
        let proxy = Proxy::new(
            &connection,
            owner.as_str(),
            expected.path.as_str(),
            PROFILE_INTERFACE,
        )
        .await?;
        let current = futures_lite::future::race(read_profile(&proxy), async {
            async_io::Timer::after(Duration::from_secs(5)).await;
            Err(ApiError::Failed(
                "NetworkManager profile query timed out".into(),
            ))
        })
        .await?;
        if current.as_ref() != Some(&expected) {
            return Err(ApiError::Conflict("Wi-Fi profile changed".into()));
        }
        proxy
            .call_with_flags::<_, _, ()>(
                "Delete",
                MethodFlags::NoAutoStart | MethodFlags::AllowInteractiveAuth,
                &(),
            )
            .await
            .map_err(super::network_error)?
            .ok_or_else(|| zbus::Error::InvalidReply.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{OwnedValue, Value};

    #[test]
    fn only_wifi_profile_metadata_is_returned() {
        let path = "/org/freedesktop/NetworkManager/Settings/42";
        let uuid = "f183029a-6344-4d15-95fb-bf51a5c32f31";
        let connection = Properties::from([
            ("id".into(), setting_value("Location\u{202e}")),
            ("uuid".into(), setting_value(uuid)),
            ("type".into(), setting_value(WIFI_SETTING)),
        ]);
        let wifi = Properties::from([("ssid".into(), setting_value(vec![b'A', 255_u8]))]);
        let security = Properties::from([("psk".into(), setting_value("fixture-secret"))]);
        let mut settings = Settings::from([
            ("connection".into(), connection),
            (WIFI_SETTING.into(), wifi),
            ("802-11-wireless-security".into(), security),
        ]);

        let profile = decode_profile(path, &settings).unwrap();
        assert_eq!(profile.name, "Location\\u{202e}");
        assert_eq!(profile.ssid, "A\\xff");
        assert_eq!(profile.uuid, uuid);
        assert!(
            !toml::to_string(&profile)
                .unwrap()
                .contains("fixture-secret")
        );

        settings
            .get_mut("connection")
            .unwrap()
            .insert("type".into(), setting_value("802-3-ethernet"));
        assert!(decode_profile(path, &settings).is_none());
    }

    fn setting_value<'a>(value: impl Into<Value<'a>>) -> OwnedValue {
        OwnedValue::try_from(value.into()).unwrap()
    }
}
