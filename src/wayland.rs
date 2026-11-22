use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug, PartialEq, Eq)]
pub struct Global {
    pub name: u32,
    pub interface: String,
    pub version: u32,
}

fn invalid(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}

pub fn read_event(reader: &mut impl Read) -> io::Result<(u32, u16, Vec<u8>)> {
    let mut header = [0; 8];
    reader.read_exact(&mut header)?;
    let object = u32::from_ne_bytes(header[..4].try_into().unwrap());
    let word = u32::from_ne_bytes(header[4..].try_into().unwrap());
    let size = (word >> 16) as usize;
    if object == 0 || size < 8 || !size.is_multiple_of(4) {
        return Err(invalid("invalid Wayland header"));
    }
    let mut body = vec![0; size - 8];
    reader.read_exact(&mut body)?;
    Ok((object, word as u16, body))
}

pub fn parse_global(body: &[u8]) -> io::Result<Global> {
    if body.len() < 12 {
        return Err(invalid("short global"));
    }
    let name = u32::from_ne_bytes(body[..4].try_into().unwrap());
    let len = u32::from_ne_bytes(body[4..8].try_into().unwrap()) as usize;
    if !(2..=256).contains(&len) {
        return Err(invalid("invalid interface length"));
    }
    let end = 8 + len;
    let version_offset = end.next_multiple_of(4);
    if body.len() != version_offset + 4 || body[end - 1] != 0 || body[8..end - 1].contains(&0) {
        return Err(invalid("invalid global string or size"));
    }
    let interface = std::str::from_utf8(&body[8..end - 1])
        .map_err(|_| invalid("invalid UTF-8"))?
        .to_owned();
    let version = u32::from_ne_bytes(body[version_offset..].try_into().unwrap());
    if name == 0 || version == 0 {
        return Err(invalid("zero global name or version"));
    }
    Ok(Global {
        name,
        interface,
        version,
    })
}

struct DeadlineStream {
    stream: UnixStream,
    deadline: Instant,
}

impl Read for DeadlineStream {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let remaining = self
            .deadline
            .checked_duration_since(Instant::now())
            .filter(|d| !d.is_zero())
            .ok_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Wayland deadline exceeded"))?;
        self.stream.set_read_timeout(Some(remaining))?;
        self.stream.read(buffer)
    }
}

/// A separate, read-only connection. Never exports object handles to GTK.
pub fn registry(path: &Path, timeout: Duration) -> io::Result<Vec<Global>> {
    let stream = UnixStream::connect(path)?;
    query_stream(stream, timeout)
}

pub fn query_stream(mut stream: UnixStream, timeout: Duration) -> io::Result<Vec<Global>> {
    let deadline = Instant::now() + timeout;
    stream.set_write_timeout(Some(timeout))?;
    // wl_display.get_registry(new_id=2), wl_display.sync(new_id=3).
    for word in [1_u32, (12 << 16) | 1, 2, 1, 12 << 16, 3] {
        stream.write_all(&word.to_ne_bytes())?;
    }
    let mut reader = DeadlineStream { stream, deadline };
    let mut globals = Vec::new();
    let mut bytes = 0;
    for _ in 0..8192 {
        let (object, opcode, body) = read_event(&mut reader)?;
        bytes += body.len() + 8;
        if bytes > 4 * 1024 * 1024 {
            return Err(invalid("registry byte limit"));
        }
        match (object, opcode) {
            (3, 0) if body.len() == 4 => return Ok(globals),
            (2, 0) => globals.push(parse_global(&body)?),
            (2, 1) if body.len() == 4 => {
                let removed = u32::from_ne_bytes(body[..].try_into().unwrap());
                globals.retain(|g| g.name != removed);
            }
            (1, 0) => return Err(invalid("compositor returned a protocol error")),
            (1, 1) if body.len() == 4 => {}
            _ => return Err(invalid("unexpected Wayland registry event")),
        }
    }
    Err(invalid("registry event limit"))
}
