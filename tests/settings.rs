use mosaic_shell_tools::settings::*;
use std::path::PathBuf;
fn dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "mosaic-settings-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&path).unwrap();
    path
}
#[test]
fn pins_roundtrip_and_stale_revision_is_rejected() {
    let d = dir();
    let p = d.join("config.toml");
    let mut s = load(&p).unwrap();
    s.pins = vec!["org.test.App.desktop".into()];
    let next = save(&p, &s).unwrap();
    assert_eq!(next.revision, 1);
    assert_eq!(load(&p).unwrap(), next);
    assert!(save(&p, &s).is_err());
    assert_eq!(load(&p).unwrap(), next);
    std::fs::remove_dir_all(d).unwrap();
}
#[test]
fn invalid_schema_and_unknown_fields_fail_without_overwrite() {
    let d = dir();
    let p = d.join("config.toml");
    for text in [
        "schema_version=99\nrevision=0\npins=[]",
        "schema_version=1\nrevision=0\npins=[]\nunknown=true",
    ] {
        std::fs::write(&p, text).unwrap();
        assert!(load(&p).is_err());
        assert!(save(&p, &Settings::default()).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), text);
    }
    std::fs::remove_dir_all(d).unwrap();
}
#[test]
fn nix_style_symlink_is_not_replaced() {
    let d = dir();
    let real = d.join("managed.toml");
    let p = d.join("config.toml");
    let text = "schema_version=1\nrevision=0\npins=[]";
    std::fs::write(&real, text).unwrap();
    std::os::unix::fs::symlink(&real, &p).unwrap();
    assert!(save(&p, &load(&p).unwrap()).is_err());
    assert!(p.is_symlink());
    assert_eq!(std::fs::read_to_string(real).unwrap(), text);
    std::fs::remove_dir_all(d).unwrap();
}
#[test]
fn invalid_outgoing_schema_cannot_destroy_working_config() {
    let d = dir();
    let p = d.join("config.toml");
    let good = save(&p, &Settings::default()).unwrap();
    let mut bad = good.clone();
    bad.schema_version = 99;
    assert!(save(&p, &bad).is_err());
    assert_eq!(load(&p).unwrap(), good);
    std::fs::remove_dir_all(d).unwrap();
}
#[test]
fn preexisting_temporary_file_is_preserved_on_collision() {
    let d = dir();
    let p = d.join("config.toml");
    let tmp = d.join(format!(".mosaic-config-{}-1.tmp", std::process::id()));
    std::fs::write(&tmp, "keep").unwrap();
    assert!(save(&p, &Settings::default()).is_err());
    assert_eq!(std::fs::read_to_string(&tmp).unwrap(), "keep");
    std::fs::remove_dir_all(d).unwrap();
}
