use shell_wayland::{Capabilities, WindowCommand};
#[test]
fn globals_report_missing_required_protocols() {
    let c = Capabilities::from_globals(vec![("wl_compositor".into(), 6)]);
    assert!(!c.ready());
    assert!(c.missing().contains(&"zwlr_layer_shell_v1"));
}
#[test]
fn command_preserves_connection_epoch() {
    let c = WindowCommand::Activate { epoch: 22, id: 7 };
    assert_eq!(c.target(), (22, 7));
}
