//! Bounded, typed DBusMenu projection. Remote markup is never evaluated.
use gtk::glib::{
    self,
    variant::{FromVariant, ToVariant},
};
use std::collections::{HashMap, HashSet};
pub const INTERFACE: &str = "com.canonical.dbusmenu";
pub type RowWire = (
    i32,
    i32,
    String,
    bool,
    bool,
    String,
    String,
    i32,
    String,
    bool,
);
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct MenuRow {
    pub id: i32,
    pub parent: i32,
    pub label: String,
    pub enabled: bool,
    pub visible: bool,
    pub kind: String,
    pub toggle_type: String,
    pub toggle_state: i32,
    pub icon_name: String,
    pub submenu: bool,
}
impl MenuRow {
    pub fn wire(&self) -> RowWire {
        (
            self.id,
            self.parent,
            self.label.clone(),
            self.enabled,
            self.visible,
            self.kind.clone(),
            self.toggle_type.clone(),
            self.toggle_state,
            self.icon_name.clone(),
            self.submenu,
        )
    }
    pub fn from_wire(w: RowWire) -> Self {
        Self {
            id: w.0,
            parent: w.1,
            label: w.2,
            enabled: w.3,
            visible: w.4,
            kind: w.5,
            toggle_type: w.6,
            toggle_state: w.7,
            icon_name: w.8,
            submenu: w.9,
        }
    }
}
#[derive(Clone, Debug, serde::Serialize)]
pub struct MenuSnapshot {
    pub token: u64,
    pub revision: u32,
    pub rows: Vec<MenuRow>,
}
impl MenuSnapshot {
    pub fn can_click(&self, token: u64, id: i32) -> bool {
        self.token == token
            && id != 0
            && self
                .rows
                .iter()
                .any(|r| r.id == id && r.enabled && r.visible && r.kind == "standard" && !r.submenu)
    }
}
fn prop<T: FromVariant>(
    p: &HashMap<String, glib::Variant>,
    key: &str,
) -> Result<Option<T>, String> {
    p.get(key)
        .map(|v| v.get::<T>().ok_or_else(|| format!("invalid {key}")))
        .transpose()
}
fn plain_label(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '_' {
            if chars.peek() == Some(&'_') {
                chars.next();
                result.push('_');
            }
        } else {
            result.push(c);
        }
    }
    result.chars().take(512).collect()
}
pub fn decode(reply: &glib::Variant) -> Result<(u32, Vec<MenuRow>), String> {
    if reply.type_().as_str() != "(u(ia{sv}av))" || reply.size() > 1_048_576 {
        return Err("invalid or oversized menu".into());
    }
    fn walk(
        v: &glib::Variant,
        parent: i32,
        depth: usize,
        ancestor_enabled: bool,
        ancestor_visible: bool,
        rows: &mut Vec<MenuRow>,
        ids: &mut HashSet<i32>,
    ) -> Result<(), String> {
        if depth > 8 || rows.len() >= 256 {
            return Err("menu bounds exceeded".into());
        }
        let (id, p, children) = v
            .get::<(i32, HashMap<String, glib::Variant>, Vec<glib::Variant>)>()
            .ok_or("invalid menu node")?;
        if !ids.insert(id) {
            return Err("duplicate menu ID".into());
        }
        if depth == 0 && id != 0 {
            return Err("invalid menu root".into());
        }
        let kind = prop::<String>(&p, "type")?.unwrap_or_else(|| "standard".into());
        let enabled =
            ancestor_enabled && prop::<bool>(&p, "enabled")?.unwrap_or(true) && kind == "standard";
        let visible = ancestor_visible && prop::<bool>(&p, "visible")?.unwrap_or(true);
        let row = MenuRow {
            id,
            parent,
            label: plain_label(&prop::<String>(&p, "label")?.unwrap_or_default()),
            enabled,
            visible,
            kind: kind.chars().take(64).collect(),
            toggle_type: prop::<String>(&p, "toggle-type")?
                .unwrap_or_default()
                .chars()
                .take(32)
                .collect(),
            toggle_state: prop::<i32>(&p, "toggle-state")?.unwrap_or(-1),
            icon_name: prop::<String>(&p, "icon-name")?
                .unwrap_or_default()
                .chars()
                .take(256)
                .collect(),
            submenu: prop::<String>(&p, "children-display")?.as_deref() == Some("submenu")
                || !children.is_empty(),
        };
        rows.push(row);
        for child in children {
            walk(&child, id, depth + 1, enabled, visible, rows, ids)?;
        }
        Ok(())
    }
    let mut rows = Vec::new();
    walk(
        &reply.child_value(1),
        -1,
        0,
        true,
        true,
        &mut rows,
        &mut HashSet::new(),
    )?;
    Ok((reply.child_value(0).get().unwrap_or(0), rows))
}
pub fn event_args(id: i32, event: &str, time: u32) -> glib::Variant {
    (id, event, 0i32.to_variant(), time).to_variant()
}
