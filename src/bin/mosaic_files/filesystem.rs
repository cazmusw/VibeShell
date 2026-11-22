use gtk::{gio, glib, prelude::*};

pub struct FileEntry {
    pub file: gio::File,
    pub name: glib::GString,
    pub search_name: String,
    pub name_key: glib::FilenameCollationKey,
    pub type_label: glib::GString,
    pub icon: Option<gio::Icon>,
    pub is_directory: bool,
    pub is_hidden: bool,
    pub size: Option<u64>,
    pub modified: Option<glib::DateTime>,
}

pub async fn read_directory(directory: &gio::File) -> Result<Vec<FileEntry>, glib::Error> {
    let enumerator = directory
        .enumerate_children_future(
            "standard::name,standard::display-name,standard::type,standard::is-hidden,standard::icon,standard::size,standard::fast-content-type,time::modified",
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
                "Папка".into()
            } else {
                info.attribute_string("standard::fast-content-type")
                    .map_or_else(
                        || "Файл".into(),
                        |mime| gio::content_type_get_description(&mime),
                    )
            };
            FileEntry {
                file: directory.child(info.name()),
                name_key: glib::FilenameCollationKey::from(&search_name),
                search_name: search_name.into(),
                type_label,
                name,
                icon: info.icon(),
                is_directory,
                is_hidden: info.is_hidden(),
                size: (kind == gio::FileType::Regular)
                    .then(|| u64::try_from(info.size()).ok())
                    .flatten(),
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

pub fn display_location(file: &gio::File) -> glib::GString {
    if file.has_uri_scheme("trash") && file.parent().is_none() {
        "Корзина".into()
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

pub async fn create_entry(
    directory: &gio::File,
    name: &str,
    kind: CreateKind,
) -> Result<gio::File, glib::Error> {
    if !valid_name(name) {
        return Err(glib::Error::new(
            gio::IOErrorEnum::InvalidFilename,
            "Некорректное имя",
        ));
    }
    let target = directory.child_for_display_name(name)?;
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
