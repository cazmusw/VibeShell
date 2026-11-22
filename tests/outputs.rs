use mosaic_shell_tools::{
    outputs::select_output,
    settings::{Settings, patched},
};
fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}
#[test]
fn preferred_returns_after_disconnect_and_order_does_not_change_fallback() {
    let both = names(&["DP-2", "DP-1"]);
    assert_eq!(select_output("DP-2", &both), Some("DP-2".into()));
    assert_eq!(
        select_output("DP-2", &names(&["DP-1"])),
        Some("DP-1".into())
    );
    assert_eq!(select_output("DP-2", &both), Some("DP-2".into()));
    assert_eq!(select_output("missing", &both), Some("DP-1".into()));
    assert_eq!(
        select_output("", &names(&["DP-1", "DP-2"])),
        Some("DP-1".into())
    );
}
#[test]
fn no_output_has_no_selection_and_anonymous_does_not_beat_named() {
    assert_eq!(select_output("DP-1", &[]), None);
    assert_eq!(
        select_output("", &names(&["", "DP-1"])),
        Some("DP-1".into())
    );
    assert_eq!(select_output("", &names(&[""])), None);
}
#[test]
fn preferred_config_is_optional_validated_and_keeps_pins() {
    let base = Settings::default();
    let next = patched(&base, "dock.preferred_output='DP-2'").unwrap();
    assert_eq!(next.dock.preferred_output, "DP-2");
    assert_eq!(next.pins, base.pins);
    assert_eq!(next.revision, base.revision);
    assert!(patched(&base, "dock.preferred_output=''").is_ok());
    assert!(patched(&base, "dock.preferred_output=42").is_err());
    assert!(
        patched(
            &base,
            &format!("dock.preferred_output='{}'", "a".repeat(257))
        )
        .is_err()
    );
    assert!(patched(&base, "dock.preferred_output=\"a\\nb\"").is_err());
}

#[test]
fn dialog_chooses_hint_active_output_then_preferred() {
    use mosaic_shell_tools::outputs::select_dialog_output;
    let available = names(&["DP-3", "DP-2", "DP-1"]);
    let active = names(&["DP-1", "DP-2", "unplugged"]);
    assert_eq!(
        select_dialog_output("DP-3", &active, "DP-2", &available),
        Some("DP-3".into())
    );
    assert_eq!(
        select_dialog_output("", &active, "DP-2", &available),
        Some("DP-2".into())
    );
    assert_eq!(
        select_dialog_output("unplugged", &active, "DP-3", &available),
        Some("DP-1".into())
    );
    assert_eq!(
        select_dialog_output("", &names(&["gone"]), "DP-3", &available),
        Some("DP-3".into())
    );
    assert_eq!(
        select_dialog_output("", &[], "gone", &available),
        Some("DP-1".into())
    );
    assert_eq!(select_dialog_output("", &active, "DP-1", &[]), None);
}
