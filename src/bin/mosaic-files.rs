#[path = "mosaic_files/address.rs"]
mod address;
#[path = "mosaic_files/admin.rs"]
mod admin;
#[path = "mosaic_files/archive_io.rs"]
mod archive_io;
#[path = "mosaic_files/archives.rs"]
mod archives;
#[path = "mosaic_files/bookmarks.rs"]
mod bookmarks;
#[path = "mosaic_files/filesystem.rs"]
mod filesystem;
#[path = "mosaic_files/folder_sizes.rs"]
mod folder_sizes;
#[path = "mosaic_files/navigation.rs"]
mod navigation;
#[path = "mosaic_files/photos.rs"]
mod photos;
#[path = "mosaic_files/properties.rs"]
mod properties;
#[path = "mosaic_files/tab.rs"]
mod tab;
#[path = "mosaic_files/templates.rs"]
mod templates;
#[path = "mosaic_files/transfer.rs"]
mod transfer;

use bookmarks::Bookmarks;
use gtk::{gdk, gio, glib, graphene, prelude::*};
use mosaic_shell_tools::diagnostics;
use std::{
    cell::RefCell,
    path::Path,
    rc::{Rc, Weak},
};
use tab::{Clipboard, Command, Tab, icon_button, unparent_popup};

const APPLICATION_ID: &str = "org.mosaic.Files";

fn main() -> glib::ExitCode {
    diagnostics::install_panic_hook("files");
    diagnostics::event("files", "start", 0);
    let app = gtk::Application::builder()
        .application_id(APPLICATION_ID)
        .flags(gio::ApplicationFlags::HANDLES_OPEN)
        .build();
    app.connect_startup(|app| {
        if let Some(display) = gdk::Display::default() {
            if let Some(icons) = option_env!("MOSAIC_GTK_ICON_DIR") {
                gtk::IconTheme::for_display(&display).add_search_path(icons);
            }
            let provider = gtk::CssProvider::new();
            provider.load_from_string(include_str!("mosaic_files/style.css"));
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
            gtk::Window::set_default_icon_name("folder");
        }
        for (action, keys) in [
            ("new-tab", "<Control>t"),
            ("close-tab", "<Control>w"),
            ("back", "<Alt>Left"),
            ("forward", "<Alt>Right"),
            ("up", "<Alt>Up"),
            ("refresh", "F5"),
            ("location", "<Control>l"),
            ("search", "<Control>f"),
            ("hidden", "<Control>h"),
            ("bookmark", "<Control>d"),
            ("new-folder", "<Control><Shift>n"),
            ("rename", "F2"),
            ("view-details", "<Control>1"),
            ("view-icons", "<Control>2"),
        ] {
            app.set_accels_for_action(&format!("win.{action}"), &[keys]);
        }
    });
    let active: Rc<RefCell<Weak<Browser>>> = Rc::default();
    let current = active.clone();
    app.connect_activate(move |app| {
        let (browser, window) = browser_window(app, &current);
        if browser.notebook.n_pages() == 0 {
            browser.add_tab(home()).command(Command::Refresh);
        }
        window.present();
    });
    app.connect_open(move |app, files, _| {
        let (browser, window) = browser_window(app, &active);
        for file in files {
            browser.add_tab(home()).open_request(file.clone());
        }
        window.present();
    });
    let code = app.run();
    diagnostics::event("files", "exit", i32::from(code.get()));
    code
}

fn browser_window(
    app: &gtk::Application,
    current: &RefCell<Weak<Browser>>,
) -> (Rc<Browser>, gtk::ApplicationWindow) {
    let active = current.borrow().upgrade();
    if let Some(browser) = active
        && let Some(window) = browser.window.upgrade()
    {
        return (browser, window);
    }
    let (browser, window) = Browser::new(app);
    *current.borrow_mut() = Rc::downgrade(&browser);
    (browser, window)
}

struct Browser {
    window: glib::WeakRef<gtk::ApplicationWindow>,
    notebook: gtk::Notebook,
    tabs: RefCell<Vec<Rc<Tab>>>,
    clipboard: Rc<Clipboard>,
    bookmarks: RefCell<Result<Bookmarks, std::io::Error>>,
    pinned: gtk::Box,
    archive_copies: archives::OpenCopies,
    tab_menu: RefCell<Option<gtk::Popover>>,
    folder_sizes: Rc<folder_sizes::Queue>,
}

#[derive(Clone, Copy)]
enum TabAction {
    MoveLeft,
    MoveRight,
    Close,
    CloseOthers,
    CloseLeft,
    CloseRight,
    Refresh,
}

impl Browser {
    fn new(app: &gtk::Application) -> (Rc<Self>, gtk::ApplicationWindow) {
        let window = gtk::ApplicationWindow::builder()
            .application(app)
            .title("Файлы · Mosaic")
            .default_width(1000)
            .default_height(660)
            .build();
        window.add_css_class("mosaic-files");

        let header = gtk::HeaderBar::new();
        let header_title = gtk::Label::new(Some("Файлы"));
        header_title.add_css_class("title");
        header.set_title_widget(Some(&header_title));
        let new_tab = icon_button("tab-new-symbolic", "Новая вкладка (Ctrl+T)");
        new_tab.set_action_name(Some("win.new-tab"));
        header.pack_start(&new_tab);
        window.set_titlebar(Some(&header));

        let notebook = gtk::Notebook::builder()
            .show_border(false)
            .scrollable(true)
            .hexpand(true)
            .vexpand(true)
            .build();
        notebook.add_css_class("files-tabs");

        let browser = Rc::new(Self {
            window: window.downgrade(),
            notebook,
            tabs: RefCell::new(Vec::new()),
            clipboard: Rc::default(),
            bookmarks: RefCell::new(Bookmarks::load()),
            pinned: gtk::Box::new(gtk::Orientation::Vertical, 4),
            archive_copies: Default::default(),
            tab_menu: RefCell::default(),
            folder_sizes: folder_sizes::Queue::new(),
        });
        let content = gtk::Paned::builder()
            .orientation(gtk::Orientation::Horizontal)
            .start_child(&browser.places())
            .end_child(&browser.notebook)
            .position(216)
            .wide_handle(true)
            .resize_start_child(false)
            .shrink_start_child(false)
            .shrink_end_child(false)
            .build();
        window.set_child(Some(&content));

        let keep_alive = browser.clone();
        window.connect_close_request(move |_| {
            // The window owns the controller, and the controller holds the window weakly.
            keep_alive.folder_sizes.cancel();
            let menu = keep_alive.tab_menu.take();
            if let Some(menu) = menu {
                menu.popdown();
                if menu.parent().is_some() {
                    menu.unparent();
                }
            }
            glib::Propagation::Proceed
        });
        browser.install_actions(&window);
        browser.install_file_keys(&window);
        browser.install_tab_menu();
        browser.install_folder_sizes();
        (browser, window)
    }

    fn places(self: &Rc<Self>) -> gtk::ScrolledWindow {
        let places = gtk::Box::new(gtk::Orientation::Vertical, 4);
        places.add_css_class("files-places");
        let title = gtk::Label::new(Some("Места"));
        title.set_xalign(0.0);
        title.add_css_class("files-places-heading");
        places.append(&title);
        let home = home();
        self.add_place(&places, "Домашняя папка", "user-home-symbolic", &home);
        places.append(&self.pinned);
        self.refresh_bookmarks();
        for (location, name, icon) in [
            (
                glib::UserDirectory::Downloads,
                "Загрузки",
                "folder-download-symbolic",
            ),
            (
                glib::UserDirectory::Documents,
                "Документы",
                "folder-documents-symbolic",
            ),
            (
                glib::UserDirectory::Pictures,
                "Изображения",
                "folder-pictures-symbolic",
            ),
            (
                glib::UserDirectory::Music,
                "Музыка",
                "folder-music-symbolic",
            ),
            (
                glib::UserDirectory::Videos,
                "Видео",
                "folder-videos-symbolic",
            ),
        ] {
            if let Some(path) = glib::user_special_dir(location) {
                let file = gio::File::for_path(path);
                if !file.equal(&home) {
                    self.add_place(&places, name, icon, &file);
                }
            }
        }
        places.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        self.add_place(
            &places,
            "Файловая система",
            "drive-harddisk-symbolic",
            &gio::File::for_path("/"),
        );
        self.add_place(
            &places,
            "Корзина",
            "user-trash-symbolic",
            &gio::File::for_uri("trash:///"),
        );
        gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .min_content_width(72)
            .child(&places)
            .build()
    }

    fn add_place(self: &Rc<Self>, places: &gtk::Box, name: &str, icon: &str, file: &gio::File) {
        places.append(&self.place_button(name, icon, file));
    }

    fn place_button(self: &Rc<Self>, name: &str, icon: &str, file: &gio::File) -> gtk::Button {
        let place = gtk::Button::new();
        place.add_css_class("flat");
        place.set_has_frame(false);
        place.set_hexpand(true);
        let content = gtk::Box::new(gtk::Orientation::Horizontal, 9);
        content.append(&gtk::Image::from_icon_name(icon));
        let label = gtk::Label::new(Some(name));
        label.set_xalign(0.0);
        label.set_max_width_chars(22);
        label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        place.set_tooltip_text(Some(&file.parse_name()));
        content.append(&label);
        place.set_child(Some(&content));
        let browser = Rc::downgrade(self);
        let file = file.clone();
        place.connect_clicked(move |_| {
            if let Some(browser) = browser.upgrade()
                && let Some(tab) = browser.current_tab()
            {
                tab.open_directory(file.clone());
            }
        });
        place
    }

    fn is_bookmarked(&self, path: &Path) -> bool {
        self.bookmarks
            .borrow()
            .as_ref()
            .is_ok_and(|bookmarks| bookmarks.contains(path))
    }

    fn change_bookmark(self: &Rc<Self>, path: &Path, remove: bool) -> Result<(), String> {
        let mut stored = self.bookmarks.borrow_mut();
        if stored.is_err() {
            *stored = Bookmarks::load();
        }
        let bookmarks = stored.as_mut().map_err(|error| error.to_string())?;
        if remove {
            bookmarks.remove(path)
        } else {
            bookmarks.toggle(path)
        }
        .map_err(|error| error.to_string())?;
        drop(stored);
        self.refresh_bookmarks();
        Ok(())
    }

    fn refresh_bookmarks(self: &Rc<Self>) {
        while let Some(child) = self.pinned.first_child() {
            self.pinned.remove(&child);
        }
        let stored = self.bookmarks.borrow();
        let paths = stored.as_ref().map_or(&[][..], Bookmarks::paths);
        self.pinned.set_visible(!paths.is_empty());
        if paths.is_empty() {
            return;
        }
        let title = gtk::Label::new(Some("Закреплённые"));
        title.set_xalign(0.0);
        title.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title.add_css_class("files-places-heading");
        self.pinned.append(&title);
        for path in paths {
            let file = gio::File::for_path(path);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 2);
            row.append(&self.place_button(
                &filesystem::display_location(&file),
                "folder-symbolic",
                &file,
            ));
            let remove = icon_button("window-close-symbolic", "Открепить папку");
            remove.add_css_class("flat");
            let browser = Rc::downgrade(self);
            let path = path.clone();
            remove.connect_clicked(move |_| {
                if let Some(browser) = browser.upgrade()
                    && let Err(error) = browser.change_bookmark(&path, true)
                    && let Some(tab) = browser.current_tab()
                {
                    tab.bookmark_error(&error);
                }
            });
            row.append(&remove);
            self.pinned.append(&row);
        }
    }

    fn install_actions(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        self.add_action(window, "bookmark", |browser| {
            if let Some(tab) = browser.current_tab() {
                tab.toggle_bookmark(tab.directory());
            }
        });
        self.add_action(window, "new-tab", |browser| {
            let start = browser
                .current_tab()
                .map_or_else(home, |tab| tab.directory());
            browser.add_tab(start).command(Command::Refresh);
        });
        self.add_action(window, "close-tab", |browser| {
            if let Some(tab) = browser.current_tab() {
                browser.close_tab(&tab);
            }
        });
        for (name, command) in [
            ("back", Command::Back),
            ("forward", Command::Forward),
            ("up", Command::Up),
            ("refresh", Command::Refresh),
            ("location", Command::Location),
            ("search", Command::Search),
            ("hidden", Command::Hidden),
            ("new-folder", Command::NewFolder),
            ("rename", Command::Rename),
            ("view-details", Command::Details),
            ("view-icons", Command::Icons),
        ] {
            self.add_action(window, name, move |browser| {
                if let Some(tab) = browser.current_tab() {
                    tab.command(command);
                }
            });
        }
    }

    fn add_action(
        self: &Rc<Self>,
        window: &gtk::ApplicationWindow,
        name: &str,
        run: impl Fn(&Rc<Self>) + 'static,
    ) {
        let weak = Rc::downgrade(self);
        let action = gio::SimpleAction::new(name, None);
        action.connect_activate(move |_, _| {
            if let Some(browser) = weak.upgrade() {
                run(&browser);
            }
        });
        window.add_action(&action);
    }

    fn install_file_keys(self: &Rc<Self>, window: &gtk::ApplicationWindow) {
        let keys = gtk::EventControllerKey::new();
        keys.set_propagation_phase(gtk::PropagationPhase::Capture);
        let browser = Rc::downgrade(self);
        let weak_window = window.downgrade();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let relevant = gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK;
            if modifiers & relevant != gdk::ModifierType::CONTROL_MASK
                || weak_window
                    .upgrade()
                    .and_then(|window| GtkWindowExt::focus(&window))
                    .is_some_and(|focus| {
                        focus.is::<gtk::Editable>()
                            || focus
                                .downcast_ref::<gtk::Label>()
                                .is_some_and(gtk::Label::is_selectable)
                    })
            {
                return glib::Propagation::Proceed;
            }
            browser
                .upgrade()
                .and_then(|browser| browser.current_tab())
                .map_or(glib::Propagation::Proceed, |tab| tab.handle_file_key(key))
        });
        window.add_controller(keys);
    }

    fn install_tab_menu(self: &Rc<Self>) {
        let menu = gtk::GestureClick::new();
        menu.set_button(gdk::BUTTON_SECONDARY);
        menu.set_propagation_phase(gtk::PropagationPhase::Capture);
        let weak = Rc::downgrade(self);
        menu.connect_pressed(move |gesture, _, x, y| {
            let Some(browser) = weak.upgrade() else {
                return;
            };
            let point = graphene::Point::new(x as f32, y as f32);
            let tab = browser
                .tabs
                .borrow()
                .iter()
                .find(|tab| {
                    browser.notebook.tab_label(&tab.page).is_some_and(|label| {
                        let header = label
                            .parent()
                            .filter(|parent| parent.css_name() == "tab")
                            .unwrap_or(label);
                        header.is_mapped()
                            && header
                                .compute_bounds(&browser.notebook)
                                .is_some_and(|bounds| bounds.contains_point(&point))
                    })
                })
                .cloned();
            if let Some(tab) = tab {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                browser.show_tab_menu(&tab, point);
            }
        });
        self.notebook.add_controller(menu);
    }

    fn add_tab(self: &Rc<Self>, directory: gio::File) -> Rc<Tab> {
        let tab = Tab::new(&directory, self.clipboard.clone(), Rc::downgrade(self));
        let label = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        label.append(&tab.title);
        let close = icon_button("window-close-symbolic", "Закрыть вкладку (Ctrl+W)");
        close.add_css_class("flat");
        close.add_css_class("files-tab-close");
        label.append(&close);
        let browser = Rc::downgrade(self);
        let weak_tab = Rc::downgrade(&tab);
        close.connect_clicked(move |_| {
            if let (Some(browser), Some(tab)) = (browser.upgrade(), weak_tab.upgrade()) {
                browser.close_tab(&tab);
            }
        });
        self.tabs.borrow_mut().push(tab.clone());
        let page = self.notebook.append_page(&tab.page, Some(&label));
        self.notebook.set_tab_reorderable(&tab.page, true);
        self.notebook.set_current_page(Some(page));
        tab
    }

    fn current_tab(&self) -> Option<Rc<Tab>> {
        let page = self.notebook.nth_page(self.notebook.current_page())?;
        self.tabs
            .borrow()
            .iter()
            .find(|tab| tab.page.upcast_ref::<gtk::Widget>() == &page)
            .cloned()
    }

    fn install_folder_sizes(self: &Rc<Self>) {
        let browser = Rc::downgrade(self);
        self.notebook.connect_page_notify(move |_| {
            if let Some(browser) = browser.upgrade() {
                browser.refresh_folder_sizes();
            }
        });
        let browser = Rc::downgrade(self);
        glib::timeout_add_local(folder_sizes::REFRESH_INTERVAL, move || {
            let Some(browser) = browser.upgrade() else {
                return glib::ControlFlow::Break;
            };
            if !browser.folder_sizes.busy() {
                browser.refresh_folder_sizes();
            }
            glib::ControlFlow::Continue
        });
    }

    fn refresh_folder_sizes(&self) {
        if let Some(tab) = self.current_tab() {
            tab.schedule_folder_sizes();
        } else {
            self.folder_sizes.cancel();
        }
    }

    fn close_tab(&self, tab: &Rc<Tab>) {
        let Some(page) = self.notebook.page_num(&tab.page) else {
            return;
        };
        if self.notebook.n_pages() == 1 {
            if let Some(window) = self.window.upgrade() {
                window.close();
            }
        } else {
            self.remove_page(page);
        }
    }

    fn remove_page(&self, page: u32) {
        if let Some(widget) = self.notebook.nth_page(Some(page)) {
            self.notebook.remove_page(Some(page));
            self.tabs
                .borrow_mut()
                .retain(|tab| tab.page.upcast_ref::<gtk::Widget>() != &widget);
        }
    }

    fn show_tab_menu(self: &Rc<Self>, tab: &Rc<Tab>, point: graphene::Point) {
        let Some(index) = self.notebook.page_num(&tab.page) else {
            return;
        };
        let count = self.notebook.n_pages();
        let old = self.tab_menu.borrow_mut().take();
        if let Some(old) = old {
            old.popdown();
            unparent_popup(&old);
        }
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 1);
        menu.add_css_class("files-context-actions");
        let popup = gtk::Popover::new();
        popup.add_css_class("files-context-menu");
        popup.set_has_arrow(false);
        popup.set_position(gtk::PositionType::Bottom);
        popup.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        popup.set_child(Some(&menu));
        popup.set_parent(&self.notebook);

        for (section_number, items) in [
            &[
                ("Переместить влево", TabAction::MoveLeft, index > 0),
                (
                    "Переместить вправо",
                    TabAction::MoveRight,
                    index + 1 < count,
                ),
            ][..],
            &[
                ("Закрыть вкладку", TabAction::Close, true),
                ("Закрыть другие вкладки", TabAction::CloseOthers, count > 1),
                ("Закрыть вкладки слева", TabAction::CloseLeft, index > 0),
                (
                    "Закрыть вкладки справа",
                    TabAction::CloseRight,
                    index + 1 < count,
                ),
                ("Обновить", TabAction::Refresh, true),
            ],
        ]
        .into_iter()
        .enumerate()
        {
            if section_number != 0 {
                menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            for &(name, action, enabled) in items {
                let button = gtk::Button::with_label(name);
                button.add_css_class("flat");
                button.set_sensitive(enabled);
                if let Some(label) = button.child().and_downcast::<gtk::Label>() {
                    label.set_xalign(0.0);
                }
                let browser = Rc::downgrade(self);
                let target = Rc::downgrade(tab);
                let weak_popup = popup.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(popup) = weak_popup.upgrade() {
                        popup.popdown();
                    }
                    let browser = browser.clone();
                    let target = target.clone();
                    glib::idle_add_local_once(move || {
                        if let (Some(browser), Some(target)) = (browser.upgrade(), target.upgrade())
                        {
                            browser.run_tab_action(&target, action);
                        }
                    });
                });
                menu.append(&button);
            }
        }
        let browser = Rc::downgrade(self);
        popup.connect_closed(move |popup| {
            if let Some(browser) = browser.upgrade() {
                let mut current = browser.tab_menu.borrow_mut();
                if current.as_ref() == Some(popup) {
                    current.take();
                }
            }
            unparent_popup(popup);
        });
        self.tab_menu.replace(Some(popup.clone()));
        popup.popup();
    }

    fn run_tab_action(&self, tab: &Rc<Tab>, action: TabAction) {
        let Some(index) = self.notebook.page_num(&tab.page) else {
            return;
        };
        let count = self.notebook.n_pages();
        let pages = match action {
            TabAction::MoveLeft => {
                if index > 0 {
                    self.notebook.reorder_child(&tab.page, Some(index - 1));
                }
                return;
            }
            TabAction::MoveRight => {
                if index + 1 < count {
                    self.notebook.reorder_child(&tab.page, Some(index + 1));
                }
                return;
            }
            TabAction::Close => return self.close_tab(tab),
            TabAction::Refresh => return tab.command(Command::Refresh),
            TabAction::CloseOthers => 0..count,
            TabAction::CloseLeft => 0..index,
            TabAction::CloseRight => index + 1..count,
        };
        self.notebook.set_current_page(Some(index));
        for page in pages.rev().filter(|page| *page != index) {
            self.remove_page(page);
        }
    }
}

fn home() -> gio::File {
    gio::File::for_path(glib::home_dir())
}
