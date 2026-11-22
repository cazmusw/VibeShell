use mosaic_shell_tools::desktop::{DockClick, Window, dock_click};

fn window(id: u32, active: bool, minimized: bool) -> Window {
    Window {
        epoch: 1,
        id,
        active,
        minimized,
        app_id: String::new(),
        title: String::new(),
        fullscreen: false,
        outputs: vec![],
    }
}

#[test]
fn cycles_in_creation_order_and_minimizes_at_the_end() {
    assert_eq!(
        dock_click(&[window(9, false, true), window(2, false, false)], true),
        DockClick::Activate(1, 2)
    );
    assert_eq!(
        dock_click(&[window(9, false, true), window(2, true, false)], true),
        DockClick::Activate(1, 9)
    );
    assert_eq!(
        dock_click(&[window(9, true, false), window(2, false, false)], true),
        DockClick::Minimize(1, 9)
    );
}

#[test]
fn single_window_toggles_with_an_explicit_opt_out() {
    assert_eq!(
        dock_click(&[window(2, true, false)], true),
        DockClick::Minimize(1, 2)
    );
    assert_eq!(
        dock_click(&[window(2, true, false)], false),
        DockClick::Activate(1, 2)
    );
    assert_eq!(
        dock_click(&[window(2, false, true)], true),
        DockClick::Activate(1, 2)
    );
    assert_eq!(dock_click(&[], true), DockClick::Launch);
}
