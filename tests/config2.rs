use mosaic_shell_tools::settings::*;
use std::{fs, path::PathBuf};
fn dir() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "mosaic-v2-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&p).unwrap();
    p
}
#[test]
fn legacy_pins_migrate_in_memory_and_backup_precedes_write() {
    let d = dir();
    let p = d.join("config.toml");
    let old = "schema_version=1\nrevision=7\npins=['a.desktop','b.desktop']\n";
    fs::write(&p, old).unwrap();
    let s = load(&p).unwrap();
    assert_eq!(s.schema_version, 2);
    assert_eq!(s.revision, 7);
    assert_eq!(s.pins, vec!["a.desktop", "b.desktop"]);
    assert_eq!(fs::read_to_string(&p).unwrap(), old);
    let next = save(&p, &s).unwrap();
    assert_eq!(next.revision, 8);
    assert_eq!(
        fs::read_to_string(d.join("config.toml.v1.bak")).unwrap(),
        old
    );
    assert_eq!(load(&p).unwrap(), next);
    fs::remove_dir_all(d).unwrap();
}
#[test]
fn typed_patch_keeps_other_fields_and_validates_ranges() {
    let base = Settings::default();
    let next = patched(
        &base,
        "theme.mode='light'\ndock.auto_hide=true\ndock.icon_size=64\ngeneral.reduced_motion=true",
    )
    .unwrap();
    assert_eq!(next.theme.mode, ThemeMode::Light);
    assert!(next.dock.auto_hide);
    assert_eq!(next.dock.icon_size, 64);
    assert!(next.general.reduced_motion);
    assert_eq!(next.pins, base.pins);
    assert_eq!(next.revision, base.revision);
    for bad in [
        "theme.mode='bogus'",
        "dock.icon_size=31",
        "dock.icon_size=81",
        "dock.hide_delay_ms=99",
        "dock.show_delay_ms=2001",
        "dock.unrecognized=true",
        "schema_version=9",
        "revision=9",
    ] {
        assert!(patched(&base, bad).is_err(), "{bad}");
    }
}

#[test]
fn dock_hover_keeps_legacy_defaults_and_validates_tuning() {
    let old = toml::from_str::<Settings>(
        "schema_version=2\nrevision=2\npins=[]\n[dock]\nmagnification=true",
    )
    .unwrap();
    assert_eq!(old.dock.hover, DockHover::default());
    let tuned = patched(&old,
        "dock.hover.zoom_percent=70\ndock.hover.radius_percent=175\ndock.hover.background_percent=25\ndock.hover.grow_ms=0\ndock.hover.shrink_ms=400\ndock.hover.easing='linear'\ndock.hover.cards_enabled=false"
    ).unwrap();
    assert_eq!(tuned.dock.hover.zoom_percent, 70);
    assert_eq!(tuned.dock.hover.background_percent, 25);
    assert_eq!(tuned.dock.hover.grow_ms, 0);
    assert!(!tuned.dock.hover.cards_enabled);
    assert_eq!(tuned.dock.hover.easing, HoverEasing::Linear);
    for invalid in [
        "dock.hover.zoom_percent=71",
        "dock.hover.radius_percent=24",
        "dock.hover.background_percent=26",
        "dock.hover.shrink_ms=1001",
        "dock.hover.cards_show_delay_ms=2001",
        "dock.hover.easing='bounce'",
    ] {
        assert!(patched(&old, invalid).is_err(), "{invalid}");
    }
}
#[test]
fn invalid_legacy_fields_do_not_disappear_during_migration() {
    let d = dir();
    let p = d.join("config.toml");
    fs::write(&p, "schema_version=1\nrevision=0\npins=[]\nunknown=true").unwrap();
    assert!(load(&p).is_err());
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn dock_on_every_output_is_opt_in() {
    let base = Settings::default();
    assert!(!base.dock.all_outputs);
    let next = patched(&base, "dock.all_outputs=true\ndock.preferred_output='DP-2'").unwrap();
    assert!(next.dock.all_outputs);
    assert_eq!(next.dock.preferred_output, "DP-2");
    assert_eq!(next.pins, base.pins);
    assert!(patched(&base, "dock.all_outputs='yes'").is_err());
}

#[test]
fn dock_edge_defaults_to_bottom_and_accepts_only_supported_edges() {
    let base = Settings::default();
    let edge = |settings: &Settings| {
        toml::Value::try_from(settings).unwrap()["dock"]["edge"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(edge(&base), "bottom");
    for name in ["left", "right", "bottom"] {
        let next = patched(&base, &format!("dock.edge='{name}'")).unwrap();
        assert_eq!(edge(&next), name);
        assert_eq!(next.pins, base.pins);
    }
    assert!(patched(&base, "dock.edge='top'").is_err());
    assert!(patched(&base, "dock.edge=2").is_err());
}

#[test]
fn panel_order_and_visibility_are_optional_and_persisted() {
    let d = dir();
    let p = d.join("config.toml");
    fs::write(&p, "schema_version=2\nrevision=0\npins=['a.desktop']").unwrap();
    let base = load(&p).unwrap();
    let serialized = toml::Value::try_from(&base).unwrap();
    let output = serialized["panel"]["order"].as_array().unwrap();
    let ids: Vec<_> = output.iter().map(|item| item.as_str().unwrap()).collect();
    assert_eq!(
        ids,
        [
            "menu", "active", "status", "spacer", "tray", "clock", "settings"
        ]
    );
    assert_eq!(serialized["panel"]["hidden"].as_array().unwrap().len(), 0);
    let next = patched(
        &base,
        "panel.order=['tray','menu','active','status','spacer','clock','settings']\npanel.hidden=['active','status']",
    ).unwrap();
    let saved = save(&p, &next).unwrap();
    assert_eq!(load(&p).unwrap(), saved);
    assert_eq!(saved.pins, base.pins);
    let serialized = toml::Value::try_from(&saved).unwrap();
    assert_eq!(serialized["panel"]["order"][0].as_str(), Some("tray"));
    assert_eq!(serialized["panel"]["hidden"][1].as_str(), Some("status"));
    for invalid in [
        "panel.order=['menu','active','status','spacer','tray','clock','clock']",
        "panel.order=['menu','active','status','spacer','tray','clock']",
        "panel.hidden=['clock']",
        "panel.hidden=['tray','tray']",
        "panel.hidden=['bogus']",
    ] {
        assert!(patched(&base, invalid).is_err(), "{invalid}");
    }
    fs::remove_dir_all(d).unwrap();
}

#[test]
fn panel_shortcuts_preserve_legacy_order_and_round_trip_independently() {
    let d = dir();
    let path = d.join("config.toml");
    fs::write(
        &path,
        "schema_version=2\nrevision=4\npins=[]\n[panel]\norder=['menu','tray','spacer','active','status','clock','settings']\nhidden=['active']",
    )
    .unwrap();
    let legacy = load(&path).unwrap();
    assert!(legacy.panel.is_visible(PanelItem::Tray));
    assert!(!legacy.panel.is_visible(PanelItem::Active));
    assert!(!legacy.panel.is_visible(PanelItem::Wifi));
    let next = patched(
        &legacy,
        "panel.order=['wifi','menu','tray','spacer','active','status','bluetooth','brightness','audio','appearance','dock','clock','settings']\npanel.hidden=['active','audio']",
    )
    .unwrap();
    let saved = save(&path, &next).unwrap();
    assert_eq!(load(&path).unwrap(), saved);
    assert!(saved.panel.is_visible(PanelItem::Wifi));
    assert!(!saved.panel.is_visible(PanelItem::Audio));
    let serialized = toml::Value::try_from(&saved).unwrap();
    assert_eq!(serialized["panel"]["order"][0].as_str(), Some("wifi"));
    assert_eq!(serialized["panel"]["order"][10].as_str(), Some("dock"));
    assert_eq!(serialized["panel"]["hidden"][1].as_str(), Some("audio"));

    assert!(patched(
        &legacy,
        "panel.order=['menu','tray','spacer','active','status','clock','settings','wifi','wifi']",
    )
    .is_err());
    fs::remove_dir_all(d).unwrap();
}
