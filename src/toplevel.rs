use crate::desktop::Window;
use gtk::glib;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    os::fd::{AsFd, AsRawFd},
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, delegate_noop, event_created_child,
    protocol::{wl_output, wl_registry, wl_seat},
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as image_handle, ext_foreign_toplevel_list_v1 as image_list,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
};

pub type Snapshot = (Vec<Window>, bool);
struct Entry {
    handle: handle::ZwlrForeignToplevelHandleV1,
    window: Window,
    outputs: HashSet<u32>,
    identity_ready: bool,
    app_id_exact: bool,
    title_exact: bool,
}
struct Output {
    global: u32,
    name: String,
    proxy: wl_output::WlOutput,
}
struct State {
    outputs: HashMap<u32, Output>,
    epoch: u64,
    next: u32,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<manager::ZwlrForeignToplevelManagerV1>,
    entries: HashMap<u32, Entry>,
    preview: PreviewCatalog,
    tx: async_channel::Sender<Snapshot>,
    rx: async_channel::Receiver<Snapshot>,
}
#[derive(Default)]
struct PreviewCatalog {
    list_global: Option<u32>,
    source_global: Option<u32>,
    copy_global: Option<u32>,
    list: Option<image_list::ExtForeignToplevelListV1>,
    windows: HashMap<u32, PreviewWindow>,
}
#[derive(Default)]
struct PreviewWindow {
    app_id: String,
    title: String,
    identifier: String,
    ready: bool,
}
impl PreviewCatalog {
    fn available(&self) -> bool {
        self.list.is_some() && self.source_global.is_some() && self.copy_global.is_some()
    }
}
impl State {
    fn publish(&self, healthy: bool) {
        let mut windows: Vec<_> = self
            .entries
            .values()
            .map(|e| {
                let mut w = e.window.clone();
                w.outputs = e
                    .outputs
                    .iter()
                    .filter_map(|id| self.outputs.get(id))
                    .map(|o| o.name.clone())
                    .filter(|n| !n.is_empty())
                    .collect();
                w.outputs.sort();
                w.outputs.dedup();
                w
            })
            .collect();
        windows.sort_by_key(|w| w.id);
        let value = (windows, healthy);
        if self.tx.is_full() {
            let _ = self.rx.try_recv();
        }
        let _ = self.tx.try_send(value);
    }
}
pub struct Tracker {
    connection: Connection,
    queue: EventQueue<State>,
    state: State,
}
pub type TrackerInit = (Rc<RefCell<Tracker>>, async_channel::Receiver<Snapshot>);
impl Tracker {
    pub fn new() -> Result<TrackerInit, Box<dyn std::error::Error>> {
        let can_open_separate_connection = std::env::var_os("WAYLAND_SOCKET").is_none();
        let connection = Connection::connect_to_env()?;
        let mut queue = connection.new_event_queue();
        let qh = queue.handle();
        let registry = connection.display().get_registry(&qh, ());
        let (tx, rx) = async_channel::bounded(1);
        let mut state = State {
            epoch: SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos() as u64,
            next: 1,
            outputs: HashMap::new(),
            seat: None,
            manager: None,
            entries: HashMap::new(),
            preview: PreviewCatalog::default(),
            tx,
            rx: rx.clone(),
        };
        queue.roundtrip(&mut state)?;
        if can_open_separate_connection
            && state.preview.source_global.is_some()
            && state.preview.copy_global.is_some()
            && let Some(name) = state.preview.list_global
        {
            state.preview.list = Some(registry.bind(name, 1, &qh, ()));
        }
        queue.roundtrip(&mut state)?;
        if state.manager.is_none() || state.seat.is_none() {
            return Err("Labwc requires foreign-toplevel and wl_seat".into());
        }
        state.publish(true);
        let tracker = Rc::new(RefCell::new(Self {
            connection,
            queue,
            state,
        }));
        let fd = tracker.borrow().connection.as_fd().as_raw_fd();
        let weak = Rc::downgrade(&tracker);
        glib_unix::unix_fd_add_local(
            fd,
            glib::IOCondition::IN | glib::IOCondition::HUP | glib::IOCondition::ERR,
            move |_, condition| {
                let Some(tracker) = weak.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let mut tracker = tracker.borrow_mut();
                if condition.intersects(glib::IOCondition::HUP | glib::IOCondition::ERR)
                    || tracker.dispatch().is_err()
                {
                    tracker.state.entries.clear();
                    tracker.state.publish(false);
                    return glib::ControlFlow::Break;
                }
                glib::ControlFlow::Continue
            },
        );
        Ok((tracker, rx))
    }
    fn dispatch(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.queue.dispatch_pending(&mut self.state)?;
        if let Some(guard) = self.queue.prepare_read() {
            guard.read()?;
        }
        self.queue.dispatch_pending(&mut self.state)?;
        self.connection.flush()?;
        Ok(())
    }
    pub(crate) fn preview_available(&self) -> bool {
        self.state.preview.available()
    }
    pub(crate) fn preview_identifier(&self, key: (u64, u32)) -> Option<&str> {
        let state = &self.state;
        if !state.preview.available()
            || state.entries.values().any(|entry| !entry.identity_ready)
            || state.preview.windows.values().any(|entry| !entry.ready)
        {
            return None;
        }
        let target = state.entries.values().find(|entry| {
            (entry.window.epoch, entry.window.id) == key && entry.app_id_exact && entry.title_exact
        })?;
        crate::dock_preview::unique_identifier(
            (&target.window.app_id, &target.window.title),
            state
                .entries
                .values()
                .map(|e| (e.window.app_id.as_str(), e.window.title.as_str())),
            state
                .preview
                .windows
                .values()
                .map(|e| ((e.app_id.as_str(), e.title.as_str()), e.identifier.as_str())),
        )
    }
    pub fn request(&self, epoch: u64, id: u32, action: &str) -> Result<(), String> {
        let entry = self
            .state
            .entries
            .values()
            .find(|e| e.window.epoch == epoch && e.window.id == id)
            .ok_or("Окно уже закрыто или устарело")?;
        match action {
            "activate" => {
                entry.handle.unset_minimized();
                entry
                    .handle
                    .activate(self.state.seat.as_ref().ok_or("Нет seat")?);
            }
            "minimize" => entry.handle.set_minimized(),
            "fullscreen" => entry.handle.set_fullscreen(None),
            "unfullscreen" => entry.handle.unset_fullscreen(),
            "close" => entry.handle.close(),
            _ => return Err("Неизвестное действие".into()),
        }
        self.connection
            .flush()
            .map_err(|_| "Wayland отключён".into())
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        {
            match interface.as_str() {
                "wl_output" => {
                    let output: wl_output::WlOutput = registry.bind(name, version.min(4), qh, ());
                    state.outputs.insert(
                        output.id().protocol_id(),
                        Output {
                            global: name,
                            name: String::new(),
                            proxy: output,
                        },
                    );
                }
                "wl_seat" if state.seat.is_none() => {
                    state.seat = Some(registry.bind(name, 1, qh, ()))
                }
                "zwlr_foreign_toplevel_manager_v1" => {
                    state.manager = Some(registry.bind(name, version.min(3), qh, ()))
                }
                "ext_foreign_toplevel_list_v1" => state.preview.list_global = Some(name),
                "ext_foreign_toplevel_image_capture_source_manager_v1" => {
                    state.preview.source_global = Some(name);
                }
                "ext_image_copy_capture_manager_v1" => state.preview.copy_global = Some(name),
                _ => {}
            }
        } else if let wl_registry::Event::GlobalRemove { name } = event {
            if state.preview.source_global == Some(name) {
                state.preview.source_global = None;
            }
            if state.preview.copy_global == Some(name) {
                state.preview.copy_global = None;
            }
            let removed: Vec<_> = state
                .outputs
                .iter()
                .filter(|(_, o)| o.global == name)
                .map(|(id, _)| *id)
                .collect();
            for id in removed {
                if let Some(o) = state.outputs.remove(&id)
                    && o.proxy.version() >= 3
                {
                    o.proxy.release();
                }
                for e in state.entries.values_mut() {
                    e.outputs.remove(&id);
                }
            }
            state.publish(true);
        }
    }
}
impl Dispatch<wl_output::WlOutput, ()> for State {
    fn event(
        state: &mut Self,
        output: &wl_output::WlOutput,
        event: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_output::Event::Name { name } => {
                if let Some(o) = state.outputs.get_mut(&output.id().protocol_id()) {
                    o.name = name.chars().take(256).collect();
                }
                state.publish(true);
            }
            wl_output::Event::Done => state.publish(true),
            _ => {}
        }
    }
}
impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        state: &mut Self,
        _: &manager::ZwlrForeignToplevelManagerV1,
        event: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            manager::Event::Toplevel { toplevel } if state.entries.len() < 4096 => {
                let id = state.next;
                state.next = state.next.checked_add(1).expect("window id exhausted");
                state.entries.insert(
                    toplevel.id().protocol_id(),
                    Entry {
                        handle: toplevel,
                        outputs: HashSet::new(),
                        identity_ready: false,
                        app_id_exact: false,
                        title_exact: false,
                        window: Window {
                            epoch: state.epoch,
                            id,
                            app_id: String::new(),
                            title: String::new(),
                            active: false,
                            minimized: false,
                            fullscreen: false,
                            outputs: Vec::new(),
                        },
                    },
                );
            }
            manager::Event::Finished => {
                state.entries.clear();
                state.publish(false);
            }
            _ => {}
        }
    }
    event_created_child!(State,manager::ZwlrForeignToplevelManagerV1,[0=>(handle::ZwlrForeignToplevelHandleV1,())]);
}
impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &handle::ZwlrForeignToplevelHandleV1,
        event: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = handle.id().protocol_id();
        let Some(entry) = state.entries.get_mut(&key) else {
            return;
        };
        match event {
            handle::Event::AppId { app_id } => {
                entry.window.app_id = app_id.chars().take(256).collect();
                entry.app_id_exact = entry.window.app_id.len() == app_id.len();
                entry.identity_ready = false;
            }
            handle::Event::Title { title } => {
                entry.window.title = title.chars().take(512).collect();
                entry.title_exact = entry.window.title.len() == title.len();
                entry.identity_ready = false;
            }
            handle::Event::State { state: flags } => {
                entry.window.fullscreen = flags
                    .chunks_exact(4)
                    .any(|v| u32::from_ne_bytes(v.try_into().unwrap()) == 3);
                entry.window.minimized = flags
                    .chunks_exact(4)
                    .any(|v| u32::from_ne_bytes(v.try_into().unwrap()) == 1);
                entry.window.active = flags
                    .chunks_exact(4)
                    .any(|v| u32::from_ne_bytes(v.try_into().unwrap()) == 2);
            }
            handle::Event::OutputEnter { output } => {
                entry.outputs.insert(output.id().protocol_id());
            }
            handle::Event::OutputLeave { output } => {
                entry.outputs.remove(&output.id().protocol_id());
            }
            handle::Event::Done => {
                entry.identity_ready = true;
                state.publish(true);
            }
            handle::Event::Closed => {
                state.entries.remove(&key);
                handle.destroy();
                state.publish(true);
            }
            _ => {}
        }
    }
}
impl Dispatch<image_list::ExtForeignToplevelListV1, ()> for State {
    fn event(
        state: &mut Self,
        list: &image_list::ExtForeignToplevelListV1,
        event: image_list::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            image_list::Event::Toplevel { toplevel } if state.preview.windows.len() < 4096 => {
                state
                    .preview
                    .windows
                    .insert(toplevel.id().protocol_id(), PreviewWindow::default());
            }
            image_list::Event::Toplevel { toplevel } => toplevel.destroy(),
            image_list::Event::Finished => {
                state.preview.list = None;
                state.preview.windows.clear();
                list.destroy();
            }
            _ => {}
        }
    }
    event_created_child!(State,image_list::ExtForeignToplevelListV1,[0=>(image_handle::ExtForeignToplevelHandleV1,())]);
}
impl Dispatch<image_handle::ExtForeignToplevelHandleV1, ()> for State {
    fn event(
        state: &mut Self,
        handle: &image_handle::ExtForeignToplevelHandleV1,
        event: image_handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let key = handle.id().protocol_id();
        let Some(entry) = state.preview.windows.get_mut(&key) else {
            return;
        };
        match event {
            image_handle::Event::AppId { app_id } => {
                entry.app_id = app_id.chars().take(257).collect();
                entry.ready = false;
            }
            image_handle::Event::Title { title } => {
                entry.title = title.chars().take(513).collect();
                entry.ready = false;
            }
            image_handle::Event::Identifier { identifier } if identifier.len() <= 32 => {
                entry.identifier = identifier;
                entry.ready = false;
            }
            image_handle::Event::Done => entry.ready = true,
            image_handle::Event::Closed => {
                state.preview.windows.remove(&key);
                handle.destroy();
            }
            _ => {}
        }
    }
}
delegate_noop!(State:ignore wl_seat::WlSeat);
