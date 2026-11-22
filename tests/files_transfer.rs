#![cfg(feature = "desktop")]

#[path = "../src/bin/mosaic_files/transfer.rs"]
mod transfer;

use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
};
use transfer::{Batch, Kind, TransferError, transfer};

#[test]
fn partially_completed_cut_keeps_only_items_that_have_not_moved() {
    let paths = ["done", "failed", "cancelled"].map(PathBuf::from);
    let moved = [paths[0].clone()].into_iter().collect();
    let cut = Batch {
        sources: paths.to_vec(),
        kind: Kind::Move,
    };
    assert_eq!(cut.after_paste(&moved).unwrap().sources, paths[1..]);
    let copy = Batch {
        sources: paths.to_vec(),
        kind: Kind::Copy,
    };
    assert_eq!(copy.after_paste(&moved).unwrap().sources, paths);
    let cut = Batch {
        sources: vec![PathBuf::from("done")],
        kind: Kind::Move,
    };
    assert!(cut.after_paste(&moved).is_none());
}

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        Self(glib::mkdtemp(std::env::temp_dir().join("mosaic-transfer-test-XXXXXX")).unwrap())
    }
}

use gtk::glib;

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn copies_nested_files_and_preserves_symbolic_links() {
    let test = TestDirectory::new();
    let source = test.0.join("folder");
    let destination = test.0.join("output");
    fs::create_dir_all(source.join("child")).unwrap();
    fs::set_permissions(&source, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::write(source.join("child/notes.txt"), "notes").unwrap();
    symlink("child/notes.txt", source.join("shortcut")).unwrap();
    let copied = transfer(&source, &destination, Kind::Copy, false).unwrap();
    assert_eq!(fs::read(copied.join("child/notes.txt")).unwrap(), b"notes");
    assert_eq!(
        fs::read_link(copied.join("shortcut")).unwrap(),
        PathBuf::from("child/notes.txt")
    );
    assert!(source.exists());
    assert_eq!(
        fs::metadata(copied).unwrap().permissions().mode() & 0o777,
        0o700
    );
}

#[test]
fn handles_conflicts_without_overwriting_and_can_keep_both() {
    let test = TestDirectory::new();
    let source = test.0.join("draft.txt");
    let destination = test.0.join("output");
    fs::write(&source, "new").unwrap();
    fs::create_dir(&destination).unwrap();
    let existing = destination.join("draft.txt");
    fs::write(&existing, "existing").unwrap();
    assert!(matches!(
        transfer(&source, &destination, Kind::Move, false),
        Err(TransferError::Conflict)
    ));
    assert_eq!(fs::read(&source).unwrap(), b"new");
    assert_eq!(fs::read(&existing).unwrap(), b"existing");
    let moved = transfer(&source, &destination, Kind::Move, true).unwrap();
    assert_eq!(moved.file_name().unwrap(), "draft — копия.txt");
    assert_eq!(fs::read(&moved).unwrap(), b"new");
    assert!(!source.exists());
}

#[test]
fn duplicates_in_place_and_rejects_a_directory_inside_itself() {
    let test = TestDirectory::new();
    let source = test.0.join("folder");
    let child = source.join("child");
    fs::create_dir_all(&child).unwrap();
    assert!(matches!(
        transfer(&source, &child, Kind::Copy, false),
        Err(TransferError::Io(_))
    ));
    assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
    let copied = transfer(&source, &test.0, Kind::Copy, false).unwrap();
    assert_eq!(copied.file_name().unwrap(), "folder — копия");
    assert!(copied.join("child").is_dir());
    assert_eq!(
        transfer(&source, &test.0, Kind::Move, false).unwrap(),
        source
    );
}
