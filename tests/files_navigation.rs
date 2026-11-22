#[path = "../src/bin/mosaic_files/navigation.rs"]
mod navigation;

use navigation::{History, Navigation};

#[test]
fn unsuccessful_navigation_leaves_the_current_directory_and_forward_history_intact() {
    let mut history = History::new("file:///home".into());
    history.commit(Navigation::Open("file:///home/notes".into()));
    history.commit(Navigation::Back);

    let pending = Navigation::Open("file:///missing".into());
    assert_eq!(history.destination(&pending), Some("file:///missing"));
    assert_eq!(history.current(), "file:///home");
    assert_eq!(
        history.destination(&Navigation::Forward),
        Some("file:///home/notes")
    );
}

#[test]
fn opening_a_new_directory_from_an_older_entry_replaces_the_forward_branch() {
    let mut history = History::new("file:///home".into());
    history.commit(Navigation::Open("file:///home/notes".into()));
    history.commit(Navigation::Back);
    history.commit(Navigation::Reload);
    history.commit(Navigation::Open("file:///home/music".into()));

    assert_eq!(history.current(), "file:///home/music");
    assert_eq!(history.destination(&Navigation::Back), Some("file:///home"));
    assert_eq!(history.destination(&Navigation::Forward), None);
}

#[test]
fn reopening_the_current_directory_does_not_add_a_back_entry() {
    let mut history = History::new("file:///home".into());
    history.commit(Navigation::Open("file:///home".into()));

    assert_eq!(history.destination(&Navigation::Back), None);
}
