use crate::address::AddressBar;
use crate::filesystem::{
    self, CreateKind, FileEntry, display_location, resolve_location, valid_name,
};
use crate::navigation::{History, Navigation};
use crate::transfer::{self, Batch, Kind, TransferError};
use crate::{Browser, admin, archives, photos, properties, templates};
use glib::translate::IntoGlib;
use gtk::{gdk, gio, glib, pango, prelude::*};
use std::{
    cell::{Cell, RefCell},
    collections::HashSet,
    path::{Path, PathBuf},
    rc::{Rc, Weak},
};

type ItemContext = Rc<RefCell<Weak<Tab>>>;

#[derive(Clone, Copy)]
enum ItemAction {
    Open,
    Pack,
    Unpack,
    OpenAsAdministrator,
    ToggleBookmark,
    CopyPath,
    Properties,
    Create(CreateKind),
    Command(Command),
}

impl ItemAction {
    fn hidden_when_disabled(self) -> bool {
        matches!(
            self,
            Self::OpenAsAdministrator | Self::ToggleBookmark | Self::Pack | Self::Unpack
        )
    }
}

#[derive(Default)]
pub struct Clipboard {
    entry: RefCell<Option<Batch>>,
    busy: Cell<bool>,
    tabs: RefCell<Vec<Weak<Tab>>>,
}

impl Clipboard {
    fn tabs(&self) -> Vec<Rc<Tab>> {
        let mut tabs = self.tabs.borrow_mut();
        tabs.retain(|tab| tab.strong_count() > 0);
        tabs.iter().filter_map(Weak::upgrade).collect()
    }

    fn sync(&self) {
        for tab in self.tabs() {
            tab.update_controls();
        }
    }
}

#[derive(Clone, Copy)]
pub enum Command {
    Back,
    Forward,
    Up,
    Refresh,
    Location,
    Search,
    Hidden,
    NewFolder,
    Rename,
    Trash,
    Clip(Kind),
    Paste,
    Details,
    Icons,
}

pub struct Tab {
    pub page: gtk::Box,
    pub title: gtk::Label,
    history: RefCell<History>,
    request: RefCell<Option<glib::JoinHandle<()>>>,
    loaded: Cell<bool>,
    mutating: Cell<bool>,
    address: AddressBar,
    context_menu: RefCell<Option<gtk::Popover>>,
    search: gtk::SearchEntry,
    hidden: gtk::ToggleButton,
    back: gtk::Button,
    forward: gtk::Button,
    up: gtk::Button,
    refresh: gtk::Button,
    create: gtk::Button,
    rename: gtk::Button,
    trash: gtk::Button,
    copy: gtk::Button,
    cut: gtk::Button,
    paste: gtk::Button,
    clipboard: Rc<Clipboard>,
    browser: Weak<Browser>,
    status: gtk::Label,
    notice: gtk::Label,
    empty: gtk::Label,
    content: gtk::Stack,
    view_mode: gtk::DropDown,
    rows: gio::ListStore,
    filtered: gtk::FilterListModel,
    filter: gtk::CustomFilter,
    selection: gtk::MultiSelection,
}

impl Tab {
    pub fn new(
        directory: &gio::File,
        clipboard: Rc<Clipboard>,
        browser: Weak<Browser>,
    ) -> Rc<Self> {
        let page = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let title = gtk::Label::new(Some(&display_location(directory)));
        title.set_width_chars(8);
        title.set_max_width_chars(21);
        title.set_ellipsize(pango::EllipsizeMode::Middle);

        let toolbar = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        toolbar.add_css_class("files-toolbar");
        let back = icon_button("go-previous-symbolic", "Назад (Alt+←)");
        let forward = icon_button("go-next-symbolic", "Вперёд (Alt+→)");
        let up = icon_button("go-up-symbolic", "На уровень выше (Alt+↑)");
        let refresh = icon_button("view-refresh-symbolic", "Обновить (F5)");
        let address = AddressBar::new();
        toolbar.append(&back);
        toolbar.append(&forward);
        toolbar.append(&up);
        toolbar.append(&refresh);
        toolbar.append(&address.widget);
        page.append(&toolbar);

        let actions = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        actions.add_css_class("files-actions");
        let create = icon_button("folder-new-symbolic", "Новая папка (Ctrl+Shift+N)");
        let rename = icon_button("document-edit-symbolic", "Переименовать (F2)");
        let trash = icon_button("user-trash-symbolic", "Переместить в корзину");
        let hidden = gtk::ToggleButton::builder()
            .icon_name("view-reveal-symbolic")
            .tooltip_text("Показывать скрытые файлы (Ctrl+H)")
            .build();
        let search = gtk::SearchEntry::builder()
            .placeholder_text("Поиск в этой папке")
            .hexpand(true)
            .build();
        actions.append(&create);
        actions.append(&rename);
        actions.append(&trash);
        actions.append(&hidden);
        let cut = icon_button("edit-cut-symbolic", "Вырезать (Ctrl+X)");
        let copy = icon_button("edit-copy-symbolic", "Копировать (Ctrl+C)");
        let paste = icon_button("edit-paste-symbolic", "Вставить (Ctrl+V)");
        actions.append(&cut);
        actions.append(&copy);
        actions.append(&paste);
        actions.append(&search);
        let view_mode = gtk::DropDown::from_strings(&["Таблица", "Значки"]);
        view_mode.set_tooltip_text(Some("Вид папки (Ctrl+1 / Ctrl+2)"));
        view_mode.update_property(&[gtk::accessible::Property::Label("Вид папки")]);
        actions.append(&view_mode);
        page.append(&actions);

        let notice = gtk::Label::new(None);
        notice.add_css_class("files-notice");
        notice.set_wrap(true);
        notice.set_xalign(0.0);
        notice.set_selectable(true);
        notice.set_max_width_chars(60);
        notice.set_visible(false);
        page.append(&notice);

        let rows = gio::ListStore::new::<glib::BoxedAnyObject>();
        let filter = gtk::CustomFilter::new(|_| true);
        let filtered = gtk::FilterListModel::new(Some(rows.clone()), Some(filter.clone()));
        let item_context: ItemContext = Rc::default();
        let (list, selection) = file_list(&filtered, &item_context);
        let grid = gtk::GridView::new(
            Some(selection.clone()),
            Some(file_factory(true, &item_context)),
        );
        grid.add_css_class("files-grid");
        grid.set_min_columns(1);
        grid.set_max_columns(9);
        grid.set_enable_rubberband(true);
        let empty = gtk::Label::new(None);
        empty.add_css_class("files-empty");
        empty.set_wrap(true);
        empty.set_justify(gtk::Justification::Center);
        let content = gtk::Stack::builder().vexpand(true).build();
        content.add_named(&file_scroller(&list), Some("details"));
        content.add_named(&file_scroller(&grid), Some("icons"));
        content.add_named(&empty, Some("empty"));
        page.append(&content);

        let status = gtk::Label::new(None);
        status.add_css_class("files-status");
        status.set_xalign(0.0);
        status.set_ellipsize(pango::EllipsizeMode::End);
        page.append(&status);
        let tab = Rc::new(Self {
            page,
            title,
            history: RefCell::new(History::new(directory.uri().into())),
            request: RefCell::new(None),
            loaded: Cell::new(false),
            mutating: Cell::new(false),
            address,
            context_menu: RefCell::default(),
            search,
            hidden,
            back,
            forward,
            up,
            refresh,
            create,
            rename,
            trash,
            cut,
            copy,
            paste,
            clipboard,
            browser,
            status,
            notice,
            empty,
            content,
            view_mode,
            rows,
            filtered,
            filter,
            selection,
        });
        *item_context.borrow_mut() = Rc::downgrade(&tab);
        tab.connect(&list, &grid);
        tab.sync_address();
        tab.clipboard.tabs.borrow_mut().push(Rc::downgrade(&tab));
        tab.update_filter();
        tab.update_controls();
        tab
    }

    fn connect(self: &Rc<Self>, list: &gtk::ColumnView, grid: &gtk::GridView) {
        for (button, command) in [
            (&self.back, Command::Back),
            (&self.forward, Command::Forward),
            (&self.up, Command::Up),
            (&self.refresh, Command::Refresh),
            (&self.create, Command::NewFolder),
            (&self.rename, Command::Rename),
            (&self.trash, Command::Trash),
            (&self.cut, Command::Clip(Kind::Move)),
            (&self.copy, Command::Clip(Kind::Copy)),
            (&self.paste, Command::Paste),
            (&self.address.edit, Command::Location),
        ] {
            let tab = Rc::downgrade(self);
            button.connect_clicked(move |_| {
                if let Some(tab) = tab.upgrade() {
                    tab.command(command);
                }
            });
        }
        let tab = Rc::downgrade(self);
        self.address.entry.connect_activate(move |entry| {
            if let Some(tab) = tab.upgrade() {
                let text = entry.text();
                tab.address.finish();
                if !text.is_empty() {
                    tab.open_request(resolve_location(&text, &tab.directory()));
                }
            }
        });
        let tab = Rc::downgrade(self);
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, modifiers| {
            let relevant = gdk::ModifierType::CONTROL_MASK
                | gdk::ModifierType::SHIFT_MASK
                | gdk::ModifierType::ALT_MASK
                | gdk::ModifierType::SUPER_MASK;
            let Some(tab) = tab.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if modifiers & relevant != gdk::ModifierType::CONTROL_MASK
                || tab
                    .parent_window()
                    .and_then(|window| GtkWindowExt::focus(&window))
                    .is_some_and(|focus| {
                        focus.is::<gtk::Text>()
                            || focus.is::<gtk::Entry>()
                            || focus.is::<gtk::SearchEntry>()
                            || focus.is::<gtk::Label>()
                    })
            {
                return glib::Propagation::Proceed;
            }
            let command = match key.to_lower() {
                gdk::Key::a => {
                    tab.selection.select_all();
                    return glib::Propagation::Stop;
                }
                gdk::Key::x => Command::Clip(Kind::Move),
                gdk::Key::c => Command::Clip(Kind::Copy),
                gdk::Key::v => Command::Paste,
                _ => return glib::Propagation::Proceed,
            };
            tab.command(command);
            glib::Propagation::Stop
        });
        self.page.add_controller(keys);
        let tab = Rc::downgrade(self);
        self.search.connect_search_changed(move |_| {
            if let Some(tab) = tab.upgrade() {
                tab.update_filter();
            }
        });
        self.search
            .connect_stop_search(|search| search.set_text(""));
        let tab = Rc::downgrade(self);
        self.hidden.connect_toggled(move |_| {
            if let Some(tab) = tab.upgrade() {
                tab.update_filter();
            }
        });
        let tab = Rc::downgrade(self);
        self.filtered.connect_items_changed(move |_, _, _, _| {
            if let Some(tab) = tab.upgrade() {
                tab.update_status();
            }
        });
        let tab = Rc::downgrade(self);
        self.selection.connect_selection_changed(move |_, _, _| {
            if let Some(tab) = tab.upgrade() {
                tab.close_context_menu();
                tab.update_controls();
                tab.update_status();
            }
        });
        let tab = Rc::downgrade(self);
        list.connect_activate(move |_, position| {
            if let Some(tab) = tab.upgrade() {
                tab.open_at(position);
            }
        });
        let tab = Rc::downgrade(self);
        grid.connect_activate(move |_, position| {
            if let Some(tab) = tab.upgrade() {
                tab.open_at(position);
            }
        });
        let tab = Rc::downgrade(self);
        self.view_mode.connect_selected_notify(move |_| {
            if let Some(tab) = tab.upgrade() {
                tab.close_context_menu();
                tab.update_status();
            }
        });
        let tab = Rc::downgrade(self);
        self.page.connect_unmap(move |_| {
            if let Some(tab) = tab.upgrade() {
                tab.close_context_menu();
            }
        });
        let tab = Rc::downgrade(self);
        let context_click = gtk::GestureClick::new();
        context_click.set_button(gdk::BUTTON_SECONDARY);
        context_click.set_propagation_phase(gtk::PropagationPhase::Capture);
        context_click.connect_released(move |gesture, _, x, y| {
            let (Some(tab), Some(anchor)) = (tab.upgrade(), gesture.widget()) else {
                return;
            };
            let mut target = anchor.pick(x, y, gtk::PickFlags::DEFAULT);
            while let Some(widget) = target {
                if widget == anchor {
                    break;
                }
                if ["files-name", "files-detail", "files-tile"]
                    .iter()
                    .any(|class| widget.has_css_class(class))
                {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    let _ = widget.activate_action("files-row.menu", Some(&(x, y).to_variant()));
                    return;
                }
                target = widget.parent();
            }
            if tab.create.is_sensitive() {
                gesture.set_state(gtk::EventSequenceState::Claimed);
                tab.show_context_menu(None, &anchor, x, y);
            }
        });
        self.content.add_controller(context_click);
    }

    fn sync_address(self: &Rc<Self>) {
        let tab = Rc::downgrade(self);
        self.address.update(&self.directory(), move |target| {
            if let Some(tab) = tab.upgrade()
                && !target.equal(&tab.directory())
            {
                tab.open_directory(target.clone());
            }
        });
    }

    fn open_at(self: &Rc<Self>, position: u32) {
        if !self.loading()
            && let Some(entry) = self
                .selection
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
        {
            let entry = entry.borrow::<FileEntry>();
            if entry.is_directory {
                self.open_directory(entry.file.clone());
            } else {
                self.open_file(entry.file.clone());
            }
        }
    }

    fn show_context_menu(
        self: &Rc<Self>,
        position: Option<u32>,
        anchor: &gtk::Widget,
        x: f64,
        y: f64,
    ) {
        if self.loading()
            || self.mutating.get()
            || position.is_some_and(|position| position >= self.selection.n_items())
        {
            return;
        }
        let point = gtk::graphene::Point::new(x as f32, y as f32);
        let Some(point) = anchor.compute_point(&self.page, &point) else {
            return;
        };
        self.close_context_menu();
        if let Some(position) = position
            && !self.selection.is_selected(position)
        {
            self.selection.select_item(position, true);
        }
        let menu = gtk::Box::new(gtk::Orientation::Vertical, 1);
        menu.add_css_class("files-context-actions");
        let directory = self.directory();
        let item = position.and_then(|position| self.entry_target(position));
        let local_folder = item
            .as_ref()
            .filter(|(_, directory)| *directory)
            .and_then(|(file, _)| file.path());
        let local_directory = directory.path();
        let archive = item
            .as_ref()
            .filter(|(_, folder)| !folder)
            .and_then(|(file, _)| file.path())
            .is_some_and(|path| archives::is_zip(&path));
        let single_archive = archive && self.selection.selection().size() == 1;
        let pinned = local_folder
            .as_deref()
            .or(local_directory.as_deref())
            .is_some_and(|path| {
                self.browser
                    .upgrade()
                    .is_some_and(|browser| browser.is_bookmarked(path))
            });
        let bookmark = if pinned {
            "Открепить папку"
        } else {
            "Закрепить папку слева"
        };
        let create = [
            (
                "Новый файл…",
                ItemAction::Create(CreateKind::File),
                self.create.is_sensitive(),
            ),
            (
                "Новая папка…",
                ItemAction::Create(CreateKind::Folder),
                self.create.is_sensitive(),
            ),
        ];
        let item_sections: &[&[(&str, ItemAction, bool)]] = &[
            &[(
                if archive {
                    "Открыть архив"
                } else {
                    "Открыть"
                },
                ItemAction::Open,
                true,
            )],
            &[
                (
                    "Вырезать",
                    ItemAction::Command(Command::Clip(Kind::Move)),
                    self.cut.is_sensitive(),
                ),
                (
                    "Копировать",
                    ItemAction::Command(Command::Clip(Kind::Copy)),
                    self.copy.is_sensitive(),
                ),
                ("Копировать путь", ItemAction::CopyPath, true),
            ],
            &[
                (
                    "Переименовать",
                    ItemAction::Command(Command::Rename),
                    self.rename.is_sensitive(),
                ),
                (
                    "В корзину",
                    ItemAction::Command(Command::Trash),
                    self.trash.is_sensitive(),
                ),
            ],
            &[("Свойства", ItemAction::Properties, true)],
            &[
                (
                    bookmark,
                    ItemAction::ToggleBookmark,
                    local_folder.is_some() && self.selection.selection().size() == 1,
                ),
                ("Запаковать…", ItemAction::Pack, local_directory.is_some()),
                ("Распаковать…", ItemAction::Unpack, single_archive),
                (
                    "Открыть с правами администратора…",
                    ItemAction::OpenAsAdministrator,
                    item.as_ref().is_some_and(|(file, _)| file.path().is_some()),
                ),
            ],
        ];
        let background_sections: &[&[(&str, ItemAction, bool)]] = &[
            &create,
            &[(
                bookmark,
                ItemAction::ToggleBookmark,
                local_directory.is_some() && self.loaded.get(),
            )],
        ];
        let sections = if position.is_some() {
            item_sections
        } else {
            background_sections
        };
        for (index, entries) in sections.iter().enumerate() {
            if !entries
                .iter()
                .any(|(_, item, enabled)| *enabled || !item.hidden_when_disabled())
            {
                continue;
            }
            if index != 0 {
                menu.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
            }
            for &(label, item, enabled) in *entries {
                if !enabled && item.hidden_when_disabled() {
                    continue;
                }
                let action = gtk::Button::with_label(label);
                if let Some(label) = action.child().and_downcast::<gtk::Label>() {
                    label.set_xalign(0.0);
                }
                action.add_css_class("flat");
                action.set_sensitive(enabled);
                let tab = Rc::downgrade(self);
                action.connect_clicked(move |_| {
                    if let Some(tab) = tab.upgrade() {
                        tab.close_context_menu();
                        tab.run_item_action(position, item);
                    }
                });
                menu.append(&action);
            }
        }
        let popup = gtk::Popover::new();
        popup.add_css_class("files-context-menu");
        popup.set_has_arrow(false);
        popup.set_autohide(true);
        popup.set_position(gtk::PositionType::Bottom);
        popup.set_pointing_to(Some(&gdk::Rectangle::new(
            point.x() as i32,
            point.y() as i32,
            1,
            1,
        )));
        if position.is_some() {
            // Labwc can resize a popup to the usable output height. Keep the child
            // shrinkable so GTK can display the menu and scroll to the remaining actions.
            let scroll = gtk::ScrolledWindow::builder()
                .hscrollbar_policy(gtk::PolicyType::Never)
                .vscrollbar_policy(gtk::PolicyType::Automatic)
                .max_content_height(315)
                .propagate_natural_height(true)
                .propagate_natural_width(true)
                .child(&menu)
                .build();
            popup.set_child(Some(&scroll));
        } else {
            popup.set_child(Some(&menu));
        }
        popup.set_parent(&self.page);
        let tab = Rc::downgrade(self);
        popup.connect_closed(move |popup| {
            if let Some(tab) = tab.upgrade() {
                let mut current = tab.context_menu.borrow_mut();
                if current.as_ref() == Some(popup) {
                    current.take();
                }
            }
            unparent_popup(popup);
        });
        *self.context_menu.borrow_mut() = Some(popup.clone());
        popup.popup();
    }

    fn close_context_menu(&self) {
        let popup = self.context_menu.borrow_mut().take();
        if let Some(popup) = popup {
            popup.popdown();
            unparent_popup(&popup);
        }
    }

    fn run_item_action(self: &Rc<Self>, position: Option<u32>, item: ItemAction) {
        match item {
            ItemAction::Command(command) => self.command(command),
            ItemAction::Create(kind) => {
                self.prompt_name(NamePrompt::Create(kind), self.directory());
            }
            ItemAction::Open => {
                if let Some(position) = position {
                    self.open_at(position);
                }
            }
            ItemAction::Pack => {
                if let Some(parent) = self.parent_window()
                    && let Some(directory) = self.directory().path()
                    && let Some(sources) = self
                        .selected_files()
                        .iter()
                        .map(gio::File::path)
                        .collect::<Option<Vec<_>>>()
                    && !sources.is_empty()
                {
                    archives::create(&parent, &directory, sources, self.archive_refresh());
                }
            }
            ItemAction::Unpack => {
                if let Some((file, false)) =
                    position.and_then(|position| self.entry_target(position))
                {
                    self.open_file_with_unpack(file, true);
                }
            }
            ItemAction::OpenAsAdministrator => {
                if let Some((file, _)) = position.and_then(|position| self.entry_target(position)) {
                    self.open_as_administrator(file);
                }
            }
            ItemAction::ToggleBookmark => {
                let file = match position {
                    Some(position) => self
                        .entry_target(position)
                        .filter(|(_, folder)| *folder)
                        .map(|(file, _)| file),
                    None => Some(self.directory()),
                };
                if let Some(file) = file {
                    self.toggle_bookmark(file);
                }
            }
            ItemAction::CopyPath | ItemAction::Properties => {
                let Some(position) = position else {
                    return;
                };
                let Some(row) = self
                    .selection
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
                else {
                    return;
                };
                let entry = row.borrow::<FileEntry>();
                let path = entry.file.parse_name();
                if matches!(item, ItemAction::CopyPath) {
                    self.page.clipboard().set_text(&path);
                } else if let Some(parent) = self.parent_window() {
                    let mut details = format!("Путь: {path}\nТип: {}", entry.type_label);
                    if let Some(size) = entry.size {
                        details.push_str(&format!("\nРазмер: {}", glib::format_size(size)));
                    }
                    if let Some(date) = entry
                        .modified
                        .as_ref()
                        .and_then(|date| date.format("%d.%m.%Y %H:%M").ok())
                    {
                        details.push_str(&format!("\nИзменён: {date}"));
                    }
                    properties::show(&parent, &entry.file, &entry.name, &details);
                }
            }
        }
    }

    fn entry_target(&self, position: u32) -> Option<(gio::File, bool)> {
        let item = self
            .selection
            .item(position)?
            .downcast::<glib::BoxedAnyObject>()
            .ok()?;
        let entry = item.borrow::<FileEntry>();
        Some((entry.file.clone(), entry.is_directory))
    }

    pub fn toggle_bookmark(&self, file: gio::File) {
        if let (Some(path), Some(browser)) = (file.path(), self.browser.upgrade())
            && let Err(error) = browser.change_bookmark(&path, false)
        {
            self.bookmark_error(&error);
        }
    }

    pub fn bookmark_error(&self, error: &str) {
        mosaic_shell_tools::diagnostics::event("files", "bookmark_save_failed", -1);
        self.show_error("Не удалось изменить закрепления", &error);
    }

    fn open_as_administrator(self: &Rc<Self>, file: gio::File) {
        let Some(parent) = self.parent_window() else {
            return;
        };
        let Some(file) = (if file.has_uri_scheme("admin") {
            Some(file)
        } else {
            admin::target(&file)
        }) else {
            self.show_error(
                "Административный доступ недоступен",
                &"Выберите локальный файл или папку.",
            );
            return;
        };
        let tab = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let result = admin::open(file.clone(), &parent).await;
            let Some(tab) = tab.upgrade() else { return };
            match result {
                Ok(true) => tab.open_directory(file),
                Ok(false) => {}
                Err(error) if error.matches(gio::IOErrorEnum::Cancelled) => {}
                Err(error) => tab.file_error(
                    "admin_open_failed",
                    "Не удалось открыть с правами администратора",
                    &error,
                ),
            }
        });
    }

    fn offer_admin(self: &Rc<Self>, file: gio::File) {
        let Some(parent) = self.parent_window() else {
            return;
        };
        let dialog = gtk::AlertDialog::builder()
            .message("Нет доступа")
            .detail(file.parse_name())
            .buttons(["Отмена", "Открыть с правами администратора"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let tab = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if dialog.choose_future(Some(&parent)).await == Ok(1)
                && let Some(tab) = tab.upgrade()
            {
                tab.open_as_administrator(file);
            }
        });
    }

    pub fn directory(&self) -> gio::File {
        gio::File::for_uri(self.history.borrow().current())
    }

    pub fn open_directory(self: &Rc<Self>, directory: gio::File) {
        self.navigate(Navigation::Open(directory.uri().into()));
    }

    pub fn open_request(self: &Rc<Self>, file: gio::File) {
        self.navigate_to(Navigation::Open(file.uri().into()), true, None);
    }

    fn open_file(self: &Rc<Self>, file: gio::File) {
        self.open_file_with_unpack(file, false);
    }

    fn archive_refresh(self: &Rc<Self>) -> impl Fn() + Clone + 'static {
        let tab = Rc::downgrade(self);
        move || {
            if let Some(tab) = tab.upgrade() {
                tab.command(Command::Refresh);
            }
        }
    }

    fn open_file_with_unpack(self: &Rc<Self>, file: gio::File, unpack: bool) {
        if file.has_uri_scheme("admin") {
            self.open_as_administrator(file);
            return;
        }
        let tab = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if file.path().is_some() {
                let info = file
                    .query_info_future(
                        "access::can-read",
                        gio::FileQueryInfoFlags::NONE,
                        glib::Priority::DEFAULT,
                    )
                    .await;
                let denied = match info {
                    Ok(info) => {
                        info.has_attribute("access::can-read") && !info.boolean("access::can-read")
                    }
                    Err(error) => error.matches(gio::IOErrorEnum::PermissionDenied),
                };
                if denied {
                    if let Some(tab) = tab.upgrade() {
                        tab.offer_admin(file);
                    }
                    return;
                }
            }
            if let Some(path) = file.path().filter(|path| archives::is_zip(path))
                && let Some(tab) = tab.upgrade()
                && let Some(parent) = tab.parent_window()
            {
                let refresh = tab.archive_refresh();
                if unpack {
                    archives::unpack_to_folder(&parent, path, move |_| refresh());
                } else if let Some(browser) = tab.browser.upgrade() {
                    archives::show(&parent, path, browser.archive_copies.clone(), refresh);
                }
                return;
            }
            if let Some(photo) = photos::Photo::from_file(&file)
                && let Some(tab) = tab.upgrade()
                && let Some(parent) = tab.parent_window()
            {
                let gallery = (0..tab.selection.n_items())
                    .filter_map(|position| tab.entry_target(position))
                    .filter(|(_, directory)| !directory)
                    .filter_map(|(file, _)| photos::Photo::from_file(&file))
                    .collect();
                photos::show(&parent, gallery, photo);
                return;
            }
            let context = gdk::Display::default().map(|display| display.app_launch_context());
            let result =
                gio::AppInfo::launch_default_for_uri_future(&file.uri(), context.as_ref()).await;
            if let Err(error) = result
                && let Some(tab) = tab.upgrade()
            {
                if file.path().is_some() && error.matches(gio::IOErrorEnum::PermissionDenied) {
                    tab.offer_admin(file);
                } else {
                    tab.file_error("open_file_failed", "Не удалось открыть файл", &error);
                }
            }
        });
    }

    fn loading(&self) -> bool {
        self.request.borrow().is_some()
    }

    fn navigate(self: &Rc<Self>, navigation: Navigation) {
        self.navigate_to(navigation, false, None);
    }

    fn navigate_to(
        self: &Rc<Self>,
        navigation: Navigation,
        allow_file: bool,
        focus: Option<gio::File>,
    ) {
        if self.mutating.get() {
            return;
        }
        let Some(uri) = self
            .history
            .borrow()
            .destination(&navigation)
            .map(str::to_owned)
        else {
            return;
        };
        self.close_context_menu();
        self.address.finish();
        if let Some(previous) = self.request.borrow_mut().take() {
            previous.abort();
        }
        let file = gio::File::for_uri(&uri);
        self.notice.set_visible(false);
        let tab = Rc::downgrade(self);
        let task = glib::spawn_future_local(async move {
            let result = filesystem::read_directory(&file).await;
            let Some(tab) = tab.upgrade() else { return };
            tab.request.borrow_mut().take();
            match result {
                Ok(entries) => {
                    let changed = tab.history.borrow().current() != uri;
                    tab.history.borrow_mut().commit(navigation);
                    tab.loaded.set(true);
                    tab.title.set_text(&display_location(&file));
                    tab.title.set_tooltip_text(Some(&file.parse_name()));
                    if changed {
                        tab.sync_address();
                        tab.search.set_text("");
                        // SearchEntry emits search-changed after a delay.
                        tab.update_filter();
                    }
                    let rows: Vec<_> = entries.into_iter().map(glib::BoxedAnyObject::new).collect();
                    tab.rows.splice(0, tab.rows.n_items(), &rows);
                    if let Some(focus) = focus {
                        tab.select_created(&focus);
                    }
                }
                Err(error) if allow_file && error.matches(gio::IOErrorEnum::NotDirectory) => {
                    if let Some(parent) = file.parent() {
                        tab.open_directory(parent);
                    }
                    tab.open_file(file);
                }
                Err(error)
                    if file.path().is_some()
                        && error.matches(gio::IOErrorEnum::PermissionDenied) =>
                {
                    tab.offer_admin(file);
                }
                Err(error) => {
                    tab.file_error("open_directory_failed", "Не удалось открыть папку", &error)
                }
            }
            tab.update_controls();
            tab.update_status();
        });
        *self.request.borrow_mut() = Some(task);
        self.update_controls();
        self.update_status();
    }

    pub fn command(self: &Rc<Self>, command: Command) {
        match command {
            Command::Back => self.navigate(Navigation::Back),
            Command::Forward => self.navigate(Navigation::Forward),
            Command::Refresh => self.navigate(Navigation::Reload),
            Command::Up => {
                if let Some(parent) = self.directory().parent() {
                    self.open_directory(parent);
                }
            }
            Command::Location => self.address.begin(&self.directory()),
            Command::Search => {
                self.search.grab_focus();
            }
            Command::Hidden => self.hidden.set_active(!self.hidden.is_active()),
            Command::NewFolder if self.create.is_sensitive() => {
                self.prompt_name(NamePrompt::Create(CreateKind::Folder), self.directory())
            }
            Command::Rename if self.rename.is_sensitive() => {
                self.prompt_name(NamePrompt::Rename, self.directory())
            }
            Command::Trash if self.trash.is_sensitive() => self.confirm_trash(),
            Command::Clip(kind) if self.copy.is_sensitive() => {
                let files = self.selected_files();
                if let Some(sources) = files
                    .iter()
                    .map(gio::File::path)
                    .collect::<Option<Vec<_>>>()
                    && !sources.is_empty()
                {
                    *self.clipboard.entry.borrow_mut() = Some(Batch { sources, kind });
                    self.clipboard.sync();
                    if kind == Kind::Copy
                        && let Err(error) =
                            self.page.clipboard().set_content(Some(&file_offer(&files)))
                    {
                        self.show_error("Не удалось скопировать файлы в системный буфер", &error);
                    }
                }
            }
            Command::Paste if self.paste.is_sensitive() => self.paste_selection(),
            Command::Details => self.view_mode.set_selected(0),
            Command::Icons => self.view_mode.set_selected(1),
            _ => {}
        }
    }

    fn update_controls(&self) {
        let history = self.history.borrow();
        let can_navigate = !self.mutating.get();
        self.back
            .set_sensitive(can_navigate && history.destination(&Navigation::Back).is_some());
        self.forward
            .set_sensitive(can_navigate && history.destination(&Navigation::Forward).is_some());
        let directory = gio::File::for_uri(history.current());
        self.up
            .set_sensitive(can_navigate && directory.parent().is_some());
        self.refresh.set_sensitive(can_navigate);
        self.address.widget.set_sensitive(can_navigate);
        let editable = self.loaded.get()
            && !self.loading()
            && can_navigate
            && !directory.has_uri_scheme("trash")
            && !directory.has_uri_scheme("admin");
        let selected = self.selection.selection().size();
        self.create.set_sensitive(editable);
        self.rename.set_sensitive(editable && selected == 1);
        self.trash.set_sensitive(editable && selected != 0);
        let clipboard_ready = editable && !self.clipboard.busy.get() && directory.path().is_some();
        let local_selection = clipboard_ready && selected != 0;
        self.copy.set_sensitive(local_selection);
        self.cut.set_sensitive(local_selection);
        self.paste
            .set_sensitive(clipboard_ready && self.clipboard.entry.borrow().is_some());
    }

    fn update_filter(&self) {
        let query = glib::casefold(self.search.text());
        let show_hidden = self.hidden.is_active();
        self.filter.set_filter_func(move |row| {
            let item = row
                .downcast_ref::<glib::BoxedAnyObject>()
                .expect("file list rows use BoxedAnyObject")
                .borrow::<FileEntry>();
            (show_hidden || !item.is_hidden) && item.search_name.contains(query.as_str())
        });
        self.update_status();
    }

    fn update_status(&self) {
        let loading = self.loading();
        let count = self.filtered.n_items();
        if count > 0 {
            self.content
                .set_visible_child_name(if self.view_mode.selected() == 1 {
                    "icons"
                } else {
                    "details"
                });
        } else {
            let text = if loading {
                "Чтение папки…"
            } else if !self.loaded.get() {
                "Выберите место или укажите путь"
            } else if !self.search.text().is_empty() {
                "По этому запросу ничего не найдено"
            } else if self.rows.n_items() > 0 {
                "В папке только скрытые файлы"
            } else {
                "Папка пуста"
            };
            self.empty.set_text(text);
            self.content.set_visible_child_name("empty");
        }
        let selected = self.selection.selection().size();
        let status = if loading {
            "Чтение папки…".to_owned()
        } else if self.mutating.get() {
            "Выполняется операция…".to_owned()
        } else if let Some((_, name)) = self.selected() {
            format!("Выбрано: {name}")
        } else if selected > 0 {
            format!("Выбрано объектов: {selected}")
        } else {
            format!("Объектов: {count}")
        };
        self.status.set_text(&status);
        self.status.set_tooltip_text(Some(&status));
    }

    fn show_error(&self, message: &str, error: &impl std::fmt::Display) {
        self.notice.set_text(&format!("{message}\n{error}"));
        self.notice.set_visible(true);
    }

    fn file_error(&self, event: &str, message: &str, error: &glib::Error) {
        let code = error
            .kind::<gio::IOErrorEnum>()
            .map_or(-1, IntoGlib::into_glib);
        mosaic_shell_tools::diagnostics::event("files", event, code);
        self.show_error(message, error);
    }

    fn paste_selection(self: &Rc<Self>) {
        let (Some(batch), Some(directory)) = (
            self.clipboard.entry.borrow().clone(),
            self.directory().path(),
        ) else {
            return;
        };
        if self.clipboard.busy.replace(true) {
            return;
        }
        self.mutating.set(true);
        self.notice.set_visible(false);
        self.clipboard.sync();
        self.update_status();
        let tab = Rc::downgrade(self);
        let clipboard = self.clipboard.clone();
        let hold = self
            .parent_window()
            .and_then(|window| window.application())
            .map(|app| app.hold());
        glib::spawn_future_local(async move {
            let _hold = hold;
            let total = batch.sources.len();
            let kind = batch.kind;
            let mut completed = HashSet::new();
            let mut errors = 0;
            let mut first_error = None;
            for source in &batch.sources {
                let mut result = transfer_job(source.clone(), directory.clone(), kind, false).await;
                if matches!(result, Err(TransferError::Conflict)) {
                    match conflict_choice(&tab, source, total > 1).await {
                        Some(1) => {
                            result =
                                transfer_job(source.clone(), directory.clone(), kind, true).await;
                        }
                        Some(2) => continue,
                        _ => break,
                    }
                }
                match result {
                    Ok(_) => {
                        completed.insert(source.clone());
                    }
                    Err(error) => {
                        errors += 1;
                        first_error.get_or_insert(error);
                    }
                }
            }
            let affected: HashSet<_> = completed.iter().filter_map(|path| path.parent()).collect();
            let inserted = completed.len();
            *clipboard.entry.borrow_mut() = batch.after_paste(&completed);
            finish_paste(&clipboard, &tab);
            if inserted > 0 {
                for open in clipboard.tabs() {
                    let current = open.directory().path();
                    if !open.loading()
                        && current.as_deref().is_some_and(|current| {
                            current == directory
                                || (kind == Kind::Move && affected.contains(current))
                        })
                    {
                        open.navigate(Navigation::Reload);
                    }
                }
            }
            if let Some(error) = first_error {
                let code = match &error {
                    TransferError::Conflict => -(gio::IOErrorEnum::Exists.into_glib()),
                    TransferError::Io(error) => -error.raw_os_error().unwrap_or(1),
                    TransferError::Gio(error) => error
                        .kind::<gio::IOErrorEnum>()
                        .map_or(-1, IntoGlib::into_glib),
                };
                mosaic_shell_tools::diagnostics::event("files", "paste_failed", code);
                if let Some(tab) = tab.upgrade() {
                    tab.show_error(&format!("Вставлено: {inserted}. Ошибок: {errors}."), &error);
                }
            } else if total > 1
                && inserted < total
                && let Some(tab) = tab.upgrade()
            {
                tab.notice
                    .set_text(&format!("Вставлено: {inserted} из {total}."));
                tab.notice.set_visible(true);
            }
        });
    }

    fn selected(&self) -> Option<(gio::File, glib::GString)> {
        let positions = self.selection.selection();
        if positions.size() != 1 {
            return None;
        }
        let selected = self
            .selection
            .item(positions.nth(0))
            .and_downcast::<glib::BoxedAnyObject>()?;
        let entry = selected.borrow::<FileEntry>();
        Some((entry.file.clone(), entry.name.clone()))
    }

    fn selected_files(&self) -> Vec<gio::File> {
        let positions = self.selection.selection();
        let Some((rest, first)) = gtk::BitsetIter::init_first(&positions) else {
            return Vec::new();
        };
        std::iter::once(first)
            .chain(rest)
            .filter_map(|position| {
                self.selection
                    .item(position)
                    .and_downcast::<glib::BoxedAnyObject>()
            })
            .map(|entry| entry.borrow::<FileEntry>().file.clone())
            .collect()
    }

    fn parent_window(&self) -> Option<gtk::Window> {
        self.page.root().and_downcast()
    }

    fn select_created(&self, file: &gio::File) {
        let position = (0..self.selection.n_items()).find(|&position| {
            self.selection
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
                .is_some_and(|entry| entry.borrow::<FileEntry>().file.equal(file))
        });
        let Some(position) = position else {
            return;
        };
        self.selection.select_item(position, true);
    }

    fn prompt_name(self: &Rc<Self>, action: NamePrompt, directory: gio::File) {
        let Some(parent) = self.parent_window() else {
            return;
        };
        let target = if matches!(action, NamePrompt::Rename) {
            let Some(selected) = self.selected() else {
                return;
            };
            Some(selected)
        } else {
            None
        };
        let label = match action {
            NamePrompt::Rename => "Переименовать",
            NamePrompt::Create(CreateKind::File) => "Новый файл",
            NamePrompt::Create(CreateKind::Folder) => "Новая папка",
        };
        let dialog = gtk::Window::builder()
            .title(label)
            .transient_for(&parent)
            .destroy_with_parent(true)
            .modal(true)
            .default_width(360)
            .resizable(false)
            .build();
        dialog.set_application(parent.application().as_ref());
        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_top(16);
        content.set_margin_bottom(16);
        content.set_margin_start(16);
        content.set_margin_end(16);
        let caption = gtk::Label::new(Some("Имя"));
        caption.set_xalign(0.0);
        let entry = gtk::Entry::builder().activates_default(true).build();
        if let Some((_, original)) = &target {
            entry.set_text(original);
        }
        let picker = if matches!(action, NamePrompt::Create(CreateKind::File))
            && directory.path().is_some()
        {
            templates::Picker::add(&content, &entry)
        } else {
            None
        };
        caption.set_mnemonic_widget(Some(&entry));
        content.append(&caption);
        content.append(&entry);
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        buttons.set_halign(gtk::Align::End);
        let cancel = gtk::Button::with_label("Отмена");
        let save = gtk::Button::with_label(if matches!(action, NamePrompt::Rename) {
            "Сохранить"
        } else {
            "Создать"
        });
        save.add_css_class("suggested-action");
        save.set_sensitive(valid_name(&entry.text()));
        buttons.append(&cancel);
        buttons.append(&save);
        content.append(&buttons);
        let keyboard = gtk::EventControllerKey::new();
        let weak_dialog = dialog.downgrade();
        keyboard.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape
                && let Some(dialog) = weak_dialog.upgrade()
            {
                dialog.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        dialog.add_controller(keyboard);
        dialog.set_child(Some(&content));
        dialog.set_default_widget(Some(&save));
        GtkWindowExt::set_focus(&dialog, Some(&entry));
        entry.select_region(0, -1);

        let weak_save = save.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(save) = weak_save.upgrade() {
                save.set_sensitive(valid_name(&entry.text()));
            }
        });
        let weak_dialog = dialog.downgrade();
        cancel.connect_clicked(move |_| {
            if let Some(dialog) = weak_dialog.upgrade() {
                dialog.close();
            }
        });
        let weak_dialog = dialog.downgrade();
        let weak_entry = entry.downgrade();
        let tab = Rc::downgrade(self);
        save.connect_clicked(move |_| {
            let (Some(tab), Some(entry), Some(dialog)) =
                (tab.upgrade(), weak_entry.upgrade(), weak_dialog.upgrade())
            else {
                return;
            };
            let name = entry.text();
            if !valid_name(&name) {
                return;
            }
            dialog.close();
            let action = match (action, &target) {
                (NamePrompt::Create(kind), _) => Operation::Create(
                    name,
                    kind,
                    picker.as_ref().and_then(templates::Picker::selected),
                ),
                (NamePrompt::Rename, Some((file, original))) if *original != name => {
                    Operation::Rename(file.clone(), name)
                }
                _ => return,
            };
            tab.perform(directory.clone(), action);
        });
        dialog.present();
    }

    fn confirm_trash(self: &Rc<Self>) {
        let files = self.selected_files();
        let Some(parent) = self.parent_window().filter(|_| !files.is_empty()) else {
            return;
        };
        let directory = self.directory();
        let question = self.selected().map_or_else(
            || {
                format!(
                    "Переместить выделенные объекты в корзину? ({})",
                    files.len()
                )
            },
            |(_, name)| format!("Переместить «{name}» в корзину?"),
        );
        let confirmation = gtk::AlertDialog::builder()
            .message(question)
            .buttons(["Отмена", "В корзину"])
            .cancel_button(0)
            .default_button(0)
            .modal(true)
            .build();
        let tab = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            if confirmation.choose_future(Some(&parent)).await == Ok(1)
                && let Some(tab) = tab.upgrade()
            {
                tab.perform(directory, Operation::Trash(files));
            }
        });
    }

    fn perform(self: &Rc<Self>, directory: gio::File, operation: Operation) {
        if self.mutating.replace(true) {
            return;
        }
        self.notice.set_visible(false);
        self.update_controls();
        self.update_status();
        let tab = Rc::downgrade(self);
        let hold = self
            .parent_window()
            .and_then(|window| window.application())
            .map(|app| app.hold());
        glib::spawn_future_local(async move {
            let _hold = hold;
            let mut trashed = None;
            let mut created = None;
            let (result, event, message) = match operation {
                Operation::Create(name, kind, template) => {
                    let from_template = template.is_some();
                    let result = match template {
                        Some(path) => templates::create(&directory, &name, path).await,
                        None => filesystem::create_entry(&directory, &name, kind).await,
                    }
                    .map(|file| created = Some(file));
                    match kind {
                        CreateKind::File if from_template => (
                            result,
                            "create_from_template_failed",
                            "Не удалось создать файл из шаблона",
                        ),
                        CreateKind::File => {
                            (result, "create_file_failed", "Не удалось создать файл")
                        }
                        CreateKind::Folder => {
                            (result, "create_folder_failed", "Не удалось создать папку")
                        }
                    }
                }
                Operation::Rename(file, name) => (
                    file.set_display_name_future(&name, glib::Priority::DEFAULT)
                        .await
                        .map(|_| ()),
                    "rename_failed",
                    "Не удалось переименовать объект",
                ),
                Operation::Trash(files) => {
                    let total = files.len();
                    let mut succeeded = 0;
                    let mut first_error = None;
                    for file in files {
                        match file.trash_future(glib::Priority::DEFAULT).await {
                            Ok(()) => succeeded += 1,
                            Err(error) => {
                                first_error.get_or_insert(error);
                            }
                        }
                    }
                    trashed = Some((succeeded, total));
                    (
                        first_error.map_or(Ok(()), Err),
                        "trash_failed",
                        "Не удалось переместить в корзину",
                    )
                }
            };
            let Some(tab) = tab.upgrade() else { return };
            tab.mutating.set(false);
            let changed = result.is_ok() || trashed.is_some_and(|(done, _)| done > 0);
            if changed && tab.directory().equal(&directory) && !tab.loading() {
                tab.navigate_to(Navigation::Reload, false, created);
            }
            if let Err(error) = result {
                if let Some((done, total)) = trashed
                    && total > 1
                {
                    tab.file_error(
                        event,
                        &format!("В корзине {done} из {total}. {message}"),
                        &error,
                    );
                } else {
                    tab.file_error(event, message, &error);
                }
            }
            tab.update_controls();
            tab.update_status();
        });
    }
}

async fn conflict_choice(tab: &Weak<Tab>, source: &Path, group: bool) -> Option<i32> {
    let window = tab.upgrade()?.parent_window()?;
    let name = source.file_name().map(glib::filename_display_name);
    let detail = name.map_or_else(
        || "Можно вставить объект под новым именем.".to_owned(),
        |name| format!("«{name}» можно вставить под новым именем."),
    );
    let buttons: &[&str] = if group {
        &["Отмена", "Сохранить оба", "Пропустить"]
    } else {
        &["Отмена", "Сохранить оба"]
    };
    gtk::AlertDialog::builder()
        .message("Объект с таким именем уже существует")
        .detail(detail)
        .buttons(buttons)
        .cancel_button(0)
        .default_button(0)
        .modal(true)
        .build()
        .choose_future(Some(&window))
        .await
        .ok()
}

fn finish_paste(clipboard: &Clipboard, tab: &Weak<Tab>) {
    clipboard.busy.set(false);
    if let Some(tab) = tab.upgrade() {
        tab.mutating.set(false);
        tab.update_status();
    }
    clipboard.sync();
}

async fn transfer_job(
    source: PathBuf,
    directory: PathBuf,
    kind: Kind,
    keep_both: bool,
) -> Result<PathBuf, TransferError> {
    gio::spawn_blocking(move || transfer::transfer(&source, &directory, kind, keep_both))
        .await
        .unwrap_or_else(|_| {
            Err(std::io::Error::other("Фоновая операция завершилась ошибкой").into())
        })
}

impl Drop for Tab {
    fn drop(&mut self) {
        self.close_context_menu();
        if let Some(task) = self.request.get_mut().take() {
            task.abort();
        }
    }
}

#[derive(Clone, Copy)]
enum NamePrompt {
    Create(CreateKind),
    Rename,
}

enum Operation {
    Create(glib::GString, CreateKind, Option<PathBuf>),
    Rename(gio::File, glib::GString),
    Trash(Vec<gio::File>),
}

pub fn icon_button(icon: &str, tooltip: &str) -> gtk::Button {
    let button = gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tooltip)
        .build();
    button.update_property(&[gtk::accessible::Property::Label(tooltip)]);
    button
}

fn file_scroller(child: &impl IsA<gtk::Widget>) -> gtk::ScrolledWindow {
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .child(child)
        .build()
}

fn unparent_popup(popup: &gtk::Popover) {
    let popup = popup.clone();
    glib::idle_add_local_once(move || {
        if popup.parent().is_some() {
            popup.unparent();
        }
    });
}

fn file_offer(files: &[gio::File]) -> gdk::ContentProvider {
    gdk::ContentProvider::for_value(&gdk::FileList::from_array(files).to_value())
}

fn connect_item_actions(
    item: &gtk::ListItem,
    widget: &impl IsA<gtk::Widget>,
    context: &ItemContext,
) {
    // A row drag must claim the motion before the view's rubber band does.
    let gesture = gtk::GestureDrag::new();
    let started = Rc::new(Cell::new(false));
    let reset = started.clone();
    gesture.connect_drag_begin(move |_, _, _| reset.set(false));
    let drag_item = item.downgrade();
    let drag_context = context.clone();
    gesture.connect_drag_update(move |gesture, dx, dy| {
        let (Some(tab), Some(item), Some(widget), Some(device)) = (
            drag_context.borrow().upgrade(),
            drag_item.upgrade(),
            gesture.widget(),
            gesture.device(),
        ) else {
            return;
        };
        if started.get()
            || !widget.drag_check_threshold(0, 0, dx.round() as i32, dy.round() as i32)
            || tab.loading()
            || tab.mutating.get()
        {
            return;
        }
        let position = item.position();
        let files = if tab.selection.is_selected(position) {
            tab.selected_files()
        } else {
            let Some((file, _)) = tab.entry_target(position) else {
                return;
            };
            vec![file]
        };
        if files.is_empty() || files.iter().any(|file| file.path().is_none()) {
            return;
        }
        let Some(surface) = widget.native().and_then(|native| native.surface()) else {
            return;
        };
        if let Some(drag) = gdk::Drag::begin(
            &surface,
            &device,
            &file_offer(&files),
            gdk::DragAction::COPY,
            dx,
            dy,
        ) {
            started.set(true);
            gesture.set_state(gtk::EventSequenceState::Claimed);
            let icon = gtk::Image::from_icon_name("text-x-generic-symbolic");
            icon.set_pixel_size(32);
            gtk::DragIcon::for_drag(&drag).set_child(Some(&icon));
            drag.connect_cancel(|drag, _| drag.drop_done(false));
            drag.connect_dnd_finished(|drag| drag.drop_done(true));
        }
    });
    widget.add_controller(gesture);

    let actions = gio::SimpleActionGroup::new();
    let activate =
        gio::SimpleAction::new("menu", Some(<(f64, f64)>::static_variant_type().as_ref()));
    let item = item.downgrade();
    let context = context.clone();
    activate.connect_activate(move |_, value| {
        if let (Some(tab), Some(item), Some((x, y))) = (
            context.borrow().upgrade(),
            item.upgrade(),
            value.and_then(glib::Variant::get::<(f64, f64)>),
        ) {
            tab.show_context_menu(Some(item.position()), tab.content.upcast_ref(), x, y);
        }
    });
    actions.add_action(&activate);
    widget.insert_action_group("files-row", Some(&actions));
}

fn file_factory(tile: bool, context: &ItemContext) -> gtk::SignalListItemFactory {
    let factory = gtk::SignalListItemFactory::new();
    let context = context.clone();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let icon = gtk::Image::new();
        let name = gtk::Label::new(None);
        let row = if tile {
            icon.set_pixel_size(48);
            name.set_width_chars(10);
            name.set_max_width_chars(14);
            name.set_wrap(true);
            name.set_wrap_mode(pango::WrapMode::WordChar);
            name.set_lines(2);
            name.set_justify(gtk::Justification::Center);
            let row = gtk::Box::new(gtk::Orientation::Vertical, 7);
            row.add_css_class("files-tile");
            row
        } else {
            name.set_xalign(0.0);
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 9);
            row.add_css_class("files-name");
            row
        };
        name.set_hexpand(true);
        name.set_ellipsize(pango::EllipsizeMode::End);
        row.append(&icon);
        row.append(&name);
        connect_item_actions(item, &row, &context);
        item.set_child(Some(&row));
    });
    factory.connect_bind(|_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        let Some(row) = item.child().and_downcast::<gtk::Box>() else {
            return;
        };
        let entry = data.borrow::<FileEntry>();
        if let Some(icon) = row.first_child().and_downcast::<gtk::Image>() {
            if let Some(source) = &entry.icon {
                icon.set_from_gicon(source);
            } else {
                icon.set_icon_name(Some(if entry.is_directory {
                    "folder-symbolic"
                } else {
                    "text-x-generic-symbolic"
                }));
            }
        }
        if let Some(label) = row.last_child().and_downcast::<gtk::Label>() {
            label.set_text(&entry.name);
            label.set_tooltip_text(Some(&entry.name));
        }
    });
    factory
}

fn file_list(
    filtered: &gtk::FilterListModel,
    context: &ItemContext,
) -> (gtk::ColumnView, gtk::MultiSelection) {
    let view = gtk::ColumnView::new(None::<gtk::SelectionModel>);
    view.add_css_class("files-list");
    view.set_show_row_separators(true);
    view.set_enable_rubberband(true);
    let name = gtk::ColumnViewColumn::new(Some("Имя"), Some(file_factory(false, context)));
    name.set_expand(true);
    name.set_sorter(Some(&file_sorter(|a, b| a.name_key.cmp(&b.name_key))));
    name.set_resizable(true);
    view.append_column(&name);

    let kind = details_column("Тип", 140, context, |entry| entry.type_label.clone());
    kind.set_sorter(Some(&file_sorter(|a, b| a.type_label.cmp(&b.type_label))));
    view.append_column(&kind);

    let size = details_column("Размер", 96, context, |entry| {
        entry.size.map(glib::format_size).unwrap_or_default()
    });
    size.set_sorter(Some(&file_sorter(|a, b| a.size.cmp(&b.size))));
    view.append_column(&size);

    let modified = details_column("Изменён", 155, context, |entry| {
        entry
            .modified
            .as_ref()
            .and_then(|date| date.format("%d.%m.%Y %H:%M").ok())
            .unwrap_or_default()
    });
    modified.set_sorter(Some(&file_sorter(|a, b| a.modified.cmp(&b.modified))));
    view.append_column(&modified);
    view.sort_by_column(Some(&name), gtk::SortType::Ascending);
    let sorters = gtk::MultiSorter::new();
    sorters.append(file_sorter(|a, b| b.is_directory.cmp(&a.is_directory)));
    if let Some(active) = view.sorter() {
        sorters.append(active);
    }
    let model = gtk::SortListModel::new(Some(filtered.clone()), Some(sorters));
    let selection = gtk::MultiSelection::new(Some(model));
    view.set_model(Some(&selection));
    (view, selection)
}

fn file_sorter(compare: fn(&FileEntry, &FileEntry) -> std::cmp::Ordering) -> gtk::CustomSorter {
    gtk::CustomSorter::new(move |a, b| {
        let a = a.downcast_ref::<glib::BoxedAnyObject>().expect("file row");
        let b = b.downcast_ref::<glib::BoxedAnyObject>().expect("file row");
        compare(&a.borrow(), &b.borrow()).into()
    })
}

fn details_column(
    title: &str,
    width: i32,
    context: &ItemContext,
    text: impl Fn(&FileEntry) -> glib::GString + 'static,
) -> gtk::ColumnViewColumn {
    let factory = gtk::SignalListItemFactory::new();
    let context = context.clone();
    factory.connect_setup(move |_, item| {
        if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
            let label = gtk::Label::new(None);
            label.add_css_class("files-detail");
            label.set_xalign(0.0);
            label.set_ellipsize(pango::EllipsizeMode::End);
            connect_item_actions(item, &label, &context);
            item.set_child(Some(&label));
        }
    });
    factory.connect_bind(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let Some(data) = item.item().and_downcast::<glib::BoxedAnyObject>() else {
            return;
        };
        if let Some(label) = item.child().and_downcast::<gtk::Label>() {
            let value = text(&data.borrow::<FileEntry>());
            label.set_text(&value);
            label.set_tooltip_text(Some(&value));
        }
    });
    let column = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    column.set_fixed_width(width);
    column.set_resizable(true);
    column
}
