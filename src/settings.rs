use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, path::Path};
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    pub schema_version: u32,
    pub revision: u64,
    pub pins: Vec<String>,
    #[serde(default)]
    pub general: General,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub dock: Dock,
    #[serde(default)]
    pub panel: Panel,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub dock_icons: BTreeMap<String, String>,
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PanelItem {
    Menu,
    Active,
    Status,
    Spacer,
    Tray,
    Clock,
    Settings,
    Wifi,
    Bluetooth,
    Brightness,
    Audio,
    Appearance,
    Dock,
    Temperatures,
    Power,
}
impl PanelItem {
    pub const BUILT_IN: [Self; 7] = [
        Self::Menu,
        Self::Active,
        Self::Status,
        Self::Spacer,
        Self::Tray,
        Self::Clock,
        Self::Settings,
    ];

    pub const SHORTCUTS: [Self; 8] = [
        Self::Wifi,
        Self::Bluetooth,
        Self::Brightness,
        Self::Audio,
        Self::Appearance,
        Self::Dock,
        Self::Temperatures,
        Self::Power,
    ];

    pub fn can_hide(self) -> bool {
        matches!(self, Self::Active | Self::Status | Self::Tray) || Self::SHORTCUTS.contains(&self)
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct Panel {
    pub order: Vec<PanelItem>,
    pub hidden: Vec<PanelItem>,
}

impl Panel {
    pub fn is_visible(&self, item: PanelItem) -> bool {
        self.order.contains(&item) && !self.hidden.contains(&item)
    }
}

impl Default for Panel {
    fn default() -> Self {
        Self {
            order: PanelItem::BUILT_IN.to_vec(),
            hidden: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct General {
    pub reduced_motion: bool,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ThemeMode {
    #[default]
    Dark,
    Light,
    System,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct Theme {
    pub mode: ThemeMode,
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DockEdge {
    #[default]
    Bottom,
    Left,
    Right,
}
impl DockEdge {
    pub fn vertical(self) -> bool {
        self != Self::Bottom
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct Dock {
    pub edge: DockEdge,
    pub icon_size: u32,
    pub preferred_output: String,
    pub current_output_only: bool,
    pub all_outputs: bool,
    pub magnification: bool,
    pub hover: DockHover,
    pub auto_hide: bool,
    pub show_delay_ms: u64,
    pub hide_delay_ms: u64,
    pub minimize_on_click: bool,
}
impl Default for Dock {
    fn default() -> Self {
        Self {
            edge: DockEdge::Bottom,
            icon_size: 48,
            preferred_output: String::new(),
            current_output_only: false,
            all_outputs: false,
            magnification: true,
            hover: DockHover::default(),
            auto_hide: false,
            show_delay_ms: 150,
            hide_delay_ms: 700,
            minimize_on_click: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HoverEasing {
    Linear,
    #[default]
    EaseOut,
    EaseInOut,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, default)]
pub struct DockHover {
    pub zoom_percent: u32,
    pub radius_percent: u32,
    pub grow_ms: u32,
    pub shrink_ms: u32,
    pub easing: HoverEasing,
    pub cards_enabled: bool,
    pub cards_show_delay_ms: u32,
    pub cards_hide_delay_ms: u32,
}

impl Default for DockHover {
    fn default() -> Self {
        Self {
            zoom_percent: 45,
            radius_percent: 65,
            grow_ms: 200,
            shrink_ms: 200,
            easing: HoverEasing::EaseOut,
            cards_enabled: true,
            cards_show_delay_ms: 450,
            cards_hide_delay_ms: 240,
        }
    }
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            schema_version: 2,
            revision: 0,
            pins: vec!["firefox.desktop".into(), "org.xfce.Thunar.desktop".into()],
            general: General::default(),
            theme: Theme::default(),
            dock: Dock::default(),
            panel: Panel::default(),
            dock_icons: BTreeMap::new(),
        }
    }
}
fn validate(s: &Settings) -> io::Result<()> {
    if s.schema_version != 2
        || s.pins.len() > 256
        || s.pins.iter().any(|p| {
            p.len() > 256
                || !p.ends_with(".desktop")
                || p.contains('/')
                || p.chars().any(char::is_control)
        })
    {
        return Err(io::Error::other("unsupported schema or invalid pins"));
    }
    if s.dock_icons.len() > 128
        || s.dock_icons.iter().any(|(key, asset)| {
            !crate::dock_icons::valid_key(key) || !crate::dock_icons::valid_asset(asset)
        })
    {
        return Err(io::Error::other("invalid Dock icon overrides"));
    }
    if s.dock.preferred_output.len() > 256 || s.dock.preferred_output.chars().any(char::is_control)
    {
        return Err(io::Error::other("invalid preferred output"));
    }
    if !(32..=80).contains(&s.dock.icon_size)
        || s.dock.show_delay_ms > 2000
        || !(100..=5000).contains(&s.dock.hide_delay_ms)
    {
        return Err(io::Error::other("dock value out of range"));
    }
    let hover = &s.dock.hover;
    if hover.zoom_percent > 70
        || !(25..=200).contains(&hover.radius_percent)
        || hover.grow_ms > 1_000
        || hover.shrink_ms > 1_000
        || hover.cards_show_delay_ms > 2_000
        || hover.cards_hide_delay_ms > 2_000
    {
        return Err(io::Error::other("dock hover value out of range"));
    }
    if PanelItem::BUILT_IN
        .iter()
        .any(|item| !s.panel.order.contains(item))
        || s.panel
            .order
            .iter()
            .enumerate()
            .any(|(i, item)| s.panel.order[..i].contains(item))
        || s.panel
            .hidden
            .iter()
            .enumerate()
            .any(|(i, item)| !item.can_hide() || s.panel.hidden[..i].contains(item))
    {
        return Err(io::Error::other("invalid panel layout"));
    }
    Ok(())
}
pub fn patched(base: &Settings, patch: &str) -> io::Result<Settings> {
    if patch.len() > 8192 {
        return Err(io::Error::other("patch exceeds 8 KiB"));
    }
    let mut patch: toml::Table =
        toml::from_str(patch).map_err(|_| io::Error::other("invalid TOML patch"))?;
    if patch.keys().any(|key| {
        !matches!(
            key.as_str(),
            "pins" | "general" | "theme" | "dock" | "panel" | "dock_icons"
        )
    }) {
        return Err(io::Error::other("unknown or read-only patch field"));
    }
    fn merge(target: &mut toml::Value, value: toml::Value) {
        if let (Some(dst), Some(src)) = (target.as_table_mut(), value.as_table()) {
            for (key, value) in src {
                if let Some(old) = dst.get_mut(key) {
                    merge(old, value.clone());
                } else {
                    dst.insert(key.clone(), value.clone());
                }
            }
        } else {
            *target = value;
        }
    }
    let icons = patch.remove("dock_icons");
    let mut value = toml::Value::try_from(base).map_err(io::Error::other)?;
    if let Some(icons) = icons {
        value
            .as_table_mut()
            .ok_or_else(|| io::Error::other("invalid config table"))?
            .insert("dock_icons".into(), icons);
    }
    merge(&mut value, toml::Value::Table(patch));
    let result: Settings = value.try_into().map_err(io::Error::other)?;
    validate(&result)?;
    Ok(result)
}
pub fn load(path: &Path) -> io::Result<Settings> {
    use io::Read;
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Settings::default()),
        Err(e) => return Err(e),
    };
    let mut text = String::new();
    file.take(65537).read_to_string(&mut text)?;
    if text.len() > 65536 {
        return Err(io::Error::other("config exceeds 64 KiB"));
    }
    let mut value: toml::Value =
        toml::from_str(&text).map_err(|_| io::Error::other("invalid config schema"))?;
    if value
        .get("schema_version")
        .and_then(toml::Value::as_integer)
        == Some(1)
    {
        let table = value
            .as_table_mut()
            .ok_or_else(|| io::Error::other("invalid config table"))?;
        if table
            .keys()
            .any(|key| !matches!(key.as_str(), "schema_version" | "revision" | "pins"))
        {
            return Err(io::Error::other("unknown schema 1 field"));
        }
        table.insert("schema_version".into(), toml::Value::Integer(2));
    }
    let s: Settings = value
        .try_into()
        .map_err(|_| io::Error::other("invalid config schema"))?;
    validate(&s)?;
    Ok(s)
}
pub fn save(path: &Path, value: &Settings) -> io::Result<Settings> {
    use std::{
        fs::OpenOptions,
        io::Write,
        os::unix::fs::{OpenOptionsExt, PermissionsExt},
    };
    validate(value)?;
    if path.is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "managed config is read-only",
        ));
    }
    if load(path)?.revision != value.revision {
        return Err(io::Error::other("config revision conflict"));
    }
    // Preserve the original legacy file before the first schema-2 write.
    if path.exists() {
        use io::Read;
        let mut old = String::new();
        std::fs::File::open(path)?
            .take(65537)
            .read_to_string(&mut old)?;
        let raw: toml::Value = toml::from_str(&old).map_err(io::Error::other)?;
        if raw.get("schema_version").and_then(toml::Value::as_integer) == Some(1) {
            let backup = path.with_file_name(format!(
                "{}.v1.bak",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            match OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&backup)
            {
                Ok(mut file) => {
                    file.write_all(old.as_bytes())?;
                    file.sync_all()?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if backup.is_symlink() {
                        return Err(io::Error::other("backup path is a symlink"));
                    }
                    let mut existing = String::new();
                    std::fs::File::open(&backup)?
                        .take(65537)
                        .read_to_string(&mut existing)?;
                    if existing != old {
                        return Err(io::Error::other("different legacy backup already exists"));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    let mut next = value.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .ok_or_else(|| io::Error::other("revision overflow"))?;
    let serialized = toml::to_string(&next).map_err(io::Error::other)?;
    if serialized.len() > 65536 {
        return Err(io::Error::other("config exceeds 64 KiB"));
    }
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::other("config path requires parent"))?;
    if !parent.exists() {
        std::fs::create_dir_all(parent)?;
        std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))?;
    }
    let tmp = parent.join(format!(
        ".mosaic-config-{}-{}.tmp",
        std::process::id(),
        next.revision
    ));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&tmp)?;
    let result = (|| {
        file.write_all(serialized.as_bytes())?;
        file.sync_all()?;
        std::fs::rename(&tmp, path)?;
        std::fs::File::open(parent)?.sync_all()?;
        Ok(next)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(tmp);
    }
    result
}
