use mosaic_shell_tools::desktop::*;

#[cfg(feature = "desktop")]
#[test]
fn trash_status_distinguishes_empty_directory_from_error() {
    use gtk::{gio, glib};
    use mosaic_shell_tools::trash::{TrashState, probe};

    let temp = std::env::temp_dir().join(format!("mosaic-trash-{}", std::process::id()));
    std::fs::create_dir(&temp).unwrap();
    let root = gio::File::for_path(&temp);
    let context = glib::MainContext::new();
    assert_eq!(context.block_on(probe(&root)), TrashState::Empty);
    let item = temp.join("sample");
    std::fs::write(&item, b"test").unwrap();
    assert_eq!(context.block_on(probe(&root)), TrashState::Full);
    std::fs::remove_file(item).unwrap();
    std::fs::remove_dir(temp).unwrap();
    assert_eq!(context.block_on(probe(&root)), TrashState::Unavailable);
}
fn app(id: &str, wm: Option<&str>) -> Application {
    Application {
        id: id.into(),
        wm_class: wm.map(str::to_owned),
    }
}
#[test]
fn desktop_identity_wins_over_wm_class() {
    let apps = vec![
        app("org.test.App.desktop", None),
        app("other.desktop", Some("org.test.App")),
    ];
    assert_eq!(
        match_app("org.test.App", &apps),
        Some("org.test.App.desktop".into())
    );
}
#[test]
fn ambiguous_wmclass_is_not_guessed() {
    let apps = vec![
        app("one.desktop", Some("shared")),
        app("two.desktop", Some("shared")),
    ];
    assert_eq!(match_app("shared", &apps), None);
    assert_eq!(match_app("unknown", &apps), None);
}
#[test]
fn legacy_thunar_pin_and_mixed_case_window_resolve_to_the_installed_launcher() {
    let apps = [app("thunar.desktop", None)];
    for id in ["org.xfce.Thunar.desktop", "org.xfce.thunar", "Thunar"] {
        assert_eq!(match_app(id, &apps).as_deref(), Some("thunar.desktop"));
    }
}
#[test]
fn pins_are_stable_and_running_is_deduplicated() {
    assert_eq!(
        dock_order(
            &["b".into(), "a".into(), "b".into()],
            &["a".into(), "c".into(), "c".into()]
        ),
        vec!["b", "a", "c"]
    );
}

#[test]
fn dock_scope_requires_confirmed_membership_only_when_enabled() {
    let mut window = Window {
        epoch: 1,
        id: 1,
        app_id: "editor.desktop".into(),
        title: String::new(),
        active: false,
        minimized: true,
        fullscreen: false,
        outputs: vec![],
    };
    assert!(dock_window_visible(&window, None));
    assert!(!dock_window_visible(&window, Some("")));
    assert!(!dock_window_visible(&window, Some("DP-1")));
    window.outputs = vec!["DP-2".into(), "DP-1".into()];
    assert!(dock_window_visible(&window, Some("DP-1")));
    assert!(dock_window_visible(&window, Some("DP-2")));
    assert!(!dock_window_visible(&window, Some("DP-3")));
}

#[test]
fn moving_an_existing_window_does_not_confirm_a_new_launch() {
    let old = Window {
        epoch: 1,
        id: 7,
        app_id: "editor.desktop".into(),
        title: "Old".into(),
        active: false,
        minimized: false,
        fullscreen: false,
        outputs: vec!["DP-1".into()],
    };
    let mut moved = old.clone();
    moved.outputs = vec!["DP-2".into()];
    moved.active = true;
    let mut opened = moved.clone();
    opened.id += 1;
    let current = [moved, opened];
    let new = newly_observed_windows(std::slice::from_ref(&old), &current);
    assert_eq!(new, vec![&current[1]]);
}

#[test]
fn a_window_can_confirm_launch_when_its_app_id_arrives_later() {
    let old = Window {
        epoch: 1,
        id: 7,
        app_id: String::new(),
        title: String::new(),
        active: false,
        minimized: false,
        fullscreen: false,
        outputs: vec![],
    };
    let mut identified = old.clone();
    identified.app_id = "editor".into();
    let current = [identified];
    let applications = [Application {
        id: "editor.desktop".into(),
        wm_class: None,
    }];
    let new = newly_observed_windows(&[old], &current);
    assert_eq!(new.len(), 1);
    assert_eq!(
        match_app(&new[0].app_id, &applications).as_deref(),
        Some("editor.desktop")
    );
}

#[test]
fn borderless_app_rules_hide_shell_surfaces_only_where_the_window_is_active() {
    let mut window = Window {
        epoch: 1,
        id: 2,
        app_id: "steam_app_123".into(),
        title: "Game".into(),
        active: true,
        minimized: false,
        fullscreen: false,
        outputs: vec!["DP-1".into()],
    };
    let rules = vec!["steam_app_123".into()];
    let hidden = |w: &Window, apps: &[String], output, single| {
        shell_surfaces_hidden(std::slice::from_ref(w), apps, output, single)
    };
    assert!(hidden(&window, &rules, "DP-1", false));
    assert!(!hidden(&window, &rules, "DP-2", false));
    assert!(!hidden(&window, &[], "DP-1", false));
    assert!(!hidden(&window, &["steam_app_12".into()], "DP-1", false));

    window.minimized = true;
    assert!(!hidden(&window, &rules, "DP-1", false));
    window.minimized = false;
    window.active = false;
    assert!(!hidden(&window, &rules, "DP-1", false));

    window.active = true;
    window.fullscreen = true;
    assert!(hidden(&window, &[], "DP-1", false));
    window.fullscreen = false;
    window.outputs.clear();
    assert!(!hidden(&window, &rules, "DP-1", false));
    assert!(hidden(&window, &rules, "DP-1", true));
}

#[test]
fn closed_or_old_epoch_handle_is_invalid() {
    let w = Window {
        epoch: 2,
        id: 7,
        app_id: "a".into(),
        title: String::new(),
        active: false,
        minimized: false,
        fullscreen: false,
        outputs: vec![],
    };
    assert!(valid_window(std::slice::from_ref(&w), 2, 7));
    assert!(!valid_window(std::slice::from_ref(&w), 1, 7));
    assert!(!valid_window(&[], 2, 7));
}

#[cfg(feature = "desktop")]
#[test]
fn dropped_desktop_files_use_catalog_identity() {
    use std::{fs, os::unix::fs::symlink};
    let root = std::env::temp_dir().join(format!("mosaic-desktop-drop-{}", std::process::id()));
    fs::create_dir(&root).unwrap();
    for name in ["catalog", "alias", "unknown"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let installed = root.join("catalog/editor.desktop");
    fs::write(
        &installed,
        "[Desktop Entry]\nType=Application\nName=Editor\nExec=true\n",
    )
    .unwrap();
    let alias = root.join("alias/editor.desktop");
    symlink(&installed, &alias).unwrap();
    let unknown = root.join("unknown/editor.desktop");
    fs::copy(&installed, &unknown).unwrap();
    let entries = [("tools-editor.desktop".into(), installed.clone())];
    let context = gtk::glib::MainContext::new();
    let resolve = |path: &std::path::Path| context.block_on(resolve_desktop_drop(path, &entries));
    assert_eq!(resolve(&installed), Some("tools-editor.desktop".into()));
    assert_eq!(resolve(&alias), Some("tools-editor.desktop".into()));
    assert_eq!(resolve(&unknown), None);
    assert_eq!(resolve(&root.join("absent.desktop")), None);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn drop_accepts_one_local_uri_with_comments_and_crlf() {
    assert_eq!(
        dropped_desktop_uri(b"# comment\r\nfile:///tmp/a%20b.desktop\r\n"),
        Some("file:///tmp/a%20b.desktop")
    );
    for value in [
        &b""[..],
        b"file:///tmp/a.desktop\nfile:///tmp/b.desktop",
        b"https://example.com/a.desktop",
        b"file://remote/tmp/a.desktop",
        b"file:///tmp/x\0",
    ] {
        assert_eq!(dropped_desktop_uri(value), None);
    }
}
