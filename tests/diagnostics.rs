use mosaic_shell_tools::diagnostics::Journal;
use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};

static NEXT: AtomicUsize = AtomicUsize::new(0);

struct Directory(PathBuf);

impl Directory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "mosaic-journal-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for Directory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn bounds_retention_and_keeps_private_modes() {
    let dir = Directory::new();
    let path = dir.0.join("mosaic");
    let journal = Journal::open(path.clone(), 256);
    for index in 0..70 {
        journal.record("files", "open_failed", index).unwrap();
    }
    let lines = journal.recent(3).unwrap();
    assert_eq!(lines.len(), 3);
    assert!(lines.last().unwrap().ends_with("\tfiles\topen_failed\t69"));
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o700
    );
    for name in ["events.log", "events.log.1"] {
        let meta = fs::metadata(path.join(name)).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
        assert!(meta.len() <= 256);
    }
}

#[test]
fn refuses_unstructured_text_and_leaves_symlink_target_alone() {
    let dir = Directory::new();
    let path = dir.0.join("mosaic");
    let journal = Journal::open(path.clone(), 256);
    journal.record("files", "open_failed", 4).unwrap();
    assert!(journal.record("files", "/home/private", 0).is_err());
    let other = dir.0.join("outside");
    fs::write(&other, "private").unwrap();
    fs::remove_file(path.join("events.log")).unwrap();
    symlink(&other, path.join("events.log")).unwrap();
    assert!(journal.record("files", "open_failed", 4).is_err());
    assert_eq!(fs::read_to_string(other).unwrap(), "private");
}
