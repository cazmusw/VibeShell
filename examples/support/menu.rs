use crate::State;
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::{cell::RefCell, collections::HashMap, rc::Rc};
pub const XML: &str = r#"<node><interface name="com.canonical.dbusmenu"><method name="AboutToShow"><arg type="i" direction="in"/><arg type="b" direction="out"/></method><method name="GetLayout"><arg type="i" direction="in"/><arg type="i" direction="in"/><arg type="as" direction="in"/><arg type="u" direction="out"/><arg type="(ia{sv}av)" direction="out"/></method><method name="Event"><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="v" direction="in"/><arg type="u" direction="in"/></method><signal name="LayoutUpdated"><arg type="u"/><arg type="i"/></signal><signal name="ItemsPropertiesUpdated"><arg type="a(ia{sv})"/><arg type="a(ias)"/></signal></interface></node>"#;
#[derive(Default)]
pub struct MenuState {
    pub enabled: bool,
    pub fail_event: bool,
    pub mode: String,
    pub checked: bool,
    pub lazy: bool,
    pub revision: u32,
    pub hold: bool,
    pub held: Vec<(gio::DBusMethodInvocation, glib::Variant)>,
}
fn node(id: i32, p: Vec<(&str, glib::Variant)>, children: Vec<glib::Variant>) -> glib::Variant {
    (
        id,
        p.into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect::<HashMap<_, _>>(),
        children,
    )
        .to_variant()
}
fn leaf(id: i32, label: &str) -> glib::Variant {
    node(id, vec![("label", label.to_variant())], vec![])
}
fn layout(s: &State) -> glib::Variant {
    let m = &s.dbusmenu;
    let mut children = vec![
        node(
            1,
            vec![
                (
                    "label",
                    if m.mode == "label" {
                        "Updated menu"
                    } else {
                        "_Open"
                    }
                    .to_variant(),
                ),
                ("enabled", (m.mode != "disabled").to_variant()),
            ],
            vec![],
        ),
        node(
            2,
            vec![
                ("label", "_Checked".to_variant()),
                ("toggle-type", "checkmark".to_variant()),
                (
                    "toggle-state",
                    (if m.checked { 1i32 } else { 0 }).to_variant(),
                ),
            ],
            vec![],
        ),
        node(
            3,
            vec![
                ("label", "Disabled".to_variant()),
                ("enabled", false.to_variant()),
            ],
            vec![],
        ),
        node(
            4,
            vec![
                ("label", "Hidden".to_variant()),
                ("visible", false.to_variant()),
            ],
            vec![],
        ),
        node(5, vec![("type", "separator".to_variant())], vec![]),
        node(
            6,
            vec![
                ("label", "_Options".to_variant()),
                ("children-display", "submenu".to_variant()),
            ],
            if m.lazy {
                vec![
                    node(
                        7,
                        vec![
                            ("label", "Choice A".to_variant()),
                            ("toggle-type", "radio".to_variant()),
                            ("toggle-state", 1i32.to_variant()),
                        ],
                        vec![],
                    ),
                    node(
                        8,
                        vec![
                            ("label", "Choice B".to_variant()),
                            ("toggle-type", "radio".to_variant()),
                            ("toggle-state", 0i32.to_variant()),
                        ],
                        vec![],
                    ),
                    leaf(9, "Nested action"),
                ]
            } else {
                vec![]
            },
        ),
        node(
            12,
            vec![
                ("label", "Disabled parent".to_variant()),
                ("enabled", false.to_variant()),
            ],
            vec![leaf(13, "Blocked child")],
        ),
    ];
    if m.mode != "removed" {
        children.push(leaf(10, "Remove me"));
    }
    if m.mode == "bad" {
        children.push(leaf(1, "Duplicate"));
    }
    if m.mode == "huge" {
        children.extend((100..400).map(|i| leaf(i, "Long menu")));
    }
    glib::Variant::tuple_from_iter([m.revision.to_variant(), node(0, vec![], children)])
}
fn signal(conn: &gio::DBusConnection, s: &mut State, properties: bool) {
    s.dbusmenu.revision += 1;
    let (member, args) = if properties {
        (
            "ItemsPropertiesUpdated",
            (
                vec![(1i32, HashMap::<String, glib::Variant>::new())],
                Vec::<(i32, Vec<String>)>::new(),
            )
                .to_variant(),
        )
    } else {
        ("LayoutUpdated", (s.dbusmenu.revision, 0i32).to_variant())
    };
    let _ = conn.emit_signal(None, "/Menu", "com.canonical.dbusmenu", member, Some(&args));
}
pub fn register(
    conn: &gio::DBusConnection,
    state: &Rc<RefCell<State>>,
) -> Result<gio::RegistrationId, glib::Error> {
    let info = gio::DBusNodeInfo::for_xml(XML)?
        .lookup_interface("com.canonical.dbusmenu")
        .unwrap();
    let s = state.clone();
    conn.register_object("/Menu", &info)
        .method_call(move |conn, _, _, _, method, args, inv| match method {
            "AboutToShow" => {
                let (id,) = args.get::<(i32,)>().unwrap();
                let mut s = s.borrow_mut();
                s.events.push(format!("AboutToShow:{id}"));
                let update = id == 6 && !s.dbusmenu.lazy;
                if update {
                    s.dbusmenu.lazy = true;
                    signal(&conn, &mut s, false);
                }
                inv.return_value(Some(&(update,).to_variant()));
            }
            "GetLayout" => {
                let mut s = s.borrow_mut();
                s.events.push("GetLayout".into());
                let value = layout(&s);
                if s.dbusmenu.hold {
                    s.dbusmenu.held.push((inv, value));
                } else {
                    inv.return_value(Some(&value));
                }
            }
            _ => {
                let (id, event, _, _) = args.get::<(i32, String, glib::Variant, u32)>().unwrap();
                let mut s = s.borrow_mut();
                if event == "clicked" && s.dbusmenu.fail_event {
                    inv.return_dbus_error(
                        "org.mosaic.Fixture.Error",
                        "deliberate menu event failure",
                    );
                    return;
                }
                s.events.push(format!("MenuEvent:{id}:{event}"));
                if id == 2 && event == "clicked" {
                    s.dbusmenu.checked = !s.dbusmenu.checked;
                    signal(&conn, &mut s, true);
                }
                inv.return_value(Some(&().to_variant()));
            }
        })
        .build()
}
pub fn admin(
    conn: &gio::DBusConnection,
    s: &Rc<RefCell<State>>,
    method: &str,
    args: &glib::Variant,
) -> bool {
    match method {
        "FailEvent" => {
            s.borrow_mut().dbusmenu.fail_event = args.get::<(bool,)>().unwrap().0;
        }
        "DBusMenu" => {
            s.borrow_mut().dbusmenu.enabled = args.get::<(bool,)>().unwrap().0;
            let _ = conn.emit_signal(
                None,
                "/StatusNotifierItem",
                "org.kde.StatusNotifierItem",
                "NewTitle",
                Some(&().to_variant()),
            );
        }
        "MenuUpdate" => {
            let (mode,) = args.get::<(String,)>().unwrap();
            let mut s = s.borrow_mut();
            s.dbusmenu.mode = mode;
            let properties = s.dbusmenu.mode == "label" || s.dbusmenu.mode == "disabled";
            signal(conn, &mut s, properties);
        }
        "HoldLayout" => {
            s.borrow_mut().dbusmenu.hold = args.get::<(bool,)>().unwrap().0;
        }
        "ReleaseLayout" => {
            let held = std::mem::take(&mut s.borrow_mut().dbusmenu.held);
            for (inv, value) in held {
                inv.return_value(Some(&value));
            }
        }
        _ => return false,
    }
    true
}
