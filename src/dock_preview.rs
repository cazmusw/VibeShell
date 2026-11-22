pub fn unique_identifier<'a>(
    target: (&str, &str),
    managed: impl IntoIterator<Item = (&'a str, &'a str)>,
    images: impl IntoIterator<Item = ((&'a str, &'a str), &'a str)>,
) -> Option<&'a str> {
    if target.0.is_empty()
        || target.1.is_empty()
        || managed
            .into_iter()
            .filter(|&key| key == target)
            .take(2)
            .count()
            != 1
    {
        return None;
    }
    let mut matches = images.into_iter().filter(|(key, _)| *key == target);
    let (_, identifier) = matches.next()?;
    (matches.next().is_none() && !identifier.is_empty()).then_some(identifier)
}

#[cfg(feature = "desktop")]
pub(crate) fn capture(identifier: &str) -> Option<Vec<u8>> {
    use std::{process::Command, time::Duration};

    let mut command = Command::new(option_env!("MOSAIC_GRIM").unwrap_or("grim"));
    command.args(["-t", "png", "-l", "1", "-s", "0.25", "-T", identifier, "-"]);
    crate::process::capture_bytes(&mut command, Duration::from_millis(900), 2 * 1024 * 1024).ok()
}

#[cfg(feature = "desktop")]
pub(crate) fn texture(bytes: Vec<u8>) -> Option<gtk::gdk::Texture> {
    use gtk::{gdk, prelude::Cast};

    let pixbuf = crate::dock_icons::decode_png(bytes, 384, 2048, 2 * 1024 * 1024).ok()?;
    let format = if pixbuf.has_alpha() {
        gdk::MemoryFormat::R8g8b8a8
    } else {
        gdk::MemoryFormat::R8g8b8
    };
    Some(
        gdk::MemoryTexture::new(
            pixbuf.width(),
            pixbuf.height(),
            format,
            &pixbuf.read_pixel_bytes(),
            pixbuf.rowstride() as usize,
        )
        .upcast(),
    )
}
