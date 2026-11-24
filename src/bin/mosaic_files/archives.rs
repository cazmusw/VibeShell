use crate::archive_io::{self, Error, Member};
use gtk::{gdk, gio, glib, pango, prelude::*};
use mosaic_shell_tools::diagnostics;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

pub type OpenCopies = Rc<RefCell<Vec<tempfile::TempDir>>>;

pub fn is_zip(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
}

fn dialog(parent: &gtk::Window, title: &str, width: i32) -> gtk::Window {
    let window = gtk::Window::builder()
        .title(title)
        .transient_for(parent)
        .destroy_with_parent(true)
        .modal(true)
        .default_width(width)
        .build();
    window.set_application(parent.application().as_ref());
    let keys = gtk::EventControllerKey::new();
    let weak = window.downgrade();
    keys.connect_key_pressed(move |_, key, _, _| {
        if key == gdk::Key::Escape
            && let Some(window) = weak.upgrade()
        {
            window.close();
            glib::Propagation::Stop
        } else {
            glib::Propagation::Proceed
        }
    });
    window.add_controller(keys);
    window
}

fn content() -> gtk::Box {
    let panel = gtk::Box::new(gtk::Orientation::Vertical, 12);
    panel.set_margin_top(14);
    panel.set_margin_bottom(14);
    panel.set_margin_start(14);
    panel.set_margin_end(14);
    panel
}

fn error_dialog(parent: &gtk::Window, error: Error) {
    let (event, message) = match error {
        Error::Cancelled => return,
        Error::Conflict => (
            "archive_conflict",
            "Такое имя уже существует. Выберите другое место или имя.",
        ),
        Error::Changed => (
            "archive_changed",
            "Файл изменился во время операции. Откройте архив заново.",
        ),
        Error::Io => (
            "archive_io_failed",
            "Не удалось прочитать или записать файл. Проверьте права и свободное место.",
        ),
        Error::Invalid => ("archive_invalid", "Архив повреждён или не является ZIP."),
        Error::Limit => (
            "archive_limit_exceeded",
            "Превышен допустимый размер или число объектов.",
        ),
        Error::UnsafePath => (
            "archive_unsafe_path",
            "Путь, ссылка или специальный файл не подходит для этой операции.",
        ),
        Error::Unsupported => (
            "archive_unsupported",
            "Формат записи, имя файла или шифрование не поддерживается.",
        ),
    };
    diagnostics::event("files", event, 0);
    gtk::AlertDialog::builder()
        .message("Не удалось выполнить операцию с архивом")
        .detail(message)
        .buttons(["Закрыть"])
        .cancel_button(0)
        .modal(true)
        .build()
        .show(Some(parent));
}

fn job<T: Send + 'static>(
    parent: &gtk::Window,
    title: &str,
    work: impl FnOnce(&AtomicBool) -> Result<T, Error> + Send + 'static,
    ready: impl FnOnce(&gtk::Window, Result<T, Error>) + 'static,
) {
    let window = dialog(parent, title, 350);
    window.set_resizable(false);
    let panel = content();
    panel.append(&gtk::Spinner::builder().spinning(true).build());
    let button = gtk::Button::with_label("Отмена");
    panel.append(&button);
    window.set_child(Some(&panel));
    let weak = window.downgrade();
    button.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let cancel = Arc::new(AtomicBool::new(false));
    let on_close = cancel.clone();
    window.connect_close_request(move |_| {
        on_close.store(true, Ordering::Relaxed);
        glib::Propagation::Proceed
    });
    let on_destroy = cancel.clone();
    window.connect_destroy(move |_| on_destroy.store(true, Ordering::Relaxed));
    window.present();

    let window = window.downgrade();
    let parent = parent.downgrade();
    glib::spawn_future_local(async move {
        let token = cancel.clone();
        let result = gio::spawn_blocking(move || work(&token)).await;
        let cancelled = cancel.load(Ordering::Relaxed);
        if let Some(window) = window.upgrade() {
            window.destroy();
        }
        if !cancelled && let Some(parent) = parent.upgrade() {
            ready(&parent, result.unwrap_or(Err(Error::Io)));
        }
    });
}

fn prompt(
    parent: &gtk::Window,
    title: &str,
    label: &str,
    initial: &Path,
    folder: bool,
    accept: impl Fn(&gtk::Window, PathBuf) + 'static,
) {
    let window = dialog(parent, title, 550);
    window.set_resizable(false);
    let panel = content();
    let caption = gtk::Label::new(Some(label));
    caption.set_xalign(0.0);
    panel.append(&caption);

    let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let entry = gtk::Entry::builder()
        .hexpand(true)
        .activates_default(true)
        .build();
    let initial_display = initial.to_string_lossy().into_owned();
    entry.set_text(&initial_display);
    caption.set_mnemonic_widget(Some(&entry));
    row.append(&entry);
    let browse = gtk::Button::with_label("Выбрать…");
    row.append(&browse);
    panel.append(&row);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Отмена");
    let proceed = gtk::Button::with_label(if folder {
        "Распаковать"
    } else {
        "Создать ZIP"
    });
    proceed.add_css_class("suggested-action");
    actions.append(&cancel);
    actions.append(&proceed);
    panel.append(&actions);
    window.set_child(Some(&panel));
    window.set_default_widget(Some(&proceed));
    gtk::prelude::GtkWindowExt::set_focus(&window, Some(&entry));

    let weak = window.downgrade();
    cancel.connect_clicked(move |_| {
        if let Some(window) = weak.upgrade() {
            window.close();
        }
    });
    let weak = window.downgrade();
    let field = entry.downgrade();
    browse.connect_clicked(move |_| {
        let (Some(window), Some(field)) = (weak.upgrade(), field.upgrade()) else {
            return;
        };
        let picker = gtk::FileDialog::builder().modal(true).build();
        let current = PathBuf::from(field.text().as_str());
        let directory = if folder {
            Some(current.as_path())
        } else {
            current.parent()
        };
        if let Some(directory) = directory.filter(|path| path.is_absolute()) {
            picker.set_initial_folder(Some(&gio::File::for_path(directory)));
        }
        if !folder {
            picker.set_initial_name(current.file_name().and_then(|name| name.to_str()));
        }
        let field = field.downgrade();
        glib::spawn_future_local(async move {
            let selected = if folder {
                picker.select_folder_future(Some(&window)).await
            } else {
                picker.save_future(Some(&window)).await
            };
            if let Ok(file) = selected
                && let Some(path) = file.path()
                && let Some(field) = field.upgrade()
            {
                field.set_text(&path.to_string_lossy());
            }
        });
    });
    let weak = window.downgrade();
    let parent = parent.downgrade();
    let suggested = initial.to_owned();
    proceed.connect_clicked(move |_| {
        let (Some(window), Some(parent)) = (weak.upgrade(), parent.upgrade()) else {
            return;
        };
        let text = entry.text();
        let path = if text == initial_display {
            suggested.clone()
        } else {
            PathBuf::from(text.as_str())
        };
        if !path.is_absolute() {
            error_dialog(&window, Error::UnsafePath);
            return;
        }
        window.close();
        accept(&parent, path);
    });
    window.present();
}

pub fn create(
    parent: &gtk::Window,
    directory: &Path,
    sources: Vec<PathBuf>,
    refresh: impl Fn() + Clone + 'static,
) {
    let initial = match sources.as_slice() {
        [source] => {
            let mut basename = source.file_name().unwrap_or_default().to_owned();
            basename.push(".zip");
            directory.join(basename)
        }
        _ => directory.join("Архив.zip"),
    };
    prompt(
        parent,
        "Запаковать в ZIP",
        "Путь к новому архиву",
        &initial,
        false,
        move |parent, mut destination| {
            if !is_zip(&destination) {
                let mut path = destination.into_os_string();
                path.push(".zip");
                destination = path.into();
            }
            let sources = sources.clone();
            let refresh = refresh.clone();
            job(
                parent,
                "Создание архива…",
                move |cancel| archive_io::pack(&sources, &destination, cancel),
                move |parent, result| match result {
                    Ok(()) => refresh(),
                    Err(error) => error_dialog(parent, error),
                },
            );
        },
    );
}

pub fn unpack_to_folder(
    parent: &gtk::Window,
    archive: PathBuf,
    ready: impl Fn(&Path) + Clone + 'static,
) {
    let initial = archive
        .parent()
        .unwrap_or_else(|| Path::new("/"))
        .to_owned();
    prompt(
        parent,
        "Распаковать ZIP",
        "Папка назначения",
        &initial,
        true,
        move |parent, directory| {
            let archive = archive.clone();
            let ready = ready.clone();
            job(
                parent,
                "Распаковка архива…",
                move |cancel| archive_io::unpack(&archive, &directory, cancel),
                move |parent, result| match result {
                    Ok(path) => ready(&path),
                    Err(error) => error_dialog(parent, error),
                },
            );
        },
    );
}

pub fn show(parent: &gtk::Window, path: PathBuf, copies: OpenCopies, refresh: impl Fn() + 'static) {
    let source = path.clone();
    job(
        parent,
        "Чтение ZIP…",
        move |cancel| archive_io::list(&source, cancel),
        move |parent, result| match result {
            Ok(entries) => viewer(parent, path, entries, copies, Rc::new(refresh)),
            Err(error) => error_dialog(parent, error),
        },
    );
}

fn viewer(
    parent: &gtk::Window,
    path: PathBuf,
    entries: Vec<Member>,
    copies: OpenCopies,
    refresh: Rc<dyn Fn()>,
) {
    let title = format!("Архив · {}", glib::filename_display_basename(&path));
    let window = dialog(parent, &title, 740);
    window.set_default_height(480);
    let panel = content();
    let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let unpack = gtk::Button::with_label("Распаковать…");
    unpack.set_tooltip_text(Some("Распаковать архив (Ctrl+E)"));
    let open = gtk::Button::with_label("Открыть файл");
    open.set_tooltip_text(Some("Открыть временную копию в приложении по умолчанию"));
    let status = gtk::Label::new(Some(&format!("Объектов: {}", entries.len())));
    status.set_hexpand(true);
    status.set_xalign(1.0);
    status.set_ellipsize(pango::EllipsizeMode::End);
    toolbar.append(&unpack);
    toolbar.append(&open);
    toolbar.append(&status);
    panel.append(&toolbar);

    let names = gtk::StringList::new(&[]);
    for item in &entries {
        let description = if item.directory {
            "папка".into()
        } else {
            glib::format_size(item.size)
        };
        names.append(&format!("{}  ·  {description}", item.path.display()));
    }
    let entries = Rc::new(entries);
    let selection = gtk::SingleSelection::new(Some(names));
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, object| {
        if let Some(row) = object.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::builder()
                .xalign(0.0)
                .margin_start(8)
                .margin_end(8)
                .margin_top(5)
                .margin_bottom(5)
                .ellipsize(pango::EllipsizeMode::Middle)
                .build();
            row.set_child(Some(&label));
        }
    });
    factory.connect_bind(|_, object| {
        if let Some(row) = object.downcast_ref::<gtk::ListItem>()
            && let Some(label) = row.child().and_downcast::<gtk::Label>()
            && let Some(item) = row.item().and_downcast::<gtk::StringObject>()
        {
            label.set_text(&item.string());
            label.set_tooltip_text(Some(&item.string()));
        }
    });
    let view = gtk::ListView::new(Some(selection.clone()), Some(factory));
    let scroll = gtk::ScrolledWindow::builder()
        .vexpand(true)
        .child(&view)
        .build();
    panel.append(&scroll);

    let selected = entries.clone();
    let open_button = open.downgrade();
    let update_open = move |selection: &gtk::SingleSelection| {
        if let Some(button) = open_button.upgrade() {
            button.set_sensitive(
                selected
                    .get(selection.selected() as usize)
                    .is_some_and(|entry| !entry.directory),
            );
        }
    };
    update_open(&selection);
    selection.connect_selected_notify(update_open);
    let weak = window.downgrade();
    let open_archive = path.clone();
    open.connect_clicked(move |_| {
        let (Some(window), Some(entry)) =
            (weak.upgrade(), entries.get(selection.selected() as usize))
        else {
            return;
        };
        if entry.directory {
            return;
        }
        if copies.borrow().len() >= 16 {
            error_dialog(&window, Error::Limit);
            return;
        }
        let path = open_archive.clone();
        let entry = entry.clone();
        let copies = copies.clone();
        job(
            &window,
            "Подготовка файла…",
            move |cancel| archive_io::stage_member(&path, &entry, cancel),
            move |window, result| match result {
                Ok((temp, path)) => {
                    copies.borrow_mut().push(temp);
                    let window = window.downgrade();
                    glib::spawn_future_local(async move {
                        let file = gio::File::for_path(path);
                        let context =
                            gdk::Display::default().map(|display| display.app_launch_context());
                        if gio::AppInfo::launch_default_for_uri_future(
                            &file.uri(),
                            context.as_ref(),
                        )
                        .await
                        .is_err()
                            && let Some(window) = window.upgrade()
                        {
                            error_dialog(&window, Error::Io);
                        }
                    });
                }
                Err(error) => error_dialog(window, error),
            },
        );
    });
    let open = open.downgrade();
    view.connect_activate(move |_, _| {
        if let Some(button) = open.upgrade()
            && button.is_sensitive()
        {
            button.emit_clicked();
        }
    });

    let weak = window.downgrade();
    let label = status.downgrade();
    unpack.connect_clicked(move |_| {
        let Some(window) = weak.upgrade() else {
            return;
        };
        let refresh = refresh.clone();
        let label = label.clone();
        unpack_to_folder(&window, path.clone(), move |path| {
            refresh();
            if let Some(label) = label.upgrade() {
                label.set_text(&format!(
                    "Распаковано в «{}»",
                    glib::filename_display_basename(path)
                ));
            }
        });
    });
    let action = gio::SimpleAction::new("extract", None);
    let button = unpack.downgrade();
    action.connect_activate(move |_, _| {
        if let Some(button) = button.upgrade() {
            button.emit_clicked();
        }
    });
    let actions = gio::SimpleActionGroup::new();
    actions.add_action(&action);
    window.insert_action_group("archive", Some(&actions));
    let shortcuts = gtk::ShortcutController::new();
    shortcuts.set_scope(gtk::ShortcutScope::Managed);
    shortcuts.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string("<Control>e"),
        Some(gtk::NamedAction::new("archive.extract")),
    ));
    window.add_controller(shortcuts);
    window.set_child(Some(&panel));
    window.present();
}
