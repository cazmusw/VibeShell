use crate::locale::tr;
use gtk::{gio, glib, prelude::*};

mod size_object {
    use gtk::glib::{self, subclass::prelude::*};
    use gtk::prelude::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default, glib::Properties)]
    #[properties(wrapper_type = super::FolderSize)]
    pub struct FolderSize {
        #[property(get, set)]
        text: RefCell<String>,
        pub(super) bytes: Cell<Option<u64>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FolderSize {
        const NAME: &'static str = "MosaicFilesFolderSize";
        type Type = super::FolderSize;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FolderSize {}
}

glib::wrapper! {
    pub struct FolderSize(ObjectSubclass<size_object::FolderSize>);
}

impl FolderSize {
    fn pending() -> Self {
        glib::Object::builder().property("text", "…").build()
    }

    pub fn bytes(&self) -> Option<u64> {
        use glib::subclass::prelude::ObjectSubclassIsExt;
        self.imp().bytes.get()
    }

    pub fn update(&self, bytes: Option<u64>, text: &str) -> bool {
        use glib::subclass::prelude::ObjectSubclassIsExt;
        let changed = self.imp().bytes.replace(bytes) != bytes;
        if self.text() != text {
            self.set_text(text);
        }
        changed
    }
}

pub struct FileEntry {
    pub file: gio::File,
    pub name: glib::GString,
    pub search_name: String,
    pub name_key: glib::FilenameCollationKey,
    pub type_label: glib::GString,
    pub icon: Option<gio::Icon>,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub mode: Option<u32>,
    pub size: Option<u64>,
    pub folder_size: Option<FolderSize>,
    pub modified: Option<glib::DateTime>,
}

pub async fn read_directory(directory: &gio::File) -> Result<Vec<FileEntry>, glib::Error> {
    let enumerator = directory
        .enumerate_children_future(
            "standard::name,standard::display-name,standard::type,standard::is-hidden,standard::is-symlink,standard::icon,standard::size,standard::fast-content-type,time::modified,unix::mode",
            gio::FileQueryInfoFlags::NONE,
            glib::Priority::DEFAULT,
        )
        .await?;
    let mut entries = Vec::new();
    loop {
        let batch = enumerator
            .next_files_future(128, glib::Priority::DEFAULT)
            .await?;
        if batch.is_empty() {
            break;
        }
        entries.extend(batch.into_iter().map(|info| {
            let name = info.display_name();
            let kind = info.file_type();
            let is_directory = kind == gio::FileType::Directory;
            let search_name = glib::casefold(&name);
            let type_label = if is_directory {
                tr("Папка").into()
            } else {
                info.attribute_string("standard::fast-content-type")
                    .map_or_else(
                        || tr("Файл").into(),
                        |mime| gio::content_type_get_description(&mime),
                    )
            };
            let file = directory.child(info.name());
            let folder_size =
                (is_directory && !info.is_symlink() && file.is_native()).then(FolderSize::pending);
            FileEntry {
                file,
                name_key: glib::FilenameCollationKey::from(&search_name),
                search_name: search_name.into(),
                type_label,
                name,
                icon: info.icon(),
                is_directory,
                is_hidden: info.is_hidden(),
                mode: info
                    .has_attribute("unix::mode")
                    .then(|| info.attribute_uint32("unix::mode") & 0o7777),
                size: (kind == gio::FileType::Regular)
                    .then(|| u64::try_from(info.size()).ok())
                    .flatten(),
                folder_size,
                modified: info.modification_date_time(),
            }
        }));
    }
    entries.sort_unstable_by(|a, b| {
        b.is_directory
            .cmp(&a.is_directory)
            .then_with(|| a.name_key.cmp(&b.name_key))
            .then_with(|| a.name.cmp(&b.name))
    });
    Ok(entries)
}

pub fn permission_label(mode: u32) -> String {
    let mut label = String::with_capacity(9);
    for (shift, special, lower, upper) in [
        (6, 0o4000, 's', 'S'),
        (3, 0o2000, 's', 'S'),
        (0, 0o1000, 't', 'T'),
    ] {
        let bits = mode >> shift;
        label.push(if bits & 4 != 0 { 'r' } else { '-' });
        label.push(if bits & 2 != 0 { 'w' } else { '-' });
        label.push(match (bits & 1 != 0, mode & special != 0) {
            (true, true) => lower,
            (false, true) => upper,
            (true, false) => 'x',
            (false, false) => '-',
        });
    }
    label
}

pub fn display_location(file: &gio::File) -> glib::GString {
    if file.has_uri_scheme("trash") && file.parent().is_none() {
        tr("Корзина").into()
    } else {
        file.basename()
            .and_then(|name| name.file_name().map(glib::filename_display_name))
            .unwrap_or_else(|| file.parse_name())
    }
}

pub fn location_components(file: &gio::File) -> Vec<gio::File> {
    let mut components = vec![file.clone()];
    while let Some(parent) = components.last().and_then(gio::File::parent) {
        components.push(parent);
    }
    components.reverse();
    components
}

pub fn resolve_location(text: &str, directory: &gio::File) -> gio::File {
    if let Some(path) = text.strip_prefix("~/") {
        gio::File::for_path(glib::home_dir().join(path))
    } else if text == "~" {
        gio::File::for_path(glib::home_dir())
    } else if text.contains("://") && glib::uri_parse_scheme(text).is_some() {
        gio::File::for_uri(text)
    } else if text.starts_with('/') {
        gio::File::for_parse_name(text)
    } else {
        directory.resolve_relative_path(text)
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty() && !matches!(name, "." | "..") && !name.contains(['/', '\0'])
}

#[derive(Clone, Copy)]
pub enum CreateKind {
    File,
    Folder,
}

pub fn named_child(directory: &gio::File, name: &str) -> Result<gio::File, glib::Error> {
    if !valid_name(name) {
        return Err(glib::Error::new(
            gio::IOErrorEnum::InvalidFilename,
            tr("Некорректное имя"),
        ));
    }
    directory.child_for_display_name(name)
}

pub async fn create_entry(
    directory: &gio::File,
    name: &str,
    kind: CreateKind,
) -> Result<gio::File, glib::Error> {
    let target = named_child(directory, name)?;
    match kind {
        CreateKind::File => {
            let stream = target
                .create_future(gio::FileCreateFlags::NONE, glib::Priority::DEFAULT)
                .await?;
            stream.close_future(glib::Priority::DEFAULT).await?;
        }
        CreateKind::Folder => {
            target
                .make_directory_future(glib::Priority::DEFAULT)
                .await?
        }
    }
    Ok(target)
}

#[cfg(test)]
mod tests {
    use super::permission_label;

    #[test]
    fn attributes_render_unix_permissions_without_file_type_bits() {
        assert_eq!(permission_label(0o100644), "rw-r--r--");
        assert_eq!(permission_label(0o041750), "rwxr-x--T");
        assert_eq!(permission_label(0o106650), "rwSr-s---");
    }
}
