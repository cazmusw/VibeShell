use mosaic_shell_tools::tray_model::*;
#[test]
fn both_registration_forms_keep_object_identity() {
    assert_eq!(
        registration(":1.12", "/Tray/One").unwrap(),
        (":1.12".into(), "/Tray/One".into())
    );
    assert_eq!(
        registration(":1.12", "org.example.Item").unwrap(),
        ("org.example.Item".into(), "/StatusNotifierItem".into())
    );
    for bad in ["not a service", "/bad//path", "/bad-path", "", "org..Bad"] {
        assert!(registration(":1.12", bad).is_err(), "{bad}");
    }
}
#[test]
fn network_argb_is_decoded_exactly_and_hostile_images_rejected() {
    assert_eq!(
        rgba(2, 1, &[128, 1, 2, 3, 255, 4, 5, 6]).unwrap(),
        vec![1, 2, 3, 128, 4, 5, 6, 255]
    );
    for (w, h, data) in [
        (0, 1, vec![]),
        (-1, 1, vec![]),
        (257, 1, vec![0; 1028]),
        (2, 2, vec![0; 15]),
    ] {
        assert!(rgba(w, h, &data).is_none());
    }
}
