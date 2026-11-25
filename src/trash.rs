use gtk::{gio, glib, prelude::*};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

pub const URI: &str = "trash:///";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TrashState {
    #[default]
    Unavailable,
    Empty,
    Full,
    Clearing,
}
impl TrashState {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Empty => "empty",
            Self::Full => "full",
            Self::Clearing => "clearing",
        }
    }
}

async fn entries(root: &gio::File) -> Result<gio::FileEnumerator, glib::Error> {
    root.enumerate_children_future(
        "standard::name",
        gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
        glib::Priority::DEFAULT,
    )
    .await
}

pub async fn probe(root: &gio::File) -> TrashState {
    let Ok(files) = entries(root).await else {
        return TrashState::Unavailable;
    };
    match files.next_files_future(1, glib::Priority::DEFAULT).await {
        Ok(items) if items.is_empty() => TrashState::Empty,
        Ok(_) => TrashState::Full,
        Err(_) => TrashState::Unavailable,
    }
}

async fn empty(root: &gio::File) -> Result<(), glib::Error> {
    let files = entries(root).await?;
    let mut first_error = None;
    loop {
        let batch = files.next_files_future(32, glib::Priority::DEFAULT).await?;
        if batch.is_empty() {
            return first_error.map_or(Ok(()), Err);
        }
        for info in batch {
            // GVfs deletes directory entries recursively within the trash backend.
            let child = root.child(info.name());
            if !child.has_uri_scheme("trash")
                || child.parent().is_none_or(|parent| !parent.equal(root))
            {
                continue;
            }
            if let Err(error) = child.delete_future(glib::Priority::DEFAULT).await
                && !error.matches(gio::IOErrorEnum::NotFound)
                && first_error.is_none()
            {
                first_error = Some(error);
            }
        }
    }
}

type Changed = Box<dyn Fn(TrashState)>;
pub struct Trash {
    root: gio::File,
    monitor: RefCell<Option<gio::FileMonitor>>,
    owner_watch: RefCell<Option<gio::WatcherId>>,
    state: Cell<TrashState>,
    checking: Cell<bool>,
    again: Cell<bool>,
    changed: Changed,
}
impl Trash {
    pub fn new(changed: impl Fn(TrashState) + 'static) -> Rc<Self> {
        Rc::new(Self {
            root: gio::File::for_uri(URI),
            monitor: RefCell::new(None),
            owner_watch: RefCell::new(None),
            state: Cell::new(TrashState::Unavailable),
            checking: Cell::new(false),
            again: Cell::new(false),
            changed: Box::new(changed),
        })
    }
    pub fn state(&self) -> TrashState {
        self.state.get()
    }
    pub fn start(self: &Rc<Self>) {
        if self.owner_watch.borrow().is_some() {
            self.refresh();
            return;
        }
        let appeared = Rc::downgrade(self);
        let vanished = Rc::downgrade(self);
        let watcher = gio::bus_watch_name(
            gio::BusType::Session,
            "org.gtk.vfs.Daemon",
            gio::BusNameWatcherFlags::NONE,
            move |_, _, _| {
                if let Some(trash) = appeared.upgrade() {
                    trash.refresh();
                }
            },
            move |_, _| {
                if let Some(trash) = vanished.upgrade() {
                    trash.remove_monitor();
                    if trash.state() != TrashState::Clearing {
                        trash.update(TrashState::Unavailable);
                    }
                    if trash.checking.get() {
                        trash.again.set(true);
                    }
                }
            },
        );
        *self.owner_watch.borrow_mut() = Some(watcher);
        self.refresh();
    }
    fn remove_monitor(&self) {
        let old = self.monitor.borrow_mut().take();
        if let Some(monitor) = old {
            monitor.cancel();
        }
    }
    fn update(&self, state: TrashState) {
        if self.state.replace(state) != state {
            (self.changed)(state);
        }
    }
    pub fn refresh(self: &Rc<Self>) {
        if self.checking.replace(true) {
            self.again.set(true);
            return;
        }
        let trash = self.clone();
        glib::spawn_future_local(async move {
            loop {
                let state = probe(&trash.root).await;
                if state == TrashState::Unavailable
                    || trash
                        .monitor
                        .borrow()
                        .as_ref()
                        .is_some_and(|monitor| monitor.is_cancelled())
                {
                    trash.remove_monitor();
                }
                if state != TrashState::Unavailable
                    && trash.monitor.borrow().is_none()
                    && let Ok(monitor) = trash
                        .root
                        .monitor_directory(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE)
                {
                    let weak = Rc::downgrade(&trash);
                    monitor.connect_changed(move |_, _, _, _| {
                        if let Some(trash) = weak.upgrade() {
                            trash.refresh();
                        }
                    });
                    *trash.monitor.borrow_mut() = Some(monitor);
                    trash.again.set(true);
                }
                if trash.state() != TrashState::Clearing {
                    trash.update(state);
                }
                if !trash.again.replace(false) {
                    trash.checking.set(false);
                    break;
                }
            }
        });
    }
    pub fn clear_confirmed(
        self: &Rc<Self>,
        completed: impl FnOnce(Result<(), glib::Error>) + 'static,
    ) {
        if self.state() != TrashState::Full {
            return;
        }
        self.update(TrashState::Clearing);
        let trash = self.clone();
        glib::spawn_future_local(async move {
            let result = empty(&trash.root).await;
            trash.update(probe(&trash.root).await);
            completed(result);
            trash.refresh();
        });
    }
}
impl Drop for Trash {
    fn drop(&mut self) {
        if let Some(watcher) = self.owner_watch.get_mut().take() {
            gio::bus_unwatch_name(watcher);
        }
    }
}
