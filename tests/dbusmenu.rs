#![cfg(feature = "desktop")]
use gtk::glib::{self, variant::ToVariant};
use mosaic_shell_tools::menu_model::*;
use std::collections::HashMap;
fn node(id: i32, props: &[(&str, glib::Variant)], children: Vec<glib::Variant>) -> glib::Variant {
    (
        id,
        props
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect::<HashMap<_, _>>(),
        children,
    )
        .to_variant()
}
fn reply(root: glib::Variant) -> glib::Variant {
    glib::Variant::tuple_from_iter([7u32.to_variant(), root])
}
#[test]
fn recursive_menu_defaults_toggles_and_ancestor_rules() {
    let tree = node(
        0,
        &[],
        vec![
            node(1, &[("label", "_Open__file".to_variant())], vec![]),
            node(
                2,
                &[
                    ("label", "_Options".to_variant()),
                    ("children-display", "submenu".to_variant()),
                    ("enabled", false.to_variant()),
                ],
                vec![node(
                    3,
                    &[
                        ("toggle-type", "checkmark".to_variant()),
                        ("toggle-state", 1i32.to_variant()),
                    ],
                    vec![],
                )],
            ),
            node(
                4,
                &[("visible", false.to_variant())],
                vec![node(5, &[], vec![])],
            ),
            node(6, &[("type", "separator".to_variant())], vec![]),
        ],
    );
    let (revision, rows) = decode(&reply(tree)).unwrap();
    assert_eq!(revision, 7);
    assert_eq!(rows[1].label, "Open_file");
    assert!(rows[1].enabled && rows[1].visible);
    assert!(rows[2].submenu);
    assert!(!rows[3].enabled);
    assert_eq!(rows[3].toggle_state, 1);
    assert_eq!(rows[3].parent, 2);
    assert!(!rows[5].visible);
    assert!(!rows[6].enabled);
}
#[test]
fn malformed_duplicate_deep_and_oversized_menus_are_errors() {
    assert!(decode(&().to_variant()).is_err());
    assert!(decode(&reply(node(1, &[], vec![]))).is_err());
    assert!(
        decode(&reply(node(
            0,
            &[("enabled", "false".to_variant())],
            vec![]
        )))
        .is_err()
    );
    assert!(
        decode(&reply(node(
            0,
            &[],
            vec![node(1, &[], vec![]), node(1, &[], vec![])]
        )))
        .is_err()
    );
    let mut deep = node(20, &[], vec![]);
    for id in (0..20).rev() {
        deep = node(id, &[], vec![deep]);
    }
    assert!(decode(&reply(deep)).is_err());
    assert!(
        decode(&reply(node(
            0,
            &[],
            (1..257).map(|id| node(id, &[], vec![])).collect()
        )))
        .is_err()
    );
    assert!(
        decode(&reply(node(
            0,
            &[("label", "x".repeat(1_100_000).to_variant())],
            vec![]
        )))
        .is_err()
    );
}
#[test]
fn menu_snapshot_rejects_disabled_hidden_non_action_and_stale_token() {
    let (_, rows) = decode(&reply(node(
        0,
        &[],
        vec![
            node(1, &[], vec![]),
            node(2, &[("enabled", false.to_variant())], vec![]),
            node(3, &[("visible", false.to_variant())], vec![]),
        ],
    )))
    .unwrap();
    let s = MenuSnapshot {
        token: 42,
        revision: 7,
        rows,
    };
    assert!(s.can_click(42, 1));
    assert!(!s.can_click(41, 1));
    assert!(!s.can_click(42, 0));
    assert!(!s.can_click(42, 2));
    assert!(!s.can_click(42, 3));
    assert!(!s.can_click(42, 999));
}
