//! Bounded, content-addressed images selected by the user for the Dock.

pub fn valid_key(key: &str) -> bool {
    if key.len() > 256
        || key
            .chars()
            .any(|ch| ch.is_control() || matches!(ch, '/' | '\\'))
    {
        return false;
    }
    if let Some(app) = key.strip_prefix("window:") {
        !app.is_empty() && !app.contains(':')
    } else {
        key.strip_suffix(".desktop")
            .is_some_and(|name| !name.is_empty() && !name.contains(':'))
    }
}

pub fn valid_asset(asset: &str) -> bool {
    asset.strip_suffix(".png").is_some_and(|hash| {
        hash.len() == 64
            && hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

#[cfg(feature = "desktop")]
mod images {
    use super::valid_asset;
    use gtk::{gdk_pixbuf::Pixbuf, gio, glib};
    use std::{
        fs,
        io::{self, Read, Write},
        os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt},
        path::{Path, PathBuf},
    };

    const SOURCE_LIMIT: u64 = 8 * 1024 * 1024;
    const ICON_SIZE: i32 = 160;
    const STORAGE_LIMIT: u64 = 128 * 1024;
    const PNG: &[u8; 16] = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR";

    pub fn asset_path(config: &Path, asset: &str) -> Option<PathBuf> {
        valid_asset(asset).then(|| config.with_file_name("dock-icons").join(asset))
    }

    fn bounded_file(path: &Path, limit: u64, allow_symlink: bool) -> io::Result<Vec<u8>> {
        let metadata = if allow_symlink {
            path.metadata()?
        } else {
            path.symlink_metadata()?
        };
        if !metadata.is_file() || metadata.len() > limit {
            return Err(io::Error::other("icon must be a bounded regular file"));
        }
        let file = fs::File::open(path)?;
        let mut bytes = Vec::new();
        file.take(limit + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > limit {
            return Err(io::Error::other("icon exceeds the size limit"));
        }
        Ok(bytes)
    }

    pub(crate) fn decode_png(
        bytes: Vec<u8>,
        size: i32,
        dimension_limit: u32,
        pixel_limit: u64,
    ) -> io::Result<Pixbuf> {
        if bytes.len() < 33 || !bytes.starts_with(PNG) {
            return Err(io::Error::other("a PNG image is required"));
        }
        let width = u32::from_be_bytes(bytes[16..20].try_into().expect("PNG width"));
        let height = u32::from_be_bytes(bytes[20..24].try_into().expect("PNG height"));
        if !(1..=dimension_limit).contains(&width)
            || !(1..=dimension_limit).contains(&height)
            || u64::from(width) * u64::from(height) > pixel_limit
        {
            return Err(io::Error::other("PNG dimensions exceed the limit"));
        }
        let stream = gio::MemoryInputStream::from_bytes(&glib::Bytes::from_owned(bytes));
        Pixbuf::from_stream_at_scale(&stream, size, size, true, gio::Cancellable::NONE)
            .map_err(io::Error::other)
    }

    pub fn import_png(config: &Path, source: &Path) -> io::Result<String> {
        let data = bounded_file(source, SOURCE_LIMIT, true)?;
        let pixbuf = decode_png(data, ICON_SIZE, 4096, 4 * 1024 * 1024)?;
        let bytes = pixbuf
            .save_to_bufferv("png", &[])
            .map_err(io::Error::other)?;
        if bytes.len() as u64 > STORAGE_LIMIT {
            return Err(io::Error::other("normalized icon exceeds the size limit"));
        }
        let sum = glib::compute_checksum_for_data(glib::ChecksumType::Sha256, &bytes)
            .ok_or_else(|| io::Error::other("could not calculate the icon checksum"))?;
        let name = format!("{sum}.png");
        let path = asset_path(config, &name).expect("checksum is a valid icon name");
        let directory = path.parent().expect("icon destination has a parent");
        if let Some(parent) = directory.parent() {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        match fs::DirBuilder::new().mode(0o700).create(directory) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let metadata = directory.symlink_metadata()?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o022 != 0 {
            return Err(io::Error::other(
                "Dock icon directory must be a private directory",
            ));
        }
        // Publish only a complete image and reuse an existing identical object.
        let temporary =
            glib::mkdtemp(directory.join(".icon-XXXXXX")).ok_or_else(io::Error::last_os_error)?;
        let result = (|| {
            let pending = temporary.join("image.png");
            let mut file = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&pending)?;
            file.write_all(&bytes)?;
            file.sync_all()?;
            match fs::hard_link(&pending, &path) {
                Ok(()) => fs::File::open(directory)?.sync_all(),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                    if bounded_file(&path, STORAGE_LIMIT, false)? == bytes {
                        Ok(())
                    } else {
                        Err(io::Error::other(
                            "existing icon contents do not match the checksum",
                        ))
                    }
                }
                Err(error) => Err(error),
            }
        })();
        let _ = fs::remove_dir_all(temporary);
        result.map(|()| name)
    }

    pub fn load_png(config: &Path, asset: &str, size: i32) -> Option<Pixbuf> {
        let path = asset_path(config, asset)?;
        let directory = path.parent()?.symlink_metadata().ok()?;
        if !directory.is_dir() {
            return None;
        }
        let bytes = bounded_file(&path, STORAGE_LIMIT, false).ok()?;
        decode_png(
            bytes,
            size.clamp(1, ICON_SIZE),
            ICON_SIZE as u32,
            u64::from(ICON_SIZE as u32).pow(2),
        )
        .ok()
    }
}

#[cfg(feature = "desktop")]
pub(crate) use images::decode_png;
#[cfg(feature = "desktop")]
pub use images::{asset_path, import_png, load_png};
