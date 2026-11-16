use gtk::{gio, prelude::*};
#[derive(Clone)]
pub struct Application {
    pub id: String,
    pub name: String,
    pub info: gio::AppInfo,
    pub wm_class: Option<String>,
}
#[derive(Clone)]
pub struct Catalog {
    pub apps: Vec<Application>,
}
impl Catalog {
    pub fn load() -> Self {
        let mut apps: Vec<_> = gio::AppInfo::all()
            .into_iter()
            .filter(|a| a.should_show())
            .filter_map(|info| {
                let id = info.id()?.to_string();
                let desktop = gio::DesktopAppInfo::new(&id);
                Some(Application {
                    id,
                    name: info.display_name().to_string(),
                    wm_class: desktop.and_then(|d| d.startup_wm_class().map(|s| s.to_string())),
                    info,
                })
            })
            .collect();
        apps.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Self { apps }
    }
    pub fn get(&self, id: &str) -> Option<&Application> {
        self.apps.iter().find(|a| a.id == id)
    }
    pub fn match_window(&self, app_id: &str) -> Option<&Application> {
        let matches: Vec<_> = self
            .apps
            .iter()
            .filter(|a| {
                a.id.trim_end_matches(".desktop") == app_id || a.wm_class.as_deref() == Some(app_id)
            })
            .collect();
        if matches.len() == 1 {
            matches.first().copied()
        } else {
            None
        }
    }
    pub fn launch(&self, id: &str) -> anyhow::Result<()> {
        let app = self
            .get(id)
            .ok_or_else(|| anyhow::anyhow!("Приложение {id} отсутствует в каталоге"))?;
        let context = gtk::gdk::Display::default().map(|d| d.app_launch_context());
        app.info.launch(&[], context.as_ref())?;
        Ok(())
    }
}
pub fn icon(app: Option<&Application>, size: i32) -> gtk::Image {
    let image = if let Some(icon) = app.and_then(|a| a.info.icon()) {
        gtk::Image::from_gicon(&icon)
    } else {
        gtk::Image::from_icon_name("application-x-executable")
    };
    image.set_pixel_size(size);
    image
}
