use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{env, fs, path::PathBuf};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ConfigMode {
    Strict,
    #[default]
    Defaults,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub schema_version: u32,
    pub mode: ConfigMode,
    pub theme: String,
    pub reduced_motion: bool,
    pub dock: DockConfig,
    pub panel: PanelConfig,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DockConfig {
    pub icon_size: i32,
    pub margin: i32,
    pub pinned: Vec<String>,
    pub autohide: bool,
    pub show_delay_ms: u64,
    pub hide_delay_ms: u64,
    pub magnification: bool,
    pub magnification_factor: f64,
    pub max_width_ratio: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PanelConfig {
    pub height: i32,
}
impl Default for DockConfig {
    fn default() -> Self {
        Self {
            icon_size: 48,
            margin: 12,
            pinned: vec![],
            autohide: false,
            show_delay_ms: 150,
            hide_delay_ms: 700,
            magnification: true,
            magnification_factor: 1.45,
            max_width_ratio: 0.9,
        }
    }
}
impl Default for PanelConfig {
    fn default() -> Self {
        Self { height: 32 }
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: 1,
            mode: ConfigMode::Defaults,
            theme: "dark".into(),
            reduced_motion: false,
            dock: DockConfig::default(),
            panel: PanelConfig::default(),
        }
    }
}
impl Config {
    pub fn parse(text: &str) -> Result<Self> {
        let c: Self = toml::from_str(text).context("Некорректный TOML конфигурации")?;
        c.validate()?;
        Ok(c)
    }
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1 {
            bail!("schema_version: поддерживается только 1");
        }
        if !(32..=80).contains(&self.dock.icon_size) {
            bail!("dock.icon_size: требуется 32..80");
        }
        if !(0..=64).contains(&self.dock.margin) {
            bail!("dock.margin: требуется 0..64");
        }
        if self.dock.show_delay_ms > 5000 || !(100..=10000).contains(&self.dock.hide_delay_ms) {
            bail!("dock: show_delay_ms 0..5000, hide_delay_ms 100..10000");
        }
        if !self.dock.magnification_factor.is_finite()
            || !(1.0..=2.0).contains(&self.dock.magnification_factor)
        {
            bail!("dock.magnification_factor: требуется 1.0..2.0");
        }
        if !self.dock.max_width_ratio.is_finite()
            || !(0.25..=0.95).contains(&self.dock.max_width_ratio)
        {
            bail!("dock.max_width_ratio: требуется 0.25..0.95");
        }
        if !(24..=64).contains(&self.panel.height) {
            bail!("panel.height: требуется 24..64");
        }
        if !["dark", "light"].contains(&self.theme.as_str()) {
            bail!("theme: dark или light");
        }
        if self.dock.pinned.len() > 128
            || self
                .dock
                .pinned
                .iter()
                .any(|id| id.contains('/') || !id.ends_with(".desktop"))
        {
            bail!("dock.pinned: до 128 desktop ID без путей");
        }
        Ok(())
    }
    pub fn with_override(&self, text: &str) -> Result<Self> {
        if self.mode == ConfigMode::Strict {
            bail!("Конфигурация управляется декларативно (strict)");
        }
        let patch: toml::Value = toml::from_str(text)?;
        if patch.get("mode").is_some() || patch.get("schema_version").is_some() {
            bail!("Нельзя менять mode/schema_version через override");
        }
        let mut base = toml::Value::try_from(self)?;
        merge(&mut base, patch);
        Self::parse(&toml::to_string(&base)?)
    }
    pub fn path() -> Result<PathBuf> {
        if let Some(path) = env::var_os("MOSAIC_CONFIG") {
            return Ok(path.into());
        }
        Ok(xdg_dir("XDG_CONFIG_HOME", ".config")?.join("mosaic-shell/config.toml"))
    }
    pub fn override_path() -> Result<PathBuf> {
        Ok(xdg_dir("XDG_STATE_HOME", ".local/state")?.join("mosaic-shell/overrides.toml"))
    }
    pub fn load() -> Result<Self> {
        let path = Self::path()?;
        let c = match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("{}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(e) => return Err(e.into()),
        };
        if c.mode == ConfigMode::Strict {
            return Ok(c);
        }
        match fs::read_to_string(Self::override_path()?) {
            Ok(text) => c.with_override(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(c),
            Err(e) => Err(e.into()),
        }
    }
    // Writes only mutable overrides; never follows a Nix config symlink for writing.
    pub fn changed_override(&self, next: &Self, existing: &str) -> Result<String> {
        next.validate()?;
        self.with_override(existing)?;
        if next.mode != self.mode || next.schema_version != self.schema_version {
            bail!("Нельзя менять режим конфигурации из GUI");
        }
        let before = toml::Value::try_from(self)?;
        let after = toml::Value::try_from(next)?;
        let mut patch: toml::Value = toml::from_str(existing)?;
        if let Some(changes) = diff(&before, &after) {
            merge(&mut patch, changes);
        }
        self.with_override(&toml::to_string(&patch)?)?;
        Ok(toml::to_string(&patch)?)
    }
    pub fn save_changes(&self, next: &Self) -> Result<()> {
        let existing = match fs::read_to_string(Self::override_path()?) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        self.save_override(&self.changed_override(next, &existing)?)
    }
    pub fn save_override(&self, text: &str) -> Result<()> {
        self.with_override(text)?;
        let path = Self::override_path()?;
        let parent = path.parent().context("Отсутствует parent")?;
        fs::create_dir_all(parent)?;
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        let tmp = parent.join(format!("overrides.{}.tmp", std::process::id()));
        let result = (|| -> Result<()> {
            use std::io::Write;
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(text.as_bytes())?;
            file.sync_all()?;
            fs::rename(&tmp, &path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(tmp);
        }
        result
    }
}
pub fn xdg_dir(key: &str, fallback: &str) -> Result<PathBuf> {
    if let Some(value) = env::var_os(key).filter(|s| !s.is_empty()) {
        let p = PathBuf::from(value);
        if p.is_absolute() {
            return Ok(p);
        }
    }
    Ok(PathBuf::from(env::var_os("HOME").context("HOME не задан")?).join(fallback))
}
fn merge(base: &mut toml::Value, patch: toml::Value) {
    match (base, patch) {
        (toml::Value::Table(base), toml::Value::Table(patch)) => {
            for (key, v) in patch {
                if let Some(old) = base.get_mut(&key) {
                    merge(old, v);
                } else {
                    base.insert(key, v);
                }
            }
        }
        (base, patch) => *base = patch,
    }
}

fn diff(before: &toml::Value, after: &toml::Value) -> Option<toml::Value> {
    if before == after {
        return None;
    }
    if let (Some(a), Some(b)) = (before.as_table(), after.as_table()) {
        let mut changes = toml::map::Map::new();
        for (key, value) in b {
            if let Some(old) = a.get(key) {
                if let Some(change) = diff(old, value) {
                    changes.insert(key.clone(), change);
                }
            } else {
                changes.insert(key.clone(), value.clone());
            }
        }
        if changes.is_empty() {
            None
        } else {
            Some(toml::Value::Table(changes))
        }
    } else {
        Some(after.clone())
    }
}
