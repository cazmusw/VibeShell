use mosaic_shell_tools::{
    doctor::{missing_required, name_owned},
    process::capture,
    wayland::Global,
};
use std::time::{Duration, Instant};

#[test]
fn required_protocols_are_not_inferred_from_desktop_environment() {
    assert_eq!(
        missing_required(&[]),
        vec![
            "wl_compositor",
            "wl_shm",
            "xdg_wm_base",
            "zwlr_layer_shell_v1",
            "zwlr_foreign_toplevel_manager_v1"
        ]
    );
    let globals: Vec<Global> = missing_required(&[])
        .iter()
        .enumerate()
        .map(|(n, name)| Global {
            name: n as u32 + 1,
            interface: name.to_string(),
            version: 1,
        })
        .collect();
    assert!(missing_required(&globals).is_empty());
}

#[test]
fn role_detection_uses_exact_bus_names() {
    let listing =
        "org.freedesktop.Notifications - - - -\norg.kde.StatusNotifierWatcherExtra - - - -\n";
    assert!(name_owned(listing, "org.freedesktop.Notifications"));
    assert!(!name_owned(listing, "org.kde.StatusNotifierWatcher"));
}

#[test]
fn subprocess_deadline_kills_hung_command() {
    let start = Instant::now();
    assert!(capture("sleep", &["5"], Duration::from_millis(30)).is_err());
    assert!(start.elapsed() < Duration::from_secs(1));
}

#[test]
fn subprocess_argv_is_never_shell_interpolated() {
    assert_eq!(
        capture(
            "printf",
            &["%s", "$(false); `false`"],
            Duration::from_secs(1)
        )
        .unwrap(),
        "$(false); `false`"
    );
}

#[test]
fn subprocess_output_is_bounded() {
    assert!(capture("yes", &[], Duration::from_secs(1)).is_err());
}

#[test]
fn subprocess_failure_is_not_empty_success() {
    assert!(capture("false", &[], Duration::from_secs(1)).is_err());
}
