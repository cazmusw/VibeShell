use gtk::{gdk, gio, glib, pango, prelude::*};
use mosaic_shell_tools::diagnostics;
use rustix::{
    fd::{AsRawFd, OwnedFd},
    fs::{AtFlags, CWD, FileType, Mode, OFlags, chmodat, fstat, open, statat},
    io::Errno,
};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::Arc,
};

const ACCESS_BITS: [u32; 9] = [
    0o400, 0o200, 0o100, 0o040, 0o020, 0o010, 0o004, 0o002, 0o001,
];

#[derive(Debug, PartialEq, Eq)]
enum Error {
    Link,
    Unsupported,
    Changed,
    System(Errno),
}

impl From<Errno> for Error {
    fn from(error: Errno) -> Self {
        Self::System(error)
    }
}

struct Target {
    path: PathBuf,
    fd: OwnedFd,
}

impl Target {
    fn open(path: PathBuf) -> Result<(Self, u32), Error> {
        let fd = open(
            &path,
            OFlags::PATH | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        let stat = fstat(&fd)?;
        match FileType::from_raw_mode(stat.st_mode) {
            FileType::RegularFile | FileType::Directory => {
                Ok((Self { path, fd }, stat.st_mode & 0o7777))
            }
            FileType::Symlink => Err(Error::Link),
            _ => Err(Error::Unsupported),
        }
    }

    fn mode(&self) -> Result<u32, Errno> {
        Ok(fstat(&self.fd)?.st_mode & 0o7777)
    }

    fn apply(&self, baseline: u32, selected: u32) -> Result<u32, Error> {
        let stat = fstat(&self.fd)?;
        let at_path = match statat(CWD, &self.path, AtFlags::SYMLINK_NOFOLLOW) {
            Err(Errno::NOENT | Errno::NOTDIR) => return Err(Error::Changed),
            result => result?,
        };
        if (at_path.st_dev, at_path.st_ino) != (stat.st_dev, stat.st_ino) {
            return Err(Error::Changed);
        }

        let changed = (baseline ^ selected) & 0o777;
        let mode = Mode::from_raw_mode((stat.st_mode & !changed) | (selected & changed));
        // rustix's chmodat does not accept AT_EMPTY_PATH on Linux. The procfs
        // descriptor refers to the pinned inode even if the pathname changes.
        chmodat(
            CWD,
            format!("/proc/self/fd/{}", self.fd.as_raw_fd()),
            mode,
            AtFlags::empty(),
        )?;
        Ok(self.mode()?)
    }
}

pub fn show(parent: &gtk::Window, file: &gio::File, name: &str, details: &str) {
    let window = gtk::Window::builder()
        .title(format!("Свойства · {name}"))
        .transient_for(parent)
        .destroy_with_parent(true)
        .modal(true)
        .default_width(560)
        .build();
    window.set_application(parent.application().as_ref());
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
    panel.set_margin_top(16);
    panel.set_margin_bottom(16);
    panel.set_margin_start(16);
    panel.set_margin_end(16);
    window.set_child(Some(&panel));

    let overview = gtk::Label::builder()
        .label(details)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(pango::WrapMode::WordChar)
        .max_width_chars(65)
        .selectable(true)
        .build();
    panel.append(&overview);
    panel.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
    let title = gtk::Label::new(Some("Права доступа"));
    title.set_xalign(0.0);
    title.add_css_class("heading");
    panel.append(&title);

    let grid = gtk::Grid::builder()
        .column_spacing(20)
        .row_spacing(8)
        .build();
    let columns = ["Чтение", "Запись", "Выполнение"];
    let rows = ["Владелец", "Группа", "Остальные"];
    for (index, name) in columns.iter().enumerate() {
        let header = gtk::Label::new(Some(name));
        if *name == "Выполнение" {
            header.set_tooltip_text(Some("Для папки — вход и доступ к её содержимому"));
        }
        grid.attach(&header, index as i32 + 1, 0, 1, 1);
    }
    for (index, name) in rows.iter().enumerate() {
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        grid.attach(&label, 0, index as i32 + 1, 1, 1);
    }
    let checks = std::array::from_fn(|index| {
        let row = index / columns.len();
        let column = index % columns.len();
        let check = gtk::CheckButton::new();
        check.set_halign(gtk::Align::Center);
        check.set_sensitive(false);
        let name = format!("{} · {}", rows[row], columns[column]);
        check.update_property(&[gtk::accessible::Property::Label(&name)]);
        grid.attach(&check, column as i32 + 1, row as i32 + 1, 1, 1);
        check
    });
    panel.append(&grid);

    let actual = gtk::Label::builder().xalign(0.0).build();
    actual.add_css_class("dim-label");
    panel.append(&actual);
    let message = gtk::Label::builder().xalign(0.0).wrap(true).build();
    panel.append(&message);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    let apply = gtk::Button::with_label("Применить");
    apply.add_css_class("suggested-action");
    apply.set_tooltip_text(Some("Применить права доступа (Ctrl+S)"));
    apply.set_sensitive(false);
    let close = gtk::Button::with_label("Закрыть");
    buttons.append(&close);
    buttons.append(&apply);
    panel.append(&buttons);

    let editor = Rc::new(Editor {
        target: RefCell::new(None),
        saving: Cell::new(false),
        checks,
        apply,
        actual,
        message,
    });
    let keep_alive = editor.clone();
    window.connect_close_request(move |_| {
        let _ = &keep_alive;
        glib::Propagation::Proceed
    });
    let weak_window = window.downgrade();
    close.connect_clicked(move |_| {
        if let Some(window) = weak_window.upgrade() {
            window.close();
        }
    });
    for check in &editor.checks {
        let weak = Rc::downgrade(&editor);
        check.connect_toggled(move |_| {
            if let Some(editor) = weak.upgrade() {
                editor.update_apply();
            }
        });
    }
    let weak = Rc::downgrade(&editor);
    editor.apply.connect_clicked(move |_| {
        if let Some(editor) = weak.upgrade() {
            editor.save();
        }
    });
    let keys = gtk::EventControllerKey::new();
    keys.set_propagation_phase(gtk::PropagationPhase::Capture);
    let weak_window = window.downgrade();
    let weak_apply = editor.apply.downgrade();
    keys.connect_key_pressed(move |_, key, _, modifiers| {
        if key == gdk::Key::Escape
            && let Some(window) = weak_window.upgrade()
        {
            window.close();
        } else if key == gdk::Key::s && modifiers.contains(gdk::ModifierType::CONTROL_MASK) {
            if let Some(button) = weak_apply.upgrade().filter(|button| button.is_sensitive()) {
                button.emit_clicked();
            }
        } else {
            return glib::Propagation::Proceed;
        }
        glib::Propagation::Stop
    });
    window.add_controller(keys);
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&close));
    window.present();

    let Some(path) = file.path() else {
        editor
            .message
            .set_text("Редактирование прав доступно для локальных файлов и папок.");
        return;
    };
    editor.message.set_text("Чтение прав доступа…");
    let weak = Rc::downgrade(&editor);
    glib::spawn_future_local(async move {
        let result = gio::spawn_blocking(move || Target::open(path)).await;
        let Some(editor) = weak.upgrade() else {
            return;
        };
        match result.unwrap_or(Err(Error::System(Errno::IO))) {
            Ok((target, mode)) => {
                editor.target.replace(Some((Arc::new(target), mode)));
                editor.show_mode(mode);
                editor.set_editable(true);
                editor.message.set_text("");
                editor.checks[0].grab_focus();
            }
            Err(error) => editor.report(error),
        }
    });
}

struct Editor {
    target: RefCell<Option<(Arc<Target>, u32)>>,
    saving: Cell<bool>,
    checks: [gtk::CheckButton; 9],
    apply: gtk::Button,
    actual: gtk::Label,
    message: gtk::Label,
}

impl Editor {
    fn selected(&self) -> u32 {
        self.checks
            .iter()
            .zip(ACCESS_BITS)
            .filter(|(check, _)| check.is_active())
            .map(|(_, bit)| bit)
            .sum()
    }

    fn update_apply(&self) {
        self.apply.set_sensitive(
            !self.saving.get()
                && self
                    .target
                    .borrow()
                    .as_ref()
                    .is_some_and(|(_, mode)| mode & 0o777 != self.selected()),
        );
    }

    fn set_editable(&self, editable: bool) {
        for check in &self.checks {
            check.set_sensitive(editable);
        }
        self.update_apply();
    }

    fn show_mode(&self, mode: u32) {
        if let Some((_, baseline)) = self.target.borrow_mut().as_mut() {
            *baseline = mode;
        }
        for (check, bit) in self.checks.iter().zip(ACCESS_BITS) {
            check.set_active(mode & bit != 0);
        }
        self.actual
            .set_text(&format!("Фактические права: {mode:04o}"));
        self.update_apply();
    }

    fn save(self: &Rc<Self>) {
        if self.saving.get() {
            return;
        }
        let Some((target, baseline)) = self.target.borrow().clone() else {
            return;
        };
        let selected = self.selected();
        if baseline & 0o777 == selected {
            return;
        }
        self.saving.set(true);
        self.set_editable(false);
        self.message.set_text("Сохранение прав доступа…");

        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || {
                let result = target.apply(baseline, selected);
                let actual = if result.is_err() {
                    target.mode().ok()
                } else {
                    None
                };
                (result, actual)
            })
            .await;
            let Some(editor) = weak.upgrade() else {
                return;
            };
            editor.saving.set(false);
            match result.unwrap_or((Err(Error::System(Errno::IO)), None)) {
                (Ok(mode), _) => {
                    editor.show_mode(mode);
                    editor.message.remove_css_class("error");
                    editor.message.set_text("Права доступа сохранены.");
                }
                (Err(error), actual) => {
                    if error == Error::Changed {
                        editor.target.take();
                        editor.actual.set_text("");
                    } else if let Some(mode) = actual {
                        editor.show_mode(mode);
                    }
                    editor.report(error);
                }
            }
            let editable = editor.target.borrow().is_some();
            editor.set_editable(editable);
        });
    }

    fn report(&self, error: Error) {
        let (event, text) = match error {
            Error::Link => (
                None,
                "Это символическая ссылка. Чтобы изменить права, откройте свойства её цели.",
            ),
            Error::Unsupported => (
                None,
                "Редактирование прав доступно для обычных файлов и папок.",
            ),
            Error::Changed => (
                Some(("permissions_target_changed", 0)),
                "Объект перемещён или заменён. Откройте его свойства заново.",
            ),
            Error::System(error) => {
                let text = match error {
                    Errno::ACCESS | Errno::PERM => {
                        "Доступ запрещён. У вас недостаточно прав для этой операции."
                    }
                    Errno::ROFS => "Файловая система доступна только для чтения.",
                    Errno::NOENT => "Файл или папка больше не существует.",
                    _ => "Не удалось прочитать или изменить права доступа.",
                };
                (Some(("permissions_failed", error.raw_os_error())), text)
            }
        };
        if let Some((event, code)) = event {
            diagnostics::event("files", event, code);
            self.message.add_css_class("error");
        }
        self.message.set_text(text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::unix::fs::{PermissionsExt, symlink},
    };

    #[test]
    fn applies_changed_bits_and_keeps_external_changes_and_special_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("folder");
        fs::create_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o1750)).unwrap();
        let child = dir.join("child");
        fs::write(&child, b"unchanged").unwrap();
        fs::set_permissions(&child, fs::Permissions::from_mode(0o600)).unwrap();

        let (target, baseline) = Target::open(dir.clone()).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o1751)).unwrap();
        assert_eq!(target.apply(baseline, 0o550), Ok(0o1551));
        assert_eq!(
            fs::metadata(child).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn uses_the_open_inode_without_read_access_and_refuses_replacement_and_links() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("file");
        fs::write(&path, b"private").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
        let (target, baseline) = Target::open(path.clone()).unwrap();
        assert_eq!(target.apply(baseline, 0o640), Ok(0o640));

        let renamed = temp.path().join("renamed");
        fs::rename(&path, &renamed).unwrap();
        symlink(&renamed, &path).unwrap();
        assert_eq!(target.apply(0o640, 0o777), Err(Error::Changed));
        assert!(matches!(Target::open(path), Err(Error::Link)));
        assert_eq!(
            fs::metadata(renamed).unwrap().permissions().mode() & 0o777,
            0o640
        );
    }
}
