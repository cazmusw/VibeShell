//! Event-driven SNI adapter. Confined to a dedicated GLib context, never GTK.
#[path = "menu_backend.rs"]
mod menu;
use crate::tray_model::{self, TrayItem, Wire};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap, HashSet},
    rc::Rc,
};
pub const PATH: &str = "/org/mosaic/Tray1";
pub const API: &str = "org.mosaic.Tray1";
pub const WATCH_PATH: &str = "/StatusNotifierWatcher";
pub const WATCHERS: [&str; 2] = [
    "org.kde.StatusNotifierWatcher",
    "org.freedesktop.StatusNotifierWatcher",
];
pub const XML: &str = r#"<node><interface name="org.mosaic.Tray1"><method name="GetSnapshot"><arg type="t" direction="out"/><arg type="a(sssssiiayb)" direction="out"/></method><method name="Action"><arg type="s" direction="in"/><arg type="s" direction="in"/><arg type="i" direction="in"/><arg type="s" direction="in"/></method><method name="GetMenu"><arg type="s" direction="in"/><arg type="i" direction="in"/><arg type="b" direction="in"/><arg type="b" direction="out"/><arg type="t" direction="out"/><arg type="u" direction="out"/><arg type="a(iisbbssisb)" direction="out"/></method><method name="MenuEvent"><arg type="s" direction="in"/><arg type="t" direction="in"/><arg type="i" direction="in"/><arg type="s" direction="in"/><arg type="u" direction="in"/></method><signal name="MenuChanged"><arg type="s"/></signal><signal name="Changed"><arg type="t"/></signal></interface></node>"#;
pub fn watcher_xml(interface: &str) -> String {
    format!(
        r#"<node><interface name="{interface}"><method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method><method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method><property name="RegisteredStatusNotifierItems" type="as" access="read"/><property name="IsStatusNotifierHostRegistered" type="b" access="read"/><property name="ProtocolVersion" type="i" access="read"/><signal name="StatusNotifierItemRegistered"><arg type="s"/></signal><signal name="StatusNotifierItemUnregistered"><arg type="s"/></signal><signal name="StatusNotifierHostRegistered"/></interface></node>"#
    )
}
pub async fn call(
    conn: &gio::DBusConnection,
    dest: &str,
    path: &str,
    iface: &str,
    method: &str,
    args: glib::Variant,
) -> Result<glib::Variant, glib::Error> {
    conn.call_future(
        Some(dest),
        path,
        iface,
        method,
        Some(&args),
        None,
        gio::DBusCallFlags::NO_AUTO_START,
        1500,
    )
    .await
}
fn spawn(f: impl std::future::Future<Output = ()> + 'static) {
    glib::MainContext::ref_thread_default().spawn_local(f);
}
struct Entry {
    owner: String,
    path: String,
    iface: String,
    item: TrayItem,
    ready: bool,
    pending: bool,
    again: bool,
    origins: HashSet<(String, String)>,
    serial: u64,
    menu: menu::EntryMenu,
}
struct State {
    conn: gio::DBusConnection,
    revision: u64,
    items: BTreeMap<String, Entry>,
    hosts: HashSet<String>,
    owned: HashSet<String>,
    external: HashMap<String, String>,
    watcher_versions: HashMap<String, u64>,
    owner_leases: HashMap<String, (u64, u32)>,
    next_entry: u64,
    next_menu_token: u64,
}
type Shared = Rc<RefCell<State>>;
impl State {
    fn lease(&mut self, name: &str) -> u64 {
        let v = self.owner_leases.entry(name.into()).or_insert((0, 0));
        v.1 += 1;
        v.0
    }
    fn unlease(&mut self, name: &str) {
        if let Some(v) = self.owner_leases.get_mut(name) {
            v.1 -= 1;
            if v.1 == 0 {
                self.owner_leases.remove(name);
            }
        }
    }

    fn changed(&mut self) {
        self.revision = self.revision.wrapping_add(1);
        let _ = self.conn.emit_signal(
            None,
            PATH,
            API,
            "Changed",
            Some(&(self.revision,).to_variant()),
        );
    }
    fn watcher_signal(&self, name: &str, args: &glib::Variant) {
        for iface in &self.owned {
            let _ = self
                .conn
                .emit_signal(None, WATCH_PATH, iface, name, Some(args));
        }
    }
    fn remove(&mut self, key: &str) {
        if self.items.remove(key).is_some() {
            self.watcher_signal("StatusNotifierItemUnregistered", &(key,).to_variant());
            self.changed();
        }
    }
}
fn text(map: &HashMap<String, glib::Variant>, name: &str) -> String {
    map.get(name)
        .and_then(|v| v.get::<String>())
        .unwrap_or_default()
        .chars()
        .take(512)
        .collect()
}
fn decode(key: &str, map: &HashMap<String, glib::Variant>) -> TrayItem {
    let mut item = TrayItem {
        key: key.into(),
        title: text(map, "Title"),
        status: text(map, "Status"),
        ..TrayItem::default()
    };
    if item.status.is_empty() {
        item.status = "Active".into();
    }
    let attention = item.status == "NeedsAttention";
    item.icon_name = text(
        map,
        if attention {
            "AttentionIconName"
        } else {
            "IconName"
        },
    );
    if item.icon_name.is_empty() {
        item.icon_name = text(map, "IconName");
    }
    item.item_is_menu = map.get("ItemIsMenu").and_then(|v| v.get()).unwrap_or(false);
    if let Some(t) = map.get("ToolTip")
        && t.type_().as_str() == "(sa(iiay)ss)"
    {
        let title = t.child_value(2).get::<String>().unwrap_or_default();
        let body = t.child_value(3).get::<String>().unwrap_or_default();
        item.tooltip = format!("{title}\n{body}")
            .trim()
            .chars()
            .take(512)
            .collect();
    }
    if item.tooltip.is_empty() {
        item.tooltip = item.title.clone();
    }
    let pixmap = if attention {
        map.get("AttentionIconPixmap")
            .filter(|v| v.type_().as_str() == "a(iiay)" && v.n_children() > 0)
            .or_else(|| map.get("IconPixmap"))
    } else {
        map.get("IconPixmap")
    };
    if let Some(pixmap) = pixmap
        && pixmap.type_().as_str() == "a(iiay)"
    {
        let mut candidates = Vec::new();
        for p in pixmap.iter().take(16) {
            let w = p.child_value(0).get::<i32>().unwrap_or(0);
            let h = p.child_value(1).get::<i32>().unwrap_or(0);
            let bytes = p.child_value(2);
            if let Ok(bytes) = bytes.fixed_array::<u8>()
                && let Some(rgba) = tray_model::rgba(w, h, bytes)
            {
                candidates.push((w, h, rgba));
            }
        }
        candidates.sort_by_key(|(w, _, _)| (w - 24).abs());
        if let Some((w, h, p)) = candidates.into_iter().next() {
            item.width = w;
            item.height = h;
            item.pixels = p;
        }
    }
    item
}
fn refresh(state: &Shared, key: &str) {
    let (conn, owner, path, serial) = {
        let mut s = state.borrow_mut();
        let conn = s.conn.clone();
        let Some(e) = s.items.get_mut(key) else {
            return;
        };
        if e.pending {
            e.again = true;
            return;
        }
        e.pending = true;
        (conn, e.owner.clone(), e.path.clone(), e.serial)
    };
    let state = state.clone();
    let key = key.to_owned();
    spawn(async move {
        let mut result = None;
        for iface in [
            "org.kde.StatusNotifierItem",
            "org.freedesktop.StatusNotifierItem",
        ] {
            if let Ok(v) = call(
                &conn,
                &owner,
                &path,
                "org.freedesktop.DBus.Properties",
                "GetAll",
                (iface,).to_variant(),
            )
            .await
                && v.type_().as_str() == "(a{sv})"
                && v.size() <= 1_048_576
                && let Some(map) = v.child_value(0).get::<HashMap<String, glib::Variant>>()
                && !map.is_empty()
            {
                result = Some((iface.to_string(), decode(&key, &map), menu::path(&map)));
                break;
            }
        }
        let again = {
            let mut s = state.borrow_mut();
            let Some(e) = s.items.get_mut(&key) else {
                return;
            };
            if e.serial != serial {
                return;
            }
            e.pending = false;
            let again = e.again;
            e.again = false;
            if let Some((iface, item, path)) = result {
                let menu_changed = e.menu.path != path;
                if menu_changed {
                    e.menu.submenus.clear();
                }
                e.menu.path = path;
                let changed = !e.ready || e.item != item;
                e.item = item;
                e.iface = iface;
                e.ready = true;
                if changed {
                    s.changed();
                }
                if menu_changed {
                    menu::invalidate(&mut s, &key);
                }
            }
            again
        };
        if again {
            refresh(&state, &key);
        }
    });
}
async fn owner(conn: &gio::DBusConnection, name: &str) -> Result<String, glib::Error> {
    let v = call(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
        "GetNameOwner",
        (name,).to_variant(),
    )
    .await?;
    Ok(v.child_value(0).get().unwrap_or_default())
}
#[derive(Clone)]
struct WatchToken {
    name: String,
    owner: String,
    version: u64,
}
impl WatchToken {
    fn valid(&self, s: &State) -> bool {
        s.external.get(&self.name) == Some(&self.owner)
            && s.watcher_versions.get(&self.name) == Some(&self.version)
    }
}
async fn register(
    state: &Shared,
    sender: &str,
    value: &str,
    _local: bool,
    _source: &str,
) -> Result<(), String> {
    let (service, path) = tray_model::registration(sender, value).map_err(str::to_owned)?;
    register_target(state, service, path, None).await
}
async fn register_target(
    state: &Shared,
    service: String,
    path: String,
    token: Option<WatchToken>,
) -> Result<(), String> {
    tray_model::registration(&service, &path).map_err(str::to_owned)?;
    let conn = state.borrow().conn.clone();
    let version = state.borrow_mut().lease(&service);
    let result = async {
        let unique = owner(&conn, &service)
            .await
            .map_err(|_| "service not running")?;
        let unique_version = state.borrow_mut().lease(&unique);
        let alive = owner(&conn, &unique).await.is_ok();
        let fresh = {
            let mut s = state.borrow_mut();
            let fresh = s.owner_leases.get(&service).is_some_and(|v| v.0 == version)
                && s.owner_leases
                    .get(&unique)
                    .is_some_and(|v| v.0 == unique_version);
            s.unlease(&unique);
            fresh
        };
        if !alive || !fresh {
            return Err("registration owner changed".into());
        }
        let key = format!("{unique}{path}");
        {
            let mut s = state.borrow_mut();
            if token.as_ref().is_some_and(|t| !t.valid(&s)) {
                return Err("watcher changed".into());
            }
            let origin = (
                token.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
                service.clone(),
            );
            if let Some(e) = s.items.get_mut(&key) {
                e.origins.insert(origin);
                return Ok(());
            }
            if s.items.len() >= 128 {
                return Err("tray item limit".into());
            }
            s.next_entry += 1;
            let serial = s.next_entry;
            s.items.insert(
                key.clone(),
                Entry {
                    owner: unique,
                    path,
                    iface: "org.kde.StatusNotifierItem".into(),
                    item: TrayItem {
                        key: key.clone(),
                        ..TrayItem::default()
                    },
                    ready: false,
                    pending: false,
                    again: false,
                    origins: HashSet::from([origin]),
                    serial,
                    menu: menu::EntryMenu::default(),
                },
            );
            s.watcher_signal("StatusNotifierItemRegistered", &(&key,).to_variant());
        }
        refresh(state, &key);
        Ok(())
    }
    .await;
    state.borrow_mut().unlease(&service);
    result
}
fn setup_watcher(state: &Shared, name: &str) {
    let state = state.clone();
    let name = name.to_string();
    let version = {
        let mut s = state.borrow_mut();
        let v = s.watcher_versions.entry(name.clone()).or_default();
        *v += 1;
        *v
    };
    spawn(async move {
        let conn = state.borrow().conn.clone();
        let result = call(
            &conn,
            "org.freedesktop.DBus",
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "RequestName",
            (&name, 4u32).to_variant(),
        )
        .await;
        let ours = result
            .ok()
            .and_then(|v| v.child_value(0).get::<u32>())
            .is_some_and(|v| v == 1 || v == 4);
        if ours {
            state.borrow_mut().owned.insert(name);
            return;
        }
        let Ok(unique) = owner(&conn, &name).await else {
            return;
        };
        if state.borrow().watcher_versions.get(&name) != Some(&version) {
            return;
        }
        state
            .borrow_mut()
            .external
            .insert(name.clone(), unique.clone());
        let token = WatchToken {
            name: name.clone(),
            owner: unique.clone(),
            version,
        };
        let host = format!("org.kde.StatusNotifierHost-{}", std::process::id());
        let _ = call(
            &conn,
            &unique,
            WATCH_PATH,
            &name,
            "RegisterStatusNotifierHost",
            (&host,).to_variant(),
        )
        .await;
        if !token.valid(&state.borrow()) {
            return;
        }
        if let Ok(list) = call(
            &conn,
            &unique,
            WATCH_PATH,
            "org.freedesktop.DBus.Properties",
            "Get",
            (&name, "RegisteredStatusNotifierItems").to_variant(),
        )
        .await
            && token.valid(&state.borrow())
            && list.type_().as_str() == "(v)"
            && let Some(items) = list
                .child_value(0)
                .as_variant()
                .and_then(|v| v.get::<Vec<String>>())
        {
            for item in items.into_iter().take(128) {
                import(&state, &token, &item).await;
            }
        }
    });
}
async fn import(state: &Shared, token: &WatchToken, value: &str) {
    let (name, path) = value
        .find('/')
        .map(|i| (&value[..i], &value[i..]))
        .unwrap_or((value, "/StatusNotifierItem"));
    if !name.is_empty() && token.valid(&state.borrow()) {
        let _ = register_target(state, name.into(), path.into(), Some(token.clone())).await;
    }
}
fn unregister(state: &Shared, source: &str, value: &str) {
    let (name, path) = value
        .find('/')
        .map(|i| (&value[..i], &value[i..]))
        .unwrap_or((value, "/StatusNotifierItem"));
    let mut s = state.borrow_mut();
    let mut removed = Vec::new();
    for (key, e) in s.items.iter_mut() {
        if e.path == path {
            let unique = e.owner == name;
            e.origins
                .retain(|(origin, alias)| origin != source || (!unique && alias != name));
            if e.origins.is_empty() {
                removed.push(key.clone());
            }
        }
    }
    for key in removed {
        s.remove(&key);
    }
}
fn fail(inv: gio::DBusMethodInvocation, message: &str) {
    inv.return_dbus_error("org.mosaic.Error1.Failed", message);
}
fn register_objects(state: &Shared) -> Result<Vec<gio::RegistrationId>, glib::Error> {
    let conn = state.borrow().conn.clone();
    let mut registrations = Vec::new();
    for interface in WATCHERS {
        let info = gio::DBusNodeInfo::for_xml(&watcher_xml(interface))?
            .lookup_interface(interface)
            .expect("static XML");
        let method_state = state.clone();
        let property_state = state.clone();
        registrations.push(
            conn.register_object(WATCH_PATH, &info)
                .method_call(move |_, sender, _, _, method, args, inv| {
                    let state = method_state.clone();
                    let sender = sender.unwrap_or("").to_string();
                    let method = method.to_owned();
                    spawn(async move {
                        let Some((value,)) = args.get::<(String,)>() else {
                            fail(inv, "invalid arguments");
                            return;
                        };
                        let result = if method == "RegisterStatusNotifierItem" {
                            register(&state, &sender, &value, true, "").await
                        } else {
                            let conn = state.borrow().conn.clone();
                            match owner(&conn, &value).await {
                                Ok(unique) => {
                                    state.borrow_mut().hosts.insert(unique);
                                    state.borrow().watcher_signal(
                                        "StatusNotifierHostRegistered",
                                        &().to_variant(),
                                    );
                                    Ok(())
                                }
                                Err(_) => Err("host not running".into()),
                            }
                        };
                        match result {
                            Ok(()) => inv.return_value(Some(&().to_variant())),
                            Err(e) => fail(inv, &e),
                        }
                    });
                })
                .property(move |_, _, _, _, property| {
                    let s = property_state.borrow();
                    match property {
                        "RegisteredStatusNotifierItems" => {
                            s.items.keys().cloned().collect::<Vec<_>>().to_variant()
                        }
                        "IsStatusNotifierHostRegistered" => (!s.hosts.is_empty()).to_variant(),
                        _ => 0i32.to_variant(),
                    }
                })
                .build()?,
        );
    }
    let info = gio::DBusNodeInfo::for_xml(XML)?
        .lookup_interface(API)
        .expect("static XML");
    let state = state.clone();
    registrations.push(
        conn.register_object(PATH, &info)
            .method_call(move |_, _, _, _, method, args, inv| {
                if method == "GetSnapshot" {
                    let s = state.borrow();
                    let items: Vec<Wire> = s
                        .items
                        .values()
                        .filter(|e| e.ready)
                        .map(|e| e.item.wire())
                        .collect();
                    inv.return_value(Some(&(s.revision, items).to_variant()));
                    return;
                }
                if method == "GetMenu" || method == "MenuEvent" {
                    menu::method(&state, method, &args, inv);
                    return;
                }
                let Some((key, action, delta, orientation)) =
                    args.get::<(String, String, i32, String)>()
                else {
                    fail(inv, "invalid arguments");
                    return;
                };
                let s = state.borrow();
                let Some(e) = s.items.get(&key) else {
                    fail(inv, "stale item");
                    return;
                };
                let (method, params) = match action.as_str() {
                    "activate" => ("Activate", (0i32, 0i32).to_variant()),
                    "secondary" => ("SecondaryActivate", (0i32, 0i32).to_variant()),
                    "context" => ("ContextMenu", (0i32, 0i32).to_variant()),
                    "scroll" if ["vertical", "horizontal"].contains(&orientation.as_str()) => (
                        "Scroll",
                        (delta.clamp(-32768, 32768), orientation).to_variant(),
                    ),
                    _ => {
                        fail(inv, "invalid action");
                        return;
                    }
                };
                let conn = s.conn.clone();
                let owner = e.owner.clone();
                let path = e.path.clone();
                let iface = e.iface.clone();
                drop(s);
                spawn(async move {
                    match call(&conn, &owner, &path, &iface, method, params).await {
                        Ok(_) => inv.return_value(Some(&().to_variant())),
                        Err(_) => fail(inv, "item action failed or timed out"),
                    }
                });
            })
            .build()?,
    );
    Ok(registrations)
}
pub struct Runtime {
    loop_: glib::MainLoop,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Runtime {
    pub fn start() -> Result<Self, Box<dyn std::error::Error>> {
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let context = glib::MainContext::new();
            let _ = context.with_thread_default(|| -> Result<(), glib::Error> {
                let conn = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE)?;
                let state = Rc::new(RefCell::new(State {
                    conn: conn.clone(),
                    revision: 0,
                    items: BTreeMap::new(),
                    hosts: HashSet::from([conn.unique_name().unwrap_or_default().to_string()]),
                    owned: HashSet::new(),
                    external: HashMap::new(),
                    watcher_versions: HashMap::new(),
                    owner_leases: HashMap::new(),
                    next_entry: 0,
                    next_menu_token: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as u64,
                }));
                let _registrations = register_objects(&state)?;
                // Startup calls run only on this worker, never on the GTK thread.
                for name in [
                    API.to_string(),
                    format!("org.kde.StatusNotifierHost-{}", std::process::id()),
                ] {
                    let result = conn.call_sync(
                        Some("org.freedesktop.DBus"),
                        "/org/freedesktop/DBus",
                        "org.freedesktop.DBus",
                        "RequestName",
                        Some(&(name, 4u32).to_variant()),
                        None,
                        gio::DBusCallFlags::NONE,
                        1500,
                        gio::Cancellable::NONE,
                    )?;
                    if !matches!(result.child_value(0).get::<u32>(), Some(1 | 4)) {
                        return Err(glib::Error::new(
                            gio::IOErrorEnum::Exists,
                            "tray name already owned",
                        ));
                    }
                }
                let changed_state = state.clone();
                let _signals = conn.subscribe_to_signal(
                    None,
                    None,
                    None,
                    None,
                    None,
                    gio::DBusSignalFlags::NONE,
                    move |signal| {
                        let s = &changed_state;
                        if signal.sender_name == "org.freedesktop.DBus"
                            && signal.object_path == "/org/freedesktop/DBus"
                            && signal.interface_name == "org.freedesktop.DBus"
                            && signal.signal_name == "NameOwnerChanged"
                        {
                            if let Some((name, old, new)) =
                                signal.parameters.get::<(String, String, String)>()
                            {
                                {
                                    let mut state = s.borrow_mut();
                                    if let Some(v) = state.owner_leases.get_mut(&name) {
                                        v.0 += 1;
                                    }
                                    if WATCHERS.contains(&name.as_str()) {
                                        *state.watcher_versions.entry(name.clone()).or_default() +=
                                            1;
                                        state.external.remove(&name);
                                        state.owned.remove(&name);
                                    }
                                    let mut removed = Vec::new();
                                    for (key, e) in state.items.iter_mut() {
                                        e.origins.retain(|(source, alias)| {
                                            source != &name && !(e.owner == old && alias == &name)
                                        });
                                        if (e.owner == name && new.is_empty())
                                            || e.origins.is_empty()
                                        {
                                            removed.push(key.clone());
                                        }
                                    }
                                    for key in removed {
                                        state.remove(&key);
                                    }
                                    state.hosts.remove(&name);
                                }
                                if WATCHERS.contains(&name.as_str()) {
                                    let ours = s.borrow().conn.unique_name().as_deref()
                                        == Some(new.as_str());
                                    if ours {
                                        s.borrow_mut().owned.insert(name);
                                    } else {
                                        setup_watcher(s, &name);
                                    }
                                }
                            }
                        } else if WATCHERS.contains(&signal.interface_name)
                            && s.borrow()
                                .external
                                .get(signal.interface_name)
                                .is_some_and(|v| v == signal.sender_name)
                        {
                            if let Some((item,)) = signal.parameters.get::<(String,)>() {
                                let token = WatchToken {
                                    name: signal.interface_name.into(),
                                    owner: signal.sender_name.into(),
                                    version: *s
                                        .borrow()
                                        .watcher_versions
                                        .get(signal.interface_name)
                                        .unwrap_or(&0),
                                };
                                if signal.signal_name == "StatusNotifierItemRegistered" {
                                    let state = s.clone();
                                    spawn(async move {
                                        import(&state, &token, &item).await;
                                    });
                                } else if signal.signal_name == "StatusNotifierItemUnregistered" {
                                    unregister(s, signal.interface_name, &item);
                                }
                            }
                        } else if signal.interface_name == crate::menu_model::INTERFACE
                            && ["LayoutUpdated", "ItemsPropertiesUpdated"]
                                .contains(&signal.signal_name)
                        {
                            menu::signal(s, signal.sender_name, signal.object_path);
                        } else if signal.interface_name == "org.freedesktop.DBus.Properties"
                            || signal.interface_name.ends_with("StatusNotifierItem")
                        {
                            let keys: Vec<_> = s
                                .borrow()
                                .items
                                .iter()
                                .filter(|(_, e)| {
                                    e.owner == signal.sender_name && e.path == signal.object_path
                                })
                                .map(|(k, _)| k.clone())
                                .collect();
                            for key in keys {
                                refresh(s, &key);
                            }
                        }
                    },
                );
                for name in WATCHERS {
                    setup_watcher(&state, name);
                }
                let loop_ = glib::MainLoop::new(Some(&context), false);
                let _ = tx.send(loop_.clone());
                loop_.run();
                let _ = conn.close_sync(gio::Cancellable::NONE);
                Ok(())
            });
        });
        let loop_ = rx.recv_timeout(std::time::Duration::from_secs(5))?;
        Ok(Self {
            loop_,
            thread: Some(thread),
        })
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        self.loop_.quit();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn malformed_optional_values_do_not_panic() {
        let map = HashMap::from([
            ("Title".into(), "Fallback".to_variant()),
            ("ToolTip".into(), 42i32.to_variant()),
            ("AttentionIconPixmap".into(), false.to_variant()),
            ("Status".into(), "NeedsAttention".to_variant()),
        ]);
        let item = decode(":1.2/Item", &map);
        assert_eq!(item.tooltip, "Fallback");
        assert!(item.pixels.is_empty());
    }
}
