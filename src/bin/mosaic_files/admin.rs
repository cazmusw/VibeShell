use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

const MAX_TEXT_BYTES: usize = 1024 * 1024;
const ATTRIBUTES: &str = "standard::type,standard::size,etag::value,id::file";

pub fn target(file: &gio::File) -> Option<gio::File> {
    if !file.has_uri_scheme("file") || file.path().is_none() {
        return None;
    }
    let uri = file.uri();
    let path = uri.strip_prefix("file://")?;
    path.starts_with('/')
        .then(|| gio::File::for_uri(&format!("admin://{path}")))
}

fn error(code: gio::IOErrorEnum, message: &str) -> glib::Error {
    glib::Error::new(code, message)
}

async fn query_info(file: &gio::File) -> Result<gio::FileInfo, glib::Error> {
    file.query_info_future(
        ATTRIBUTES,
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        glib::Priority::DEFAULT,
    )
    .await
}

#[derive(Debug, PartialEq, Eq)]
struct Version {
    etag: glib::GString,
    identity: Option<glib::GString>,
}

impl Version {
    fn from_info(info: &gio::FileInfo) -> Result<Self, glib::Error> {
        if info.file_type() != gio::FileType::Regular {
            return Err(error(
                gio::IOErrorEnum::NotRegularFile,
                "Редактор открывает обычные файлы. Для ссылки выберите исходный файл.",
            ));
        }
        let etag = info
            .etag()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                error(
                    gio::IOErrorEnum::NotSupported,
                    "Система не вернула версию файла для безопасного сохранения.",
                )
            })?;
        Ok(Self {
            etag,
            identity: info.attribute_string("id::file"),
        })
    }
}

#[derive(Debug)]
struct Document {
    text: String,
    version: Version,
}

fn changed() -> glib::Error {
    error(
        gio::IOErrorEnum::WrongEtag,
        "Файл изменился в другом приложении. Откройте его заново перед сохранением.",
    )
}

fn text_limit() -> glib::Error {
    error(
        gio::IOErrorEnum::MessageTooLarge,
        "Редактор поддерживает текстовые файлы до 1 МиБ.",
    )
}

async fn read_document(file: &gio::File, info: &gio::FileInfo) -> Result<Document, glib::Error> {
    let version = Version::from_info(info)?;
    if info.size() > MAX_TEXT_BYTES as i64 {
        return Err(text_limit());
    }
    let stream = file.read_future(glib::Priority::DEFAULT).await?;
    let opened = stream
        .query_info_future(ATTRIBUTES, glib::Priority::DEFAULT)
        .await?;
    if Version::from_info(&opened)? != version {
        return Err(changed());
    }
    let mut bytes = Vec::new();
    loop {
        let chunk = stream
            .read_bytes_future(
                (MAX_TEXT_BYTES + 1 - bytes.len()).min(64 * 1024),
                glib::Priority::DEFAULT,
            )
            .await?;
        if chunk.is_empty() {
            break;
        }
        bytes.extend_from_slice(&chunk);
        if bytes.len() > MAX_TEXT_BYTES {
            return Err(text_limit());
        }
    }
    if Version::from_info(&query_info(file).await?)? != version {
        return Err(changed());
    }
    let text = String::from_utf8(bytes)
        .ok()
        .filter(|text| !text.contains('\0'))
        .ok_or_else(|| {
            error(
                gio::IOErrorEnum::InvalidData,
                "Редактор поддерживает текстовые файлы в UTF-8.",
            )
        })?;
    Ok(Document { text, version })
}

async fn save_document(
    file: &gio::File,
    text: String,
    version: &Version,
) -> Result<Option<Version>, glib::Error> {
    if text.len() > MAX_TEXT_BYTES {
        return Err(text_limit());
    }
    if Version::from_info(&query_info(file).await?)? != *version {
        return Err(changed());
    }
    let (saved, _) = file
        .replace_contents_future(
            text.into_bytes(),
            Some(&version.etag),
            false,
            gio::FileCreateFlags::NONE,
        )
        .await
        .map_err(|(_, error)| error)?;
    // The admin backend may omit the new ETag when closing the write stream.
    let confirmed = async { read_document(file, &query_info(file).await?).await }.await;
    Ok(confirmed
        .ok()
        .filter(|doc| doc.text.as_bytes() == saved)
        .map(|doc| doc.version))
}

pub async fn open(file: gio::File, parent: &gtk::Window) -> Result<bool, glib::Error> {
    let info = match query_info(&file).await {
        Err(error) if error.matches(gio::IOErrorEnum::NotMounted) => {
            let operation = gtk::MountOperation::new(Some(parent));
            if let Err(error) = file
                .mount_enclosing_volume_future(gio::MountMountFlags::NONE, Some(&operation))
                .await
                && !error.matches(gio::IOErrorEnum::AlreadyMounted)
            {
                return Err(error);
            }
            query_info(&file).await?
        }
        result => result?,
    };
    if info.file_type() == gio::FileType::Directory {
        return Ok(true);
    }
    let document = read_document(&file, &info).await?;
    editor(file, document, parent);
    Ok(false)
}

fn editor(file: gio::File, document: Document, parent: &gtk::Window) {
    let window = gtk::Window::builder()
        .title("Редактор · Доступ администратора")
        .transient_for(parent)
        .modal(true)
        .default_width(680)
        .default_height(480)
        .build();
    let heading = gtk::HeaderBar::new();
    let save = gtk::Button::with_label("Сохранить");
    save.add_css_class("suggested-action");
    save.set_sensitive(false);
    heading.pack_end(&save);
    window.set_titlebar(Some(&heading));
    let body = gtk::Box::new(gtk::Orientation::Vertical, 8);
    let path = gtk::Label::new(Some(&file.parse_name()));
    path.set_xalign(0.0);
    path.set_selectable(true);
    path.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
    path.set_margin_start(12);
    path.set_margin_end(12);
    body.append(&path);
    let input = gtk::TextView::builder()
        .monospace(true)
        .top_margin(8)
        .bottom_margin(8)
        .left_margin(12)
        .right_margin(12)
        .build();
    let buffer = input.buffer();
    buffer.set_text(&document.text);
    body.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hexpand(true)
            .child(&input)
            .build(),
    );
    let notice = gtk::Label::new(None);
    notice.set_wrap(true);
    notice.set_margin_bottom(8);
    body.append(&notice);
    window.set_child(Some(&body));

    let version = Rc::new(RefCell::new(Some(document.version)));
    let dirty = Rc::new(Cell::new(false));
    let busy = Rc::new(Cell::new(false));
    let changed_button = save.downgrade();
    let dirty_state = dirty.clone();
    let current = version.clone();
    buffer.connect_changed(move |_| {
        dirty_state.set(true);
        if let Some(button) = changed_button.upgrade() {
            button.set_sensitive(current.borrow().is_some());
        }
    });

    let ready = busy.clone();
    let dirty_state = dirty.clone();
    let parent_window = window.downgrade();
    save.connect_clicked(move |button| {
        let Some(current) = version.borrow_mut().take() else {
            return;
        };
        if ready.replace(true) {
            return;
        }
        button.set_sensitive(false);
        input.set_editable(false);
        notice.set_text("Сохранение…");
        let text = buffer
            .text(&buffer.start_iter(), &buffer.end_iter(), true)
            .to_string();
        let (file, button, input, notice, ready, dirty, version) = (
            file.clone(),
            button.clone(),
            input.clone(),
            notice.clone(),
            ready.clone(),
            dirty_state.clone(),
            version.clone(),
        );
        glib::spawn_future_local(async move {
            match save_document(&file, text, &current).await {
                Ok(next) => {
                    dirty.set(false);
                    notice.set_text(if next.is_some() {
                        "Сохранено"
                    } else {
                        "Сохранено. Откройте файл заново, чтобы продолжить редактирование."
                    });
                    input.set_editable(next.is_some());
                    *version.borrow_mut() = next;
                }
                Err(error) => {
                    mosaic_shell_tools::diagnostics::event("files", "admin_save_failed", -1);
                    notice.set_text(error.message());
                    *version.borrow_mut() = Some(current);
                    input.set_editable(true);
                    button.set_sensitive(true);
                }
            }
            ready.set(false);
        });
    });

    let confirm_pending = Rc::new(Cell::new(false));
    window.connect_close_request(move |_| {
        if busy.get() {
            return glib::Propagation::Stop;
        }
        if !dirty.get() {
            return glib::Propagation::Proceed;
        }
        if !confirm_pending.replace(true)
            && let Some(window) = parent_window.upgrade()
        {
            let pending = confirm_pending.clone();
            let dirty = dirty.clone();
            glib::spawn_future_local(async move {
                let dialog = gtk::AlertDialog::builder()
                    .message("Закрыть без сохранения?")
                    .buttons(["Продолжить редактирование", "Закрыть"])
                    .cancel_button(0)
                    .default_button(0)
                    .modal(true)
                    .build();
                let choice = dialog.choose_future(Some(&window)).await;
                pending.set(false);
                if choice == Ok(1) {
                    dirty.set(false);
                    window.close();
                }
            });
        }
        glib::Propagation::Stop
    });
    window.present();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        ffi::OsStr, fs, os::unix::ffi::OsStrExt, os::unix::fs::PermissionsExt, path::PathBuf,
    };

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(glib::mkdtemp(std::env::temp_dir().join("mosaic-admin-test-XXXXXX")).unwrap())
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn admin_uri_keeps_non_utf8_filename_and_escapes_delimiters() {
        let path = OsStr::from_bytes(b"/example/notes %?#\xff");
        let original = gio::File::for_path(path);
        let uri = target(&original).unwrap().uri();
        assert_eq!(uri, "admin:///example/notes%20%25%3F%23%FF");
        assert!(target(&gio::File::for_uri("smb://example/share")).is_none());
    }

    #[test]
    fn editor_round_trips_bytes_and_preserves_mode() {
        let directory = TestDirectory::new();
        let path = directory.0.join("notes");
        fs::write(&path, "old\r\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let file = gio::File::for_path(&path);
        glib::MainContext::new().block_on(async {
            let document = read_document(&file, &query_info(&file).await.unwrap())
                .await
                .unwrap();
            assert_eq!(document.text, "old\r\n");
            let next = save_document(&file, "new\r\n".into(), &document.version)
                .await
                .unwrap()
                .unwrap();
            save_document(&file, "ещё\r\n".into(), &next)
                .await
                .unwrap()
                .unwrap();
        });
        assert_eq!(fs::read(&path).unwrap(), "ещё\r\n".as_bytes());
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }

    #[test]
    fn editor_preserves_a_file_modified_or_removed_since_reading() {
        let directory = TestDirectory::new();
        let path = directory.0.join("notes");
        fs::write(&path, "old").unwrap();
        let file = gio::File::for_path(&path);
        glib::MainContext::new().block_on(async {
            let document = read_document(&file, &query_info(&file).await.unwrap())
                .await
                .unwrap();
            fs::write(&path, "external change").unwrap();
            let error = save_document(&file, "edited".into(), &document.version)
                .await
                .unwrap_err();
            assert!(error.matches(gio::IOErrorEnum::WrongEtag));
            assert_eq!(fs::read(&path).unwrap(), b"external change");
            fs::remove_file(&path).unwrap();
            let error = save_document(&file, "edited".into(), &document.version)
                .await
                .unwrap_err();
            assert!(error.matches(gio::IOErrorEnum::NotFound));
            assert!(!path.exists());
        });
    }

    #[test]
    fn editor_rejects_binary_large_and_symlink_targets() {
        let directory = TestDirectory::new();
        let path = directory.0.join("notes");
        let file = gio::File::for_path(&path);
        glib::MainContext::new().block_on(async {
            for bytes in [
                b"binary\x00".as_slice(),
                b"invalid\xff",
                &vec![b'x'; MAX_TEXT_BYTES + 1],
            ] {
                fs::write(&path, bytes).unwrap();
                assert!(
                    read_document(&file, &query_info(&file).await.unwrap())
                        .await
                        .is_err()
                );
            }
            let link_path = directory.0.join("link");
            std::os::unix::fs::symlink(&path, &link_path).unwrap();
            let link = gio::File::for_path(&link_path);
            let error = read_document(&link, &query_info(&link).await.unwrap())
                .await
                .unwrap_err();
            assert!(error.matches(gio::IOErrorEnum::NotRegularFile));
        });
    }
}
