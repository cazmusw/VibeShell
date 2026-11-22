#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Application {
    pub id: String,
    pub wm_class: Option<String>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub epoch: u64,
    pub id: u32,
    pub app_id: String,
    pub title: String,
    pub active: bool,
    pub minimized: bool,
    pub fullscreen: bool,
    pub outputs: Vec<String>,
}

pub fn match_app(app_id: &str, apps: &[Application]) -> Option<String> {
    if app_id.is_empty() {
        return None;
    }
    if let Some(app) = apps
        .iter()
        .find(|a| a.id == app_id || a.id.strip_suffix(".desktop") == Some(app_id))
    {
        return Some(app.id.clone());
    }
    let matches: Vec<_> = apps
        .iter()
        .filter(|a| a.wm_class.as_deref() == Some(app_id))
        .collect();
    match matches.as_slice() {
        [app] => return Some(app.id.clone()),
        [] => (),
        _ => return None,
    }
    let name = app_id.strip_suffix(".desktop").unwrap_or(app_id);
    let name = if name.eq_ignore_ascii_case("org.xfce.thunar") {
        "thunar"
    } else {
        name
    };
    let mut matches = apps.iter().filter(|a| {
        a.id.strip_suffix(".desktop")
            .unwrap_or(&a.id)
            .eq_ignore_ascii_case(name)
            || a.wm_class
                .as_deref()
                .is_some_and(|class| class.eq_ignore_ascii_case(name))
    });
    let first = matches.next()?;
    matches.next().is_none().then(|| first.id.clone())
}
pub fn dock_order(pins: &[String], running: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    pins.iter()
        .chain(running)
        .filter(|id| seen.insert((*id).clone()))
        .cloned()
        .collect()
}
pub fn valid_window(windows: &[Window], epoch: u64, id: u32) -> bool {
    windows.iter().any(|w| w.epoch == epoch && w.id == id)
}
pub fn dock_window_visible(window: &Window, output: Option<&str>) -> bool {
    output.is_none_or(|name| !name.is_empty() && window.outputs.iter().any(|out| out == name))
}
pub fn newly_observed_windows<'a>(previous: &[Window], current: &'a [Window]) -> Vec<&'a Window> {
    let known: std::collections::HashSet<_> = previous
        .iter()
        .map(|w| (w.epoch, w.id, w.app_id.as_str()))
        .collect();
    current
        .iter()
        .filter(|w| !known.contains(&(w.epoch, w.id, w.app_id.as_str())))
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
pub enum DockClick {
    Launch,
    Activate(u64, u32),
    Minimize(u64, u32),
}

pub fn dock_click(windows: &[Window], minimize_at_end: bool) -> DockClick {
    let mut ordered: Vec<_> = windows.iter().collect();
    ordered.sort_by_key(|w| (w.epoch, w.id));
    let Some(first) = ordered.first() else {
        return DockClick::Launch;
    };
    if let Some(index) = ordered.iter().position(|w| w.active && !w.minimized) {
        if let Some(next) = ordered.get(index + 1) {
            return DockClick::Activate(next.epoch, next.id);
        }
        if minimize_at_end {
            let active = ordered[index];
            return DockClick::Minimize(active.epoch, active.id);
        }
    }
    DockClick::Activate(first.epoch, first.id)
}
pub const DESKTOP_DROP_LIMIT: usize = 16 * 1024;

pub fn dropped_desktop_uri(data: &[u8]) -> Option<&str> {
    if data.len() > DESKTOP_DROP_LIMIT {
        return None;
    }
    let mut lines = std::str::from_utf8(data)
        .ok()?
        .lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'));
    let uri = lines.next()?;
    if lines.next().is_some()
        || uri.chars().any(char::is_control)
        || !(uri.starts_with("file:///") || uri.starts_with("file://localhost/"))
    {
        return None;
    }
    Some(uri)
}

#[cfg(feature = "desktop")]
pub async fn resolve_desktop_drop(
    path: &std::path::Path,
    catalog: &[(String, std::path::PathBuf)],
) -> Option<String> {
    use gtk::{gio, glib, prelude::*};

    async fn file_id(path: &std::path::Path) -> Option<glib::GString> {
        let info = gio::File::for_path(path)
            .query_info_future(
                "standard::type,id::file",
                gio::FileQueryInfoFlags::NONE,
                glib::Priority::DEFAULT,
            )
            .await
            .ok()?;
        if info.file_type() != gio::FileType::Regular {
            return None;
        }
        info.attribute_string("id::file")
    }

    if !path.is_absolute() || path.extension()? != "desktop" {
        return None;
    }
    let identity = file_id(path).await?;
    if let Some((id, _)) = catalog.iter().find(|(_, file)| file == path) {
        return Some(id.clone());
    }
    let mut found = None;
    for (id, file) in catalog {
        if file.file_name() == path.file_name()
            && file_id(file).await.as_ref() == Some(&identity)
            && found.replace(id.as_str()).is_some()
        {
            return None;
        }
    }
    found.map(str::to_owned)
}
