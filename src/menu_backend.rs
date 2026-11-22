//! Child of tray_backend: all state remains on its GLib context.
use super::{Shared, State, call, fail, spawn};
use crate::menu_model::{self, MenuSnapshot, RowWire};
use glib::variant::ToVariant;
use gtk::{gio, glib};
use std::collections::HashMap;
#[derive(Default)]
pub(super) struct EntryMenu {
    pub path: String,
    pub epoch: u64,
    pub busy: bool,
    pub cache: Option<MenuSnapshot>,
    // IDs are only for AboutToShow; events always require a fresh cache token.
    pub submenus: std::collections::HashSet<i32>,
}
pub(super) fn path(properties: &HashMap<String, glib::Variant>) -> String {
    properties
        .get("Menu")
        .filter(|v| v.type_().as_str() == "o")
        .and_then(|v| v.str())
        .filter(|v| *v != "/" && v.len() <= 255)
        .unwrap_or_default()
        .into()
}
pub(super) fn invalidate(state: &mut State, key: &str) {
    if let Some(e) = state.items.get_mut(key) {
        e.menu.epoch = e.menu.epoch.wrapping_add(1);
        e.menu.cache = None;
    }
    let _ = state.conn.emit_signal(
        None,
        super::PATH,
        super::API,
        "MenuChanged",
        Some(&(key,).to_variant()),
    );
}
pub(super) fn signal(state: &Shared, sender: &str, path: &str) {
    let keys: Vec<_> = state
        .borrow()
        .items
        .iter()
        .filter(|(_, e)| e.owner == sender && e.menu.path == path)
        .map(|(k, _)| k.clone())
        .collect();
    for key in keys {
        invalidate(&mut state.borrow_mut(), &key);
    }
}
async fn get(
    state: &Shared,
    key: &str,
    parent: i32,
    prepare: bool,
) -> Result<Option<MenuSnapshot>, String> {
    let (conn, owner, path, serial) = {
        let mut s = state.borrow_mut();
        let conn = s.conn.clone();
        let e = s.items.get_mut(key).ok_or("stale tray item")?;
        if e.menu.path.is_empty() {
            return Ok(None);
        }
        if e.menu.busy {
            return Err("menu request already pending".into());
        }
        if prepare && parent != 0 && !e.menu.submenus.contains(&parent) {
            return Err("stale or disabled submenu".into());
        }
        e.menu.busy = true;
        (conn, e.owner.clone(), e.menu.path.clone(), e.serial)
    };
    let result = async {
        if prepare {
            call(
                &conn,
                &owner,
                &path,
                menu_model::INTERFACE,
                "AboutToShow",
                (parent,).to_variant(),
            )
            .await
            .map_err(|_| "AboutToShow failed")?;
        }
        let epoch = {
            let s = state.borrow();
            let e = s
                .items
                .get(key)
                .filter(|e| e.serial == serial && e.menu.path == path)
                .ok_or("menu owner/path changed")?;
            e.menu.epoch
        };
        let properties = vec![
            "label",
            "enabled",
            "visible",
            "type",
            "children-display",
            "toggle-type",
            "toggle-state",
            "icon-name",
        ];
        let reply = call(
            &conn,
            &owner,
            &path,
            menu_model::INTERFACE,
            "GetLayout",
            (0i32, 8i32, properties).to_variant(),
        )
        .await
        .map_err(|_| "GetLayout failed")?;
        let (revision, rows) = menu_model::decode(&reply)?;
        let mut s = state.borrow_mut();
        let e = s
            .items
            .get(key)
            .filter(|e| e.serial == serial && e.menu.path == path && e.menu.epoch == epoch)
            .ok_or("menu changed during request")?;
        if let Some(c) = &e.menu.cache
            && c.revision == revision
            && c.rows == rows
        {
            return Ok(Some(c.clone()));
        }
        s.next_menu_token = s.next_menu_token.wrapping_add(1);
        let token = s.next_menu_token;
        let snapshot = MenuSnapshot {
            token,
            revision,
            rows,
        };
        let e = s.items.get_mut(key).ok_or("stale item")?;
        e.menu.submenus = snapshot
            .rows
            .iter()
            .filter(|r| r.enabled && r.visible && r.submenu)
            .map(|r| r.id)
            .collect();
        e.menu.cache = Some(snapshot.clone());
        Ok(Some(snapshot))
    }
    .await;
    if let Some(e) = state
        .borrow_mut()
        .items
        .get_mut(key)
        .filter(|e| e.serial == serial)
    {
        e.menu.busy = false;
    }
    result
}
pub(super) fn method(
    state: &Shared,
    method: &str,
    args: &glib::Variant,
    inv: gio::DBusMethodInvocation,
) -> bool {
    if method == "GetMenu" {
        let Some((key, parent, prepare)) = args.get::<(String, i32, bool)>() else {
            fail(inv, "invalid menu arguments");
            return true;
        };
        let state = state.clone();
        spawn(async move {
            match get(&state, &key, parent, prepare).await {
                Ok(Some(menu)) => inv.return_value(Some(
                    &(
                        true,
                        menu.token,
                        menu.revision,
                        menu.rows.iter().map(|r| r.wire()).collect::<Vec<RowWire>>(),
                    )
                        .to_variant(),
                )),
                Ok(None) => inv.return_value(Some(
                    &(false, 0u64, 0u32, Vec::<RowWire>::new()).to_variant(),
                )),
                Err(e) => fail(inv, &e),
            }
        });
        return true;
    }
    if method == "MenuEvent" {
        let Some((key, token, id, event, time)) = args.get::<(String, u64, i32, String, u32)>()
        else {
            fail(inv, "invalid event arguments");
            return true;
        };
        let data = {
            let s = state.borrow();
            s.items
                .get(&key)
                .and_then(|e| e.menu.cache.as_ref().map(|c| (e, c)))
                .and_then(|(e, c)| {
                    let valid = match event.as_str() {
                        "clicked" => c.can_click(token, id),
                        "opened" | "closed" => {
                            c.token == token
                                && c.rows.iter().any(|r| {
                                    r.id == id && r.visible && (id == 0 || r.submenu && r.enabled)
                                })
                        }
                        _ => false,
                    };
                    valid.then(|| (s.conn.clone(), e.owner.clone(), e.menu.path.clone()))
                })
        };
        let Some((conn, owner, path)) = data else {
            fail(inv, "stale, hidden or disabled menu event");
            return true;
        };
        spawn(async move {
            match call(
                &conn,
                &owner,
                &path,
                menu_model::INTERFACE,
                "Event",
                menu_model::event_args(id, &event, time),
            )
            .await
            {
                Ok(_) => inv.return_value(Some(&().to_variant())),
                Err(_) => fail(inv, "menu event failed"),
            }
        });
        return true;
    }
    // The caller routes only the two declared methods here.
    false
}
