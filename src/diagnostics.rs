use std::{
    env, fs,
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const LIMIT: u64 = 128 * 1024;
const CURRENT: &str = "events.log";
const PREVIOUS: &str = "events.log.1";

pub struct Journal {
    directory: PathBuf,
    limit: u64,
}

impl Journal {
    pub fn open(directory: PathBuf, limit: u64) -> Self {
        Self { directory, limit }
    }

    pub fn local() -> Option<Self> {
        let state = env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                let home = PathBuf::from(env::var_os("HOME")?);
                home.is_absolute().then(|| home.join(".local/state"))
            })?;
        Some(Self::open(state.join("mosaic"), LIMIT))
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    pub fn record(&self, component: &str, event: &str, code: i32) -> io::Result<()> {
        if ![component, event].into_iter().all(|token| {
            !token.is_empty()
                && token.len() <= 48
                && token
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
        }) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid journal token",
            ));
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default();
        let line = format!(
            "{}\t{}\t{}\t{component}\t{event}\t{code}\n",
            now.as_millis(),
            env!("CARGO_PKG_VERSION"),
            std::process::id()
        );
        let size = line.len() as u64;
        if size > self.limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "journal line exceeds limit",
            ));
        }
        let _lock = self.lock(true)?;
        let path = self.directory.join(CURRENT);
        let mut file = private_file(&path, true)?;
        if file.metadata()?.len().saturating_add(size) > self.limit {
            drop(file);
            fs::rename(&path, self.directory.join(PREVIOUS))?;
            file = private_file(&path, true)?;
        }
        file.write_all(line.as_bytes())
    }

    pub fn recent(&self, count: usize) -> io::Result<Vec<String>> {
        match self.directory.symlink_metadata() {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        let _lock = self.lock(false)?;
        let mut lines = Vec::new();
        for name in [PREVIOUS, CURRENT] {
            let file = match private_file(&self.directory.join(name), false) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error),
            };
            for line in BufReader::new(file.take(self.limit)).lines() {
                lines.push(line?);
            }
        }
        let start = lines.len().saturating_sub(count);
        Ok(lines.split_off(start))
    }

    fn lock(&self, create: bool) -> io::Result<fs::File> {
        if create {
            if let Some(parent) = self.directory.parent() {
                fs::create_dir_all(parent)?;
            }
            match fs::DirBuilder::new().mode(0o700).create(&self.directory) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error),
            }
        }
        let metadata = self.directory.symlink_metadata()?;
        if !metadata.is_dir() || metadata.permissions().mode() & 0o077 != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "journal directory is not private",
            ));
        }
        let lock = private_file(&self.directory.join("events.lock"), true)?;
        lock.try_lock().map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "journal lock is busy or unavailable",
            )
        })?;
        Ok(lock)
    }
}

fn private_file(path: &Path, write: bool) -> io::Result<fs::File> {
    match path.symlink_metadata() {
        Ok(meta)
            if !meta.is_file() || meta.nlink() != 1 || meta.permissions().mode() & 0o077 != 0 =>
        {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "journal file is not private",
            ));
        }
        Ok(_) => {}
        Err(error) if write && error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    fs::OpenOptions::new()
        .read(true)
        .append(write)
        .create(write)
        .mode(0o600)
        .open(path)
}

pub fn event(component: &str, event: &str, code: i32) {
    if let Some(journal) = Journal::local() {
        let _ = journal.record(component, event, code);
    }
}

pub fn install_panic_hook(component: &'static str) {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let line = info
            .location()
            .map_or(-1, |location| location.line() as i32);
        event(component, "panic", line);
        default(info);
    }));
}
