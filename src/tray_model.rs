/// SNI permits either a bus name (default object) or a path on the caller.
pub fn registration(sender: &str, value: &str) -> Result<(String, String), &'static str> {
    if value.is_empty() || value.len() > 255 {
        return Err("invalid registration");
    }
    if let Some(path) = value.strip_prefix('/') {
        if value != "/"
            && path
                .split('/')
                .any(|p| p.is_empty() || !p.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_'))
        {
            return Err("invalid path");
        }
        return Ok((sender.into(), value.into()));
    }
    let unique = value.starts_with(':');
    let name = value.trim_start_matches(':');
    if !name.contains('.')
        || name.split('.').any(|p| {
            p.is_empty()
                || (!unique && p.as_bytes()[0].is_ascii_digit())
                || !p
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        })
    {
        return Err("invalid bus name");
    }
    Ok((value.into(), "/StatusNotifierItem".into()))
}
pub fn rgba(width: i32, height: i32, argb: &[u8]) -> Option<Vec<u8>> {
    if !(1..=256).contains(&width)
        || !(1..=256).contains(&height)
        || argb.len() != width as usize * height as usize * 4
    {
        return None;
    }
    Some(
        argb.chunks_exact(4)
            .flat_map(|p| [p[1], p[2], p[3], p[0]])
            .collect(),
    )
}
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TrayItem {
    pub key: String,
    pub title: String,
    pub status: String,
    pub icon_name: String,
    pub tooltip: String,
    pub width: i32,
    pub height: i32,
    pub pixels: Vec<u8>,
    pub item_is_menu: bool,
}
pub type Wire = (
    String,
    String,
    String,
    String,
    String,
    i32,
    i32,
    Vec<u8>,
    bool,
);
impl TrayItem {
    pub fn wire(&self) -> Wire {
        (
            self.key.clone(),
            self.title.clone(),
            self.status.clone(),
            self.icon_name.clone(),
            self.tooltip.clone(),
            self.width,
            self.height,
            self.pixels.clone(),
            self.item_is_menu,
        )
    }
    pub fn from_wire(w: Wire) -> Self {
        Self {
            key: w.0,
            title: w.1,
            status: w.2,
            icon_name: w.3,
            tooltip: w.4,
            width: w.5,
            height: w.6,
            pixels: w.7,
            item_is_menu: w.8,
        }
    }
}
