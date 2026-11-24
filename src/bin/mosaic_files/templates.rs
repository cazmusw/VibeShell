use gtk::{gio, glib, prelude::*};
use rustix::fs::{Mode, OFlags, open};
use std::{
    cell::RefCell,
    fs::{self, File, Permissions},
    io,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    rc::Rc,
};

pub struct Picker {
    choice: gtk::DropDown,
    paths: Rc<RefCell<Vec<PathBuf>>>,
}

impl Picker {
    pub fn add(panel: &gtk::Box, name: &gtk::Entry) -> Option<Self> {
        let directory = glib::user_special_dir(glib::UserDirectory::Templates)
            .filter(|path| path != &glib::home_dir())?;
        let label = gtk::Label::new(Some("Шаблон"));
        label.set_xalign(0.0);
        let model = gtk::StringList::new(&["Пустой файл"]);
        let choice = gtk::DropDown::new(Some(model.clone()), gtk::Expression::NONE);
        label.set_mnemonic_widget(Some(&choice));
        choice.update_property(&[gtk::accessible::Property::Label("Шаблон")]);
        // Hide the picker until there is a choice to make.
        label.set_visible(false);
        choice.set_visible(false);
        panel.append(&label);
        panel.append(&choice);

        let paths = Rc::new(RefCell::new(Vec::new()));
        let by_choice = paths.clone();
        let weak_name = name.downgrade();
        let previous = RefCell::new(String::new());
        choice.connect_selected_notify(move |choice| {
            let Some(name) = weak_name.upgrade() else {
                return;
            };
            let paths = by_choice.borrow();
            let selected = selected_path(choice, &paths);
            let suggestion = selected
                .map(glib::filename_display_basename)
                .unwrap_or_default();
            let current = name.text();
            if current.is_empty() || current.as_str() == previous.borrow().as_str() {
                name.set_text(&suggestion);
                let stem = Path::new(suggestion.as_str())
                    .file_stem()
                    .and_then(|part| part.to_str())
                    .unwrap_or(suggestion.as_str());
                name.select_region(0, stem.chars().count() as i32);
            }
            previous.replace(suggestion.into());
        });

        let weak_choice = choice.downgrade();
        let weak_label = label.downgrade();
        let loaded = paths.clone();
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || list(&directory)).await;
            let (Some(choice), Some(label)) = (weak_choice.upgrade(), weak_label.upgrade()) else {
                return;
            };
            match result {
                Ok(Ok(files)) if !files.is_empty() => {
                    for path in &files {
                        model.append(&glib::filename_display_basename(path));
                    }
                    loaded.replace(files);
                    label.set_visible(true);
                    choice.set_visible(true);
                }
                Ok(Ok(_)) => {}
                _ => {
                    label.set_text("Не удалось прочитать папку шаблонов.");
                    label.set_visible(true);
                }
            }
        });
        Some(Self { choice, paths })
    }

    pub fn selected(&self) -> Option<PathBuf> {
        selected_path(&self.choice, &self.paths.borrow()).map(Path::to_owned)
    }
}

fn selected_path<'a>(choice: &gtk::DropDown, paths: &'a [PathBuf]) -> Option<&'a Path> {
    let index = choice.selected().checked_sub(1)?;
    paths.get(index as usize).map(PathBuf::as_path)
}

fn list(directory: &Path) -> io::Result<Vec<PathBuf>> {
    let entries = match fs::read_dir(directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut paths = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_name().as_encoded_bytes().starts_with(b".")
            && entry.file_type().is_ok_and(|kind| kind.is_file())
        {
            paths.push(entry.path());
        }
    }
    paths.sort_by_cached_key(|path| {
        glib::FilenameCollationKey::from(glib::filename_display_basename(path))
    });
    Ok(paths)
}

pub async fn create(
    directory: &gio::File,
    name: &str,
    source: PathBuf,
) -> Result<gio::File, glib::Error> {
    let target = crate::filesystem::named_child(directory, name)?;
    let path = target.path().ok_or_else(|| {
        glib::Error::new(
            gio::IOErrorEnum::NotSupported,
            "Шаблоны доступны для локальных папок.",
        )
    })?;
    gio::spawn_blocking(move || copy(&source, &path))
        .await
        .unwrap_or_else(|_| {
            Err(glib::Error::new(
                gio::IOErrorEnum::Failed,
                "Ошибка при копировании шаблона.",
            ))
        })?;
    Ok(target)
}

fn gio_error(error: io::Error, detail: &str) -> glib::Error {
    let code = error
        .raw_os_error()
        .map_or(gio::IOErrorEnum::Failed, gio::io_error_from_errno);
    glib::Error::new(code, detail)
}

fn copy(source: &Path, target: &Path) -> Result<(), glib::Error> {
    let read_failed = |error| gio_error(error, "Шаблон недоступен или больше не существует.");
    let write_failed = |error| gio_error(error, "Не удалось записать файл в выбранную папку.");
    let mut input = File::from(
        open(
            source,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .map_err(io::Error::from)
        .map_err(read_failed)?,
    );
    if !input.metadata().map_err(read_failed)?.is_file() {
        return Err(glib::Error::new(
            gio::IOErrorEnum::NotRegularFile,
            "Шаблон больше не является обычным файлом.",
        ));
    }
    let parent = target.parent().ok_or_else(|| {
        glib::Error::new(
            gio::IOErrorEnum::InvalidFilename,
            "Некорректная папка назначения.",
        )
    })?;
    let mut staged = tempfile::Builder::new()
        .prefix(".mosaic-template-")
        .permissions(Permissions::from_mode(0o666))
        .tempfile_in(parent)
        .map_err(write_failed)?;
    io::copy(&mut input, staged.as_file_mut())
        .map_err(|error| gio_error(error, "Не удалось скопировать содержимое шаблона."))?;
    staged.persist_noclobber(target).map_err(|error| {
        if error.error.kind() == io::ErrorKind::AlreadyExists {
            glib::Error::new(
                gio::IOErrorEnum::Exists,
                "Объект с таким именем уже существует.",
            )
        } else {
            write_failed(error.error)
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publishes_complete_contents_and_preserves_an_existing_target() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("letter.txt");
        let target = temp.path().join("copy.txt");
        fs::write(&source, b"Mosaic\0Template\n").unwrap();
        copy(&source, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"Mosaic\0Template\n");

        fs::write(&target, b"existing").unwrap();
        let error = copy(&source, &target).unwrap_err();
        assert!(error.matches(gio::IOErrorEnum::Exists));
        assert_eq!(fs::read(&target).unwrap(), b"existing");
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
    }

    #[test]
    fn rejects_missing_and_replaced_sources_without_leaving_a_file() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let target = temp.path().join("target");
        assert!(
            copy(&source, &target)
                .unwrap_err()
                .matches(gio::IOErrorEnum::NotFound)
        );
        let original = temp.path().join("original");
        fs::write(&original, b"regular").unwrap();
        std::os::unix::fs::symlink(original, &source).unwrap();
        assert!(copy(&source, &target).is_err());
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 2);
        assert!(!target.exists());
    }

    #[test]
    fn offers_only_visible_regular_files_at_the_top_level() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("visible.txt");
        fs::write(&file, b"template").unwrap();
        fs::write(temp.path().join(".hidden"), b"hidden").unwrap();
        fs::create_dir(temp.path().join("folder")).unwrap();
        std::os::unix::fs::symlink(&file, temp.path().join("linked.txt")).unwrap();
        assert_eq!(list(temp.path()).unwrap(), [file]);
    }
}
