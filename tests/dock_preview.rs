use mosaic_shell_tools::dock_preview::unique_identifier;

#[test]
fn preview_requires_one_matching_window_in_both_catalogs() {
    let chosen = ("editor", "Document");
    let alternate = ("editor", "Notes");
    let ext = [(chosen, "window-1"), (alternate, "window-2")];
    assert_eq!(
        unique_identifier(chosen, [chosen, alternate], ext),
        Some("window-1")
    );
    assert_eq!(unique_identifier(chosen, [chosen, chosen], ext), None);
    assert_eq!(unique_identifier(chosen, [alternate], ext), None);
    assert_eq!(
        unique_identifier(chosen, [chosen], [ext[0], (chosen, "other")]),
        None
    );
    assert_eq!(unique_identifier(chosen, [chosen], [ext[1]]), None);
    let untitled = ("editor", "");
    assert_eq!(
        unique_identifier(untitled, [untitled], [(untitled, "id")]),
        None
    );
}
