use mosaic_shell_tools::power::{Battery, BatteryState, Profile};

#[test]
fn keeps_empty_battery_and_unknown_estimate_distinct() {
    let battery = Battery::from_values(0., 2, 0, 7200).unwrap();
    assert_eq!(battery.percent, 0);
    assert_eq!(battery.state, BatteryState::Discharging);
    assert_eq!(battery.seconds, None);
    assert_eq!(
        Battery::from_values(62.4, 1, 7200, 1200).unwrap().seconds,
        Some(1200)
    );
    assert_eq!(
        Battery::from_values(100., 4, 0, 0).unwrap().state,
        BatteryState::Full
    );
    assert!(Battery::from_values(f64::NAN, 1, 0, 0).is_none());
    assert!(Battery::from_values(150., 2, 0, 0).is_none());
}

#[test]
fn maps_only_supported_profile_ids() {
    for id in ["power-saver", "balanced", "performance"] {
        assert_eq!(Profile::parse(id).unwrap().id(), id);
    }
    assert_eq!(Profile::parse("arbitrary"), None);
}
