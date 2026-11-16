use shell_core::{search_score, Config, ConfigMode, Window, WindowEvent, WindowStore};

#[test]
fn config_defaults_and_bounds() {
    let c = Config::parse("").unwrap();
    assert_eq!(c.dock.icon_size, 48);
    assert_eq!(c.panel.height, 32);
    assert!(Config::parse("[dock]\nicon_size=31").is_err());
    assert!(Config::parse("[dock]\nicon_size=81").is_err());
    assert!(Config::parse("[dock]\nunknown=true").is_err());
    assert!(Config::parse("schema_version=2").is_err());
}

#[test]
fn strict_rejects_overrides_and_defaults_preserves_base() {
    let base = Config::parse("mode='strict'\n[dock]\nicon_size=64").unwrap();
    assert_eq!(base.mode, ConfigMode::Strict);
    assert!(base.with_override("[dock]\nicon_size=40").is_err());
    let base = Config::parse("[dock]\nicon_size=64\npinned=['firefox.desktop']").unwrap();
    let effective = base.with_override("[dock]\nicon_size=40").unwrap();
    assert_eq!(effective.dock.icon_size, 40);
    assert_eq!(effective.dock.pinned, ["firefox.desktop"]);
    assert_eq!(base.dock.icon_size, 64);
}

#[test]
fn stale_window_commands_cannot_target_new_connection() {
    let mut store = WindowStore::default();
    store.apply(WindowEvent::Reset(8));
    store.apply(WindowEvent::Upsert(Window {
        epoch: 8,
        id: 4,
        app_id: "firefox".into(),
        title: "Private".into(),
        ..Window::default()
    }));
    assert!(store.contains(8, 4));
    store.apply(WindowEvent::Reset(9));
    store.apply(WindowEvent::Upsert(Window {
        epoch: 8,
        id: 4,
        ..Window::default()
    }));
    assert!(!store.contains(8, 4));
    assert!(store.snapshot().is_empty());
}

#[test]
fn closed_windows_disappear_and_snapshot_order_is_stable() {
    let mut s = WindowStore::default();
    for id in [4, 1] {
        s.apply(WindowEvent::Upsert(Window {
            id,
            ..Window::default()
        }));
    }
    assert_eq!(
        s.snapshot().iter().map(|w| w.id).collect::<Vec<_>>(),
        [1, 4]
    );
    s.apply(WindowEvent::Closed { epoch: 0, id: 1 });
    assert!(!s.contains(0, 1));
}

#[test]
fn search_ranks_exact_above_prefix_above_subsequence_and_handles_unicode() {
    assert!(search_score("fire", "fire") > search_score("fire", "firefox"));
    assert!(search_score("fire", "firefox") > search_score("fire", "file reader"));
    assert!(search_score("терм", "Терминал").is_some());
    assert!(search_score("xyz", "Firefox").is_none());
}

#[test]
fn gui_changes_do_not_freeze_untouched_nix_defaults() {
    let base = Config::parse("[dock]\nicon_size=64").unwrap();
    let mut next = base.clone();
    next.theme = "light".into();
    let patch = base.changed_override(&next, "").unwrap();
    assert!(!patch.contains("icon_size"));
    assert!(patch.contains("light"));
    let changed_defaults = Config::parse("[dock]\nicon_size=72").unwrap();
    assert_eq!(
        changed_defaults
            .with_override(&patch)
            .unwrap()
            .dock
            .icon_size,
        72
    );
}
