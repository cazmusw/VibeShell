//! Disposable stage-0 probe: all wl_seat/handle objects belong to ONE diagnostic
//! connection. GTK objects never cross connection boundaries. Titles discarded.
#[path = "../src/wayland.rs"]
mod wayland;
use std::{
    collections::HashMap,
    env,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::Duration,
};

fn word(bytes: &[u8]) -> u32 {
    u32::from_ne_bytes(bytes[..4].try_into().unwrap())
}
fn string(s: &str) -> Vec<u8> {
    let mut body = ((s.len() + 1) as u32).to_ne_bytes().to_vec();
    body.extend_from_slice(s.as_bytes());
    body.push(0);
    while !body.len().is_multiple_of(4) {
        body.push(0);
    }
    body
}
fn send(stream: &mut UnixStream, object: u32, opcode: u32, body: &[u8]) -> io::Result<()> {
    let mut wire = object.to_ne_bytes().to_vec();
    wire.extend_from_slice(&((((8 + body.len()) as u32) << 16) | opcode).to_ne_bytes());
    wire.extend_from_slice(body);
    stream.write_all(&wire)
}

#[derive(Default)]
struct Window {
    ours: bool,
    minimized: bool,
    activated: bool,
}

pub struct Probe {
    stream: UnixStream,
    buffer: Vec<u8>,
    windows: HashMap<u32, Window>,
    stage: u8,
}

impl Probe {
    pub fn new() -> io::Result<Self> {
        let display = PathBuf::from(
            env::var_os("WAYLAND_DISPLAY").ok_or_else(|| io::Error::other("missing display"))?,
        );
        let path = if display.is_absolute() {
            display
        } else {
            PathBuf::from(
                env::var_os("XDG_RUNTIME_DIR")
                    .ok_or_else(|| io::Error::other("missing runtime"))?,
            )
            .join(display)
        };
        let mut stream = UnixStream::connect(path)?;
        let globals = wayland::query_stream(stream.try_clone()?, Duration::from_secs(2))?;
        for (interface, id, version) in [
            ("wl_seat", 4_u32, 1_u32),
            ("zwlr_foreign_toplevel_manager_v1", 5, 3),
        ] {
            let g = globals
                .iter()
                .find(|g| g.interface == interface)
                .ok_or_else(|| io::Error::other("required probe global missing"))?;
            let mut body = g.name.to_ne_bytes().to_vec();
            body.extend(string(interface));
            body.extend_from_slice(&g.version.min(version).to_ne_bytes());
            body.extend_from_slice(&id.to_ne_bytes());
            send(&mut stream, 2, 0, &body)?;
        }
        stream.set_nonblocking(true)?;
        Ok(Self {
            stream,
            buffer: Vec::new(),
            windows: HashMap::new(),
            stage: 0,
        })
    }

    pub fn tick(&mut self) -> io::Result<bool> {
        for _ in 0..64 {
            let mut bytes = [0; 8192];
            match self.stream.read(&mut bytes) {
                Ok(0) => return Err(io::Error::other("compositor disconnected")),
                Ok(n) => self.buffer.extend_from_slice(&bytes[..n]),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
            if self.buffer.len() > 4 * 1024 * 1024 {
                return Err(io::Error::other("probe input limit"));
            }
        }
        while self.buffer.len() >= 8 {
            let header = word(&self.buffer[4..]);
            let len = (header >> 16) as usize;
            if len < 8 || !len.is_multiple_of(4) {
                return Err(io::Error::other("invalid probe event"));
            }
            if self.buffer.len() < len {
                break;
            }
            let object = word(&self.buffer);
            let opcode = header & 0xffff;
            let body = self.buffer[8..len].to_vec();
            self.buffer.drain(..len);
            if object == 1 && opcode == 0 {
                return Err(io::Error::other("Wayland protocol error"));
            }
            if object == 5 && opcode == 0 && body.len() == 4 {
                if self.windows.len() >= 4096 {
                    return Err(io::Error::other("probe window limit"));
                }
                self.windows.insert(word(&body), Window::default());
            }
            if let Some(window) = self.windows.get_mut(&object) {
                match opcode {
                    1 if body.len() >= 4 => {
                        let len = word(&body) as usize;
                        window.ours =
                            body.get(4..4 + len) == Some(b"mosaic-stage-0-probe\0".as_slice());
                    }
                    4 if body.len() >= 4 => {
                        let len = word(&body) as usize;
                        let states = body
                            .get(4..4 + len)
                            .ok_or_else(|| io::Error::other("invalid state array"))?;
                        window.minimized = states.chunks_exact(4).any(|v| word(v) == 1);
                        window.activated = states.chunks_exact(4).any(|v| word(v) == 2);
                    }
                    5 if window.ours => match self.stage {
                        0 => {
                            send(&mut self.stream, object, 2, &[])?;
                            self.stage = 1;
                            println!("minimize requested (owned probe window)");
                        }
                        1 if window.minimized => {
                            println!("minimized confirmed by compositor");
                            send(&mut self.stream, object, 3, &[])?;
                            send(&mut self.stream, object, 4, &4_u32.to_ne_bytes())?;
                            self.stage = 2;
                        }
                        2 if window.activated && !window.minimized => {
                            self.stage = 3;
                            println!(
                                "restored and activated confirmed; seat and handle share diagnostic connection"
                            );
                        }
                        _ => {}
                    },
                    6 => {
                        self.windows.remove(&object);
                    }
                    _ => {} // No titles or other application payloads are retained.
                }
            }
        }
        Ok(self.stage == 3)
    }
}
