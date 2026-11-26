#![cfg(feature = "desktop")]

use mosaic_shell_tools::network::{Protection, WifiNetwork, group_networks, ssid_label};

#[test]
fn wifi_groups_repeat_access_points_without_hiding_current_connection() {
    let ap = |ssid: &[u8], protection, strength, connected| WifiNetwork {
        ssid: ssid.to_vec(),
        name: ssid_label(ssid),
        protection,
        strength,
        connected,
        access_point: format!("/org/freedesktop/NetworkManager/AccessPoint/{strength}"),
        personal: None,
    };
    let groups = group_networks([
        ap(b"Cafe", Protection::Secured, 92, false),
        ap(b"Cafe", Protection::Secured, 26, true),
        ap(b"Cafe", Protection::Open, 73, false),
        ap(b"Home", Protection::Secured, 58, false),
        ap(b"", Protection::Open, 88, false),
    ]);
    assert_eq!(groups.len(), 3);
    assert_eq!(groups[0].strength, 26);
    assert!(groups[0].connected);
    assert!(groups[0].access_point.ends_with("/26"));
    assert_eq!(groups[1].protection, Protection::Open);
    assert_eq!(groups[2].name, "Home");
}

#[test]
fn radio_network_labels_escape_control_and_directional_characters() {
    assert_eq!(ssid_label("Сеть\n\u{202e}".as_bytes()), "Сеть\\n\\u{202e}");
    assert!(!ssid_label(b"\xfftest").contains('\u{fffd}'));
}

#[test]
fn new_wifi_profile_accepts_only_rsn_personal_modes() {
    use mosaic_shell_tools::network::{PersonalMode, personal_mode};
    assert_eq!(personal_mode(0x100 | 0x08), Some(PersonalMode::WpaPsk));
    assert_eq!(personal_mode(0x400 | 0x08), Some(PersonalMode::Sae));
    assert_eq!(personal_mode(0x500 | 0x08), Some(PersonalMode::WpaPsk));
    for unsupported in [0, 0x200, 0x800, 0x1000, 0x2000] {
        assert_eq!(personal_mode(unsupported), None);
    }
}

#[test]
fn ethernet_snapshot_works_without_wifi_and_ignores_virtual_device_types() {
    use mosaic_shell_tools::network::{EthernetLink, decode_snapshot};
    use zbus::{
        fdo::ManagedObjects,
        zvariant::{OwnedObjectPath, OwnedValue, Value},
    };

    let path = |id: &str| OwnedObjectPath::try_from(id).unwrap();
    let physical = path("/org/freedesktop/NetworkManager/Devices/2");
    let virtual_link = path("/org/freedesktop/NetworkManager/Devices/3");
    let mut objects = ManagedObjects::new();
    objects.insert(
        path("/org/freedesktop/NetworkManager"),
        [(
            "org.freedesktop.NetworkManager".try_into().unwrap(),
            [(
                "Devices".into(),
                OwnedValue::try_from(Value::from(vec![physical.clone(), virtual_link.clone()]))
                    .unwrap(),
            )]
            .into(),
        )]
        .into(),
    );
    for (device, kind, carrier) in [(physical.clone(), 1_u32, true), (virtual_link, 20, true)] {
        objects.insert(
            device,
            [
                (
                    "org.freedesktop.NetworkManager.Device".try_into().unwrap(),
                    [
                        ("DeviceType".into(), OwnedValue::from(kind)),
                        (
                            "Interface".into(),
                            OwnedValue::try_from(Value::from("enp14s0")).unwrap(),
                        ),
                        ("Managed".into(), OwnedValue::from(true)),
                        ("State".into(), OwnedValue::from(100_u32)),
                    ]
                    .into(),
                ),
                (
                    "org.freedesktop.NetworkManager.Device.Wired"
                        .try_into()
                        .unwrap(),
                    [
                        ("Carrier".into(), OwnedValue::from(carrier)),
                        ("Speed".into(), OwnedValue::from(2500_u32)),
                    ]
                    .into(),
                ),
            ]
            .into(),
        );
    }
    let connected = decode_snapshot(&objects);
    assert!(!connected.enabled);
    assert!(connected.adapters.is_empty());
    assert_eq!(connected.ethernet.len(), 1);
    assert_eq!(connected.ethernet[0].name, "enp14s0");
    assert_eq!(connected.ethernet[0].link, EthernetLink::Connected);
    assert_eq!(connected.ethernet[0].speed_mbps, 2500);

    let wired = objects
        .get_mut(&physical)
        .unwrap()
        .iter_mut()
        .find(|(interface, _)| interface.as_str().ends_with(".Wired"))
        .unwrap()
        .1;
    wired.insert("Carrier".into(), OwnedValue::from(false));
    let unplugged = decode_snapshot(&objects);
    assert_eq!(unplugged.ethernet[0].link, EthernetLink::NoCarrier);
    assert_eq!(unplugged.ethernet[0].speed_mbps, 0);
    let round_trip = toml::from_str(&toml::to_string(&connected).unwrap()).unwrap();
    assert_eq!(connected, round_trip);
    assert!(decode_snapshot(&ManagedObjects::new()).ethernet.is_empty());
}
