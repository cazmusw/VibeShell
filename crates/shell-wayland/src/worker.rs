use crate::{Capabilities, WindowCommand};
use anyhow::{Context, Result};
use shell_core::Window;
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    os::{
        fd::{AsFd, AsRawFd},
        unix::net::UnixStream,
    },
    sync::{mpsc, Arc, Mutex},
    thread,
};
use wayland_client::{
    delegate_noop, event_created_child,
    protocol::{wl_output, wl_registry, wl_seat},
    Connection, Dispatch, QueueHandle,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1 as handle, zwlr_foreign_toplevel_manager_v1 as manager,
};

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub revision: u64,
    pub capabilities: Capabilities,
    pub windows: Vec<Window>,
    pub error: Option<String>,
}
#[derive(Clone)]
pub struct WaylandHandle {
    pub state: Arc<Mutex<Snapshot>>,
    tx: mpsc::SyncSender<WindowCommand>,
    wake: Arc<Mutex<UnixStream>>,
}
impl WaylandHandle {
    pub fn start() -> Result<Self> {
        let (tx, rx) = mpsc::sync_channel(64);
        let (wake, reader) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        reader.set_nonblocking(true)?;
        let state = Arc::new(Mutex::new(Snapshot::default()));
        let shared = state.clone();
        thread::Builder::new()
            .name("mosaic-wayland".into())
            .spawn(move || {
                if let Err(e) = run(rx, reader, shared.clone()) {
                    let mut s = shared.lock().unwrap();
                    s.windows.clear();
                    s.error = Some(e.to_string());
                    s.revision += 1;
                }
            })?;
        Ok(Self {
            state,
            tx,
            wake: Arc::new(Mutex::new(wake)),
        })
    }
    pub fn send(&self, command: WindowCommand) -> Result<()> {
        self.tx
            .try_send(command)
            .context("Очередь команд окон недоступна")?;
        match self.wake.lock().unwrap().write(&[1]) {
            Ok(_) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.state.lock().unwrap().clone()
    }
}
struct Entry {
    handle: handle::ZwlrForeignToplevelHandleV1,
    pending: Window,
}
#[derive(Default)]
struct State {
    caps: Capabilities,
    seat: Option<wl_seat::WlSeat>,
    manager: Option<manager::ZwlrForeignToplevelManagerV1>,
    entries: BTreeMap<u32, Entry>,
    next_id: u32,
    epoch: u64,
    dirty: bool,
}
impl State {
    fn publish(&mut self, shared: &Mutex<Snapshot>) {
        if !self.dirty {
            return;
        }
        self.dirty = false;
        let mut s = shared.lock().unwrap();
        s.revision += 1;
        s.capabilities = Capabilities::from_globals(self.caps.globals.clone());
        s.windows = self.entries.values().map(|e| e.pending.clone()).collect();
    }
    fn command(&self, c: WindowCommand) {
        let (epoch, id) = c.target();
        if epoch != self.epoch {
            return;
        }
        let Some(e) = self.entries.get(&id) else {
            return;
        };
        match c {
            WindowCommand::Activate { .. } => {
                if let Some(seat) = &self.seat {
                    e.handle.unset_minimized();
                    e.handle.activate(seat);
                }
            }
            WindowCommand::Minimize { .. } => e.handle.set_minimized(),
            WindowCommand::Close { .. } => e.handle.close(),
        }
    }
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        r: &wl_registry::WlRegistry,
        e: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = e
        {
            s.caps.globals.push((interface.clone(), version));
            s.dirty = true;
            match interface.as_str() {
                "zwlr_foreign_toplevel_manager_v1" => {
                    s.manager = Some(r.bind(name, version.min(3), q, ()))
                }
                "wl_seat" if s.seat.is_none() => s.seat = Some(r.bind(name, version.min(7), q, ())),
                "wl_output" => {
                    let _: wl_output::WlOutput = r.bind(name, version.min(4), q, ());
                }
                _ => {}
            }
        }
    }
}
impl Dispatch<manager::ZwlrForeignToplevelManagerV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &manager::ZwlrForeignToplevelManagerV1,
        e: manager::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match e {
            manager::Event::Toplevel { toplevel } => {
                s.next_id += 1;
                let id = s.next_id;
                s.entries.insert(
                    id,
                    Entry {
                        handle: toplevel,
                        pending: Window {
                            epoch: s.epoch,
                            id,
                            ..Default::default()
                        },
                    },
                );
            }
            manager::Event::Finished => {
                s.entries.clear();
                s.manager = None;
                s.dirty = true;
            }
            _ => {}
        }
    }
    event_created_child!(State,manager::ZwlrForeignToplevelManagerV1,[0=>(handle::ZwlrForeignToplevelHandleV1,())]);
}
impl Dispatch<handle::ZwlrForeignToplevelHandleV1, ()> for State {
    fn event(
        s: &mut Self,
        h: &handle::ZwlrForeignToplevelHandleV1,
        e: handle::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(id) = s
            .entries
            .iter()
            .find(|(_, v)| v.handle == *h)
            .map(|(k, _)| *k)
        else {
            return;
        };
        if matches!(e, handle::Event::Closed) {
            s.entries.remove(&id);
            h.destroy();
            s.dirty = true;
            return;
        }
        let w = &mut s.entries.get_mut(&id).unwrap().pending;
        match e {
            handle::Event::Title { title } => w.title = title,
            handle::Event::AppId { app_id } => w.app_id = app_id,
            handle::Event::State { state } => {
                w.active = false;
                w.minimized = false;
                w.maximized = false;
                for chunk in state.chunks_exact(4) {
                    let n = u32::from_ne_bytes(chunk.try_into().unwrap());
                    match n {
                        0 => w.maximized = true,
                        1 => w.minimized = true,
                        2 => w.active = true,
                        _ => {}
                    }
                }
            }
            handle::Event::Done => s.dirty = true,
            _ => {}
        }
    }
}
delegate_noop!(State:ignore wl_seat::WlSeat);
delegate_noop!(State:ignore wl_output::WlOutput);
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
fn run(
    rx: mpsc::Receiver<WindowCommand>,
    mut wake: UnixStream,
    shared: Arc<Mutex<Snapshot>>,
) -> Result<()> {
    let conn = Connection::connect_to_env()
        .context("Нет соединения Wayland. Запускайте внутри сессии Labwc")?;
    let mut queue = conn.new_event_queue::<State>();
    conn.display().get_registry(&queue.handle(), ());
    let mut state = State {
        epoch: epoch(),
        ..Default::default()
    };
    queue.roundtrip(&mut state)?;
    queue.roundtrip(&mut state)?;
    state.publish(&shared);
    loop {
        queue.dispatch_pending(&mut state)?;
        while let Ok(c) = rx.try_recv() {
            state.command(c)
        }
        conn.flush()?;
        state.publish(&shared);
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [
            libc::pollfd {
                fd: conn.as_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // SAFETY: both descriptors and the two-element array live for this call; no pointer escapes.
        let result = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, -1) };
        if result < 0 {
            let e = std::io::Error::last_os_error();
            if e.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(e.into());
        }
        if fds[0].revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
            anyhow::bail!("Wayland connection closed")
        }
        if fds[0].revents & libc::POLLIN != 0 {
            guard.read()?;
        } else {
            drop(guard)
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut buf = [0; 128];
            while wake.read(&mut buf).is_ok_and(|n| n > 0) {}
        }
        if fds[1].revents & libc::POLLHUP != 0 {
            return Ok(());
        }
    }
}
pub fn probe() -> Result<Capabilities> {
    let conn = Connection::connect_to_env()?;
    let mut q = conn.new_event_queue::<State>();
    conn.display().get_registry(&q.handle(), ());
    let mut s = State {
        epoch: epoch(),
        ..Default::default()
    };
    q.roundtrip(&mut s)?;
    Ok(Capabilities::from_globals(s.caps.globals))
}
