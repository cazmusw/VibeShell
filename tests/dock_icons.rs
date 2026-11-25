use mosaic_shell_tools::{
    dock_icons::{valid_asset, valid_key},
    settings::{Settings, patched},
};

#[test]
fn overrides_are_optional_and_replaceable_in_schema_two() {
    let initial = Settings::default();
    assert!(initial.dock_icons.is_empty());
    let asset = format!("{}.png", "a".repeat(64));
    let patch = format!("[dock_icons]\n'app.desktop'='{asset}'");
    let updated = patched(&initial, &patch).unwrap();
    assert_eq!(updated.dock_icons["app.desktop"], asset);
    assert_eq!(updated.pins, initial.pins);
    assert!(
        patched(&updated, "[dock_icons]")
            .unwrap()
            .dock_icons
            .is_empty()
    );
    assert!(patched(&initial, "[dock_icons]\n'app.desktop'='../outside.png'").is_err());
    assert!(patched(&initial, &format!("[dock_icons]\n'window:12:34'='{asset}'")).is_err());
}

#[test]
fn only_managed_basenames_and_stable_app_ids_are_accepted() {
    assert!(valid_key("org.test.App.desktop"));
    assert!(valid_key("window:org.test.App"));
    assert!(!valid_key("window:42:1"));
    assert!(!valid_key("window:"));
    assert!(!valid_key("this/is.desktop"));
    assert!(valid_asset(&format!("{}.png", "8".repeat(64))));
    assert!(!valid_asset("../outside.png"));
    assert!(!valid_asset(&format!("{}.png", "X".repeat(64))));
}

#[cfg(feature = "desktop")]
#[test]
fn png_import_owns_a_bounded_copy_independent_of_the_original() {
    use gtk::gdk_pixbuf::{Colorspace, Pixbuf};
    use mosaic_shell_tools::dock_icons::{asset_path, import_png, load_png};
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
        path::PathBuf,
    };

    struct Folder(PathBuf);
    impl Drop for Folder {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
    let parent =
        Folder(gtk::glib::mkdtemp(std::env::temp_dir().join("mosaic-icons-test-XXXXXX")).unwrap());
    let source = parent.0.join("original.png");
    let config = parent.0.join("mosaic").join("config.toml");
    let pixels = Pixbuf::new(Colorspace::Rgb, true, 8, 320, 160).unwrap();
    pixels.fill(0x104080ff);
    let bytes = pixels.save_to_bufferv("png", &[]).unwrap();
    fs::write(&source, &bytes).unwrap();
    let selected = import_png(&config, &source).unwrap();
    assert_eq!(selected, import_png(&config, &source).unwrap());
    fs::remove_file(&source).unwrap();
    let stored = asset_path(&config, &selected).unwrap();
    assert!(stored.is_file());
    assert_eq!(
        stored.metadata().unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(
        stored
            .parent()
            .unwrap()
            .metadata()
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700,
    );
    let copied = load_png(&config, &selected, 64).unwrap();
    assert_eq!((copied.width(), copied.height()), (64, 32));
    let tiny = parent.0.join("tiny.png");
    let low_resolution = Pixbuf::new(Colorspace::Rgb, true, 8, 8, 4).unwrap();
    low_resolution.fill(0x20a060ff);
    low_resolution.savev(&tiny, "png", &[]).unwrap();
    let selected_tiny = import_png(&config, &tiny).unwrap();
    let native = load_png(&config, &selected_tiny, 160).unwrap();
    assert_eq!((native.width(), native.height()), (8, 4));
    let mut invalid = bytes.clone();
    invalid[16..20].copy_from_slice(&4097_u32.to_be_bytes());
    fs::write(&source, invalid).unwrap();
    assert!(import_png(&config, &source).is_err());

    fs::remove_file(&stored).unwrap();
    assert!(load_png(&config, &selected, 64).is_none());
    fs::write(&source, bytes).unwrap();
    symlink(&source, &stored).unwrap();
    assert!(load_png(&config, &selected, 64).is_none());
    assert!(import_png(&config, &source).is_err());
}
