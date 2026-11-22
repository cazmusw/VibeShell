mod support {
    pub mod menu;
}
// Only the private integration bus is supported; never registers on host bus.
use glib::variant::ToVariant;
use gtk::{gio, glib};
use mosaic_shell_tools::tray_backend::{self, WATCH_PATH, WATCHERS};
use std::{cell::RefCell, rc::Rc};
const ITEM: &str = r#"<node><interface name="org.kde.StatusNotifierItem"><property name="Menu" type="o" access="read"/><property name="Title" type="s" access="read"/><property name="Status" type="s" access="read"/><property name="IconName" type="s" access="read"/><property name="IconPixmap" type="a(iiay)" access="read"/><property name="ToolTip" type="(sa(iiay)ss)" access="read"/><property name="ItemIsMenu" type="b" access="read"/><method name="Activate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><method name="SecondaryActivate"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><method name="ContextMenu"><arg type="i" direction="in"/><arg type="i" direction="in"/></method><method name="Scroll"><arg type="i" direction="in"/><arg type="s" direction="in"/></method><signal name="NewTitle"/><signal name="NewStatus"><arg type="s"/></signal></interface></node>"#;
const ADMIN: &str = r#"<node><interface name="org.mosaic.TrayFixture"><method name="Register"><arg type="s" direction="in"/></method><method name="Status"><arg type="s" direction="in"/></method><method name="Title"><arg type="s" direction="in"/></method><method name="Menu"><arg type="b" direction="in"/></method><method name="FailEvent"><arg type="b" direction="in"/></method><method name="DBusMenu"><arg type="b" direction="in"/></method><method name="MenuUpdate"><arg type="s" direction="in"/></method><method name="HoldLayout"><arg type="b" direction="in"/></method><method name="ReleaseLayout"/><method name="Events"><arg type="s" direction="out"/></method><method name="Add"><arg type="s" direction="in"/></method><method name="Unregister"><arg type="s" direction="in"/></method><method name="ReleaseWatcher"/><method name="ReleaseHost"/><method name="DropAlias"/><method name="Quit"/></interface></node>"#;
#[derive(Default)]
struct State {
    dbusmenu: support::menu::MenuState,
    title: String,
    status: String,
    menu: bool,
    events: Vec<String>,
    registered: Vec<String>,
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let runtime = std::env::var("XDG_RUNTIME_DIR")?;
    if runtime.starts_with("/run/user") || !runtime.contains("mosaic-") {
        return Err("private fixture runtime required".into());
    }
    let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
    let own = |name: &str| -> Result<(), glib::Error> {
        conn.call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            Some(&(name, 4u32).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )?;
        Ok(())
    };
    let args: Vec<_> = std::env::args().collect();
    let admin = args
        .iter()
        .find_map(|s| s.strip_prefix("--admin="))
        .unwrap_or("org.mosaic.TrayFixture");
    own(admin)?;
    if admin == "org.mosaic.TrayFixture" {
        own("org.example.TrayAlias")?;
    }
    let watchers: Vec<&str> = WATCHERS
        .into_iter()
        .filter(|name| {
            args.iter().any(|s| {
                s == "--watcher"
                    || (s == "--watcher=kde" && name.contains("kde"))
                    || (s == "--watcher=fd" && name.contains("freedesktop"))
            })
        })
        .collect();
    let pending: Rc<RefCell<Vec<gio::DBusMethodInvocation>>> = Rc::new(RefCell::new(Vec::new()));
    let state = Rc::new(RefCell::new(State {
        title: "Fixture tray".into(),
        status: "Active".into(),
        ..State::default()
    }));
    let mut objects = vec![support::menu::register(&conn, &state)?];
    let mut paths = vec!["/StatusNotifierItem".to_owned(), "/Other".to_owned()];
    if args.iter().any(|arg| arg == "--many-items") {
        paths.extend((0..16).map(|index| format!("/Tray{index:02}")));
    }
    for path in &paths {
        let info = gio::DBusNodeInfo::for_xml(ITEM)?
            .lookup_interface("org.kde.StatusNotifierItem")
            .unwrap();
        let s = state.clone();
        let props = s.clone();
        objects.push(
            conn.register_object(path, &info)
                .method_call(move |_, _, _, _, method, args, inv| {
                    s.borrow_mut().events.push(format!("{method}:{args}"));
                    inv.return_value(Some(&().to_variant()));
                })
                .property(move |_, _, _, _, p| {
                    let s = props.borrow();
                    match p {
                        "Menu" => glib::variant::ObjectPath::try_from(if s.dbusmenu.enabled {
                            "/Menu"
                        } else {
                            "/"
                        })
                        .unwrap()
                        .to_variant(),
                        "Title" => s.title.to_variant(),
                        "Status" => s.status.to_variant(),
                        "IconName" => "missing-fixture-icon".to_variant(),
                        "IconPixmap" => {
                            vec![(2i32, 1i32, vec![255u8, 255, 0, 0, 128, 0, 255, 0])].to_variant()
                        }
                        "ToolTip" => (
                            "",
                            Vec::<(i32, i32, Vec<u8>)>::new(),
                            &s.title,
                            "Tooltip literal <markup>",
                        )
                            .to_variant(),
                        _ => s.menu.to_variant(),
                    }
                })
                .build()?,
        );
    }
    if !watchers.is_empty() {
        for name in watchers.iter().copied() {
            own(name)?;
            let info = gio::DBusNodeInfo::for_xml(&tray_backend::watcher_xml(name))?
                .lookup_interface(name)
                .unwrap();
            let s = state.clone();
            let p = s.clone();
            let held = pending.clone();
            let hold = args.iter().any(|s| s == "--hold-host");
            objects.push(
                conn.register_object(WATCH_PATH, &info)
                    .method_call(move |conn, sender, _, _, method, args, inv| {
                        if method == "RegisterStatusNotifierItem" {
                            let (value,) = args.get::<(String,)>().unwrap();
                            let value = if value.starts_with('/') {
                                format!("{}{value}", sender.unwrap())
                            } else {
                                value
                            };
                            s.borrow_mut().registered.push(value.clone());
                            let _ = conn.emit_signal(
                                None,
                                WATCH_PATH,
                                name,
                                "StatusNotifierItemRegistered",
                                Some(&(value,).to_variant()),
                            );
                        } else {
                            s.borrow_mut().events.push("HostRegistered".into());
                            if hold {
                                held.borrow_mut().push(inv);
                                return;
                            }
                        }
                        inv.return_value(Some(&().to_variant()));
                    })
                    .property(move |_, _, _, _, prop| match prop {
                        "RegisteredStatusNotifierItems" => p.borrow().registered.to_variant(),
                        "ProtocolVersion" => 0i32.to_variant(),
                        _ => true.to_variant(),
                    })
                    .build()?,
            );
        }
    }
    let loop_ = glib::MainLoop::new(None, false);
    if args.iter().any(|s| s == "--preload") {
        state
            .borrow_mut()
            .registered
            .push("org.mosaic.TrayFixture".into());
    }
    let quit = loop_.clone();
    let release_names = watchers.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    let s = state.clone();
    let info = gio::DBusNodeInfo::for_xml(ADMIN)?
        .lookup_interface("org.mosaic.TrayFixture")
        .unwrap();
    objects.push(
        conn.register_object("/Fixture", &info)
            .method_call(move |conn, _, _, _, method, args, inv| {
                if support::menu::admin(&conn, &s, method, &args) {
                    inv.return_value(Some(&().to_variant()));
                    return;
                }
                match method {
                    "Add" | "Unregister" => {
                        let (value,) = args.get::<(String,)>().unwrap();
                        if method == "Add" {
                            s.borrow_mut().registered.push(value.clone());
                        } else {
                            s.borrow_mut().registered.retain(|v| v != &value);
                        }
                        for name in &release_names {
                            let _ = conn.emit_signal(
                                None,
                                WATCH_PATH,
                                name,
                                if method == "Add" {
                                    "StatusNotifierItemRegistered"
                                } else {
                                    "StatusNotifierItemUnregistered"
                                },
                                Some(&(value.clone(),).to_variant()),
                            );
                        }
                        inv.return_value(Some(&().to_variant()));
                    }
                    "ReleaseHost" => {
                        for held in pending.borrow_mut().drain(..) {
                            held.return_value(Some(&().to_variant()));
                        }
                        inv.return_value(Some(&().to_variant()));
                    }
                    "ReleaseWatcher" => {
                        let names = release_names.clone();
                        glib::MainContext::default().spawn_local(async move {
                            for name in names {
                                let _ = tray_backend::call(
                                    &conn,
                                    "org.freedesktop.DBus",
                                    "/org/freedesktop/DBus",
                                    "org.freedesktop.DBus",
                                    "ReleaseName",
                                    (name,).to_variant(),
                                )
                                .await;
                            }
                            inv.return_value(Some(&().to_variant()));
                        });
                    }
                    "Events" => {
                        inv.return_value(Some(&(s.borrow().events.join("\n"),).to_variant()))
                    }
                    "DropAlias" => {
                        glib::MainContext::default().spawn_local(async move {
                            let _ = tray_backend::call(
                                &conn,
                                "org.freedesktop.DBus",
                                "/org/freedesktop/DBus",
                                "org.freedesktop.DBus",
                                "ReleaseName",
                                ("org.example.TrayAlias",).to_variant(),
                            )
                            .await;
                            inv.return_value(Some(&().to_variant()));
                        });
                    }
                    "Quit" => {
                        inv.return_value(Some(&().to_variant()));
                        let quit = quit.clone();
                        glib::MainContext::default().spawn_local(async move {
                            let _ = conn.flush_future().await;
                            quit.quit();
                        });
                    }
                    "Register" => {
                        let (value,) = args.get::<(String,)>().unwrap();
                        glib::MainContext::default().spawn_local(async move {
                            match tray_backend::call(
                                &conn,
                                WATCHERS[0],
                                WATCH_PATH,
                                WATCHERS[0],
                                "RegisterStatusNotifierItem",
                                (value,).to_variant(),
                            )
                            .await
                            {
                                Ok(_) => inv.return_value(Some(&().to_variant())),
                                Err(e) => inv
                                    .return_dbus_error("org.mosaic.Fixture.Error", &e.to_string()),
                            }
                        });
                    }
                    _ => {
                        if method == "Menu" {
                            s.borrow_mut().menu = args.get::<(bool,)>().unwrap().0;
                        } else {
                            let (value,) = args.get::<(String,)>().unwrap();
                            if method == "Title" {
                                s.borrow_mut().title = value;
                            } else {
                                s.borrow_mut().status = value;
                            }
                        }
                        let member = if method == "Status" {
                            "NewStatus"
                        } else {
                            "NewTitle"
                        };
                        let value = if method == "Status" {
                            (s.borrow().status.clone(),).to_variant()
                        } else {
                            ().to_variant()
                        };
                        let _ = conn.emit_signal(
                            None,
                            "/StatusNotifierItem",
                            "org.kde.StatusNotifierItem",
                            member,
                            Some(&value),
                        );
                        inv.return_value(Some(&().to_variant()));
                    }
                }
            })
            .build()?,
    );
    loop_.run();
    drop(objects);
    Ok(())
}
