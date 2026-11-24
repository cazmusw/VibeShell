#![cfg(feature = "desktop")]

#[path = "../src/bin/mosaic_files/filesystem.rs"]
mod filesystem;

use filesystem::{
    CreateKind, create_entry, display_location, location_components, read_directory,
    resolve_location, valid_name,
};
use gtk::{gio, glib, prelude::*};
use std::{ffi::OsStr, fs, os::unix::ffi::OsStrExt, path::PathBuf};

struct TestDirectory(PathBuf);

#[test]
fn create_file_is_exclusive_and_rejects_paths() {
    let directory = TestDirectory::new();
    let location = directory.file();
    let context = glib::MainContext::new();
    let created = context
        .block_on(create_entry(&location, "notes.txt", CreateKind::File))
        .unwrap();
    let path = directory.0.join("notes.txt");
    assert!(created.equal(&gio::File::for_path(&path)));
    assert_eq!(fs::metadata(&path).unwrap().len(), 0);
    fs::write(&path, "original").unwrap();
    let conflict = context
        .block_on(create_entry(&location, "notes.txt", CreateKind::File))
        .unwrap_err();
    assert!(conflict.matches(gio::IOErrorEnum::Exists));
    assert_eq!(fs::read(&path).unwrap(), b"original");
    assert!(
        context
            .block_on(create_entry(&location, "../escaped", CreateKind::File))
            .is_err()
    );
    assert!(
        context
            .block_on(create_entry(&location, "folder", CreateKind::Folder))
            .unwrap()
            .path()
            .unwrap()
            .is_dir()
    );
}

impl TestDirectory {
    fn new() -> Self {
        Self(glib::mkdtemp(std::env::temp_dir().join("mosaic-files-test-XXXXXX")).unwrap())
    }

    fn file(&self) -> gio::File {
        gio::File::for_path(&self.0)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn address_uses_the_selected_folder_for_relative_names_with_colons() {
    let directory = TestDirectory::new();
    let target = resolve_location("draft:notes", &directory.file());
    assert_eq!(target.path(), Some(directory.0.join("draft:notes")));
}

#[test]
fn enumeration_keeps_native_filenames_and_includes_hidden_files() {
    let directory = TestDirectory::new();
    let non_unicode = directory.0.join(OsStr::from_bytes(b"notes-\xff"));
    fs::write(&non_unicode, "hello").unwrap();
    fs::write(directory.0.join(".secret"), "").unwrap();
    fs::create_dir(directory.0.join("Z Folder")).unwrap();

    let files = glib::MainContext::new()
        .block_on(read_directory(&directory.file()))
        .unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].name, "Z Folder");
    assert!(files[0].is_directory);
    let size = files[0].folder_size.as_ref().unwrap();
    assert_eq!(size.bytes(), None);
    assert!(size.update(Some(0), "0"));
    assert_eq!(size.bytes(), Some(0));
    assert!(files[1].is_hidden);
    assert_eq!(files[2].file.path(), Some(non_unicode));
    assert_eq!(files[2].size, Some(5));
    assert_eq!(files[2].search_name, glib::casefold(&files[2].name));
    assert!(files[2].modified.is_some());
    assert!(files[2].icon.is_some());
}

#[test]
fn address_round_trips_percent_escaped_local_paths() {
    let directory = TestDirectory::new();
    let file = directory.file().child("100% # ready");
    assert!(resolve_location(&file.uri(), &directory.file()).equal(&file));
    assert_eq!(display_location(&file), "100% # ready");
}

#[test]
fn file_names_are_single_path_components() {
    for name in ["", ".", "..", "sibling/notes", "a\0b"] {
        assert!(!valid_name(name), "{name:?} must be rejected");
    }
    assert!(valid_name("Résumé 2026.txt"));
    assert!(valid_name("draft:notes"));
}

#[test]
fn file_names_sort_numbers_naturally_and_have_type_labels() {
    let directory = TestDirectory::new();
    fs::write(directory.0.join("file10.txt"), "").unwrap();
    fs::write(directory.0.join("file2.txt"), "").unwrap();
    fs::create_dir(directory.0.join("zzz")).unwrap();
    let files = glib::MainContext::new()
        .block_on(read_directory(&directory.file()))
        .unwrap();
    let names: Vec<_> = files.iter().map(|file| file.name.as_str()).collect();
    assert_eq!(names, ["zzz", "file2.txt", "file10.txt"]);
    assert_eq!(files[0].type_label, "Папка");
    assert!(!files[1].type_label.is_empty());
}

#[test]
fn address_segments_keep_exact_parent_targets_including_escaped_names() {
    let location = gio::File::for_uri("file:///home/100%25%20ready/notes");
    let segments = location_components(&location);
    let labels: Vec<_> = segments.iter().map(display_location).collect();
    assert_eq!(labels, ["/", "home", "100% ready", "notes"]);
    assert!(segments[1].equal(&gio::File::for_path("/home")));
    assert!(segments.last().unwrap().equal(&location));
    let trash = gio::File::for_uri("trash:///");
    let segments = location_components(&trash);
    assert_eq!(segments.len(), 1);
    assert!(segments[0].equal(&trash));
}
