use gtk::{gio, glib};
use mosaic_shell_tools::diagnostics;
use rustix::fs::{AtFlags, CWD, Dir, FileType, Mode, OFlags, Stat, fstat, open, openat, statat};
use serde::{Deserialize, Serialize};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    ffi::OsString,
    fs::{DirBuilder, File},
    io::{self, Read, Write},
    os::unix::{ffi::OsStringExt, fs::DirBuilderExt},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub const REFRESH_INTERVAL: Duration = Duration::from_secs(45);
const MAX_CACHE_AGE: u64 = 7 * 24 * 60 * 60;
const MAX_RECORDS: usize = 512;
const MAX_PATH_BYTES: usize = 4096;
const MAX_CACHE_BYTES: usize = 12 * 1024 * 1024;
const MAX_DEPTH: usize = 192;
const MAX_OBJECTS: usize = 100_000;
const MAX_SCAN_TIME: Duration = Duration::from_secs(2);
const CACHE_VERSION: u8 = 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value {
    Cached(u64),
    Measured(u64),
    Unavailable,
    Limit,
}

type Publish = Box<dyn Fn(&Path, Value)>;

struct Job {
    folders: Vec<PathBuf>,
    publish: Publish,
}

pub struct Queue {
    cache_file: PathBuf,
    running: Cell<bool>,
    cancel: RefCell<Option<Arc<AtomicBool>>>,
    next: RefCell<Option<Job>>,
    invalidations: RefCell<HashSet<PathBuf>>,
}

impl Queue {
    pub fn new() -> Rc<Self> {
        Rc::new(Self {
            cache_file: glib::user_cache_dir().join("mosaic-files/folder-sizes.json"),
            running: Cell::new(false),
            cancel: RefCell::new(None),
            next: RefCell::new(None),
            invalidations: RefCell::default(),
        })
    }

    pub fn busy(&self) -> bool {
        self.running.get()
    }

    pub fn cancel(&self) {
        self.next.take();
        if let Some(cancel) = self.cancel.borrow().as_ref() {
            cancel.store(true, Ordering::Relaxed);
        }
    }

    pub fn submit(
        self: &Rc<Self>,
        folders: Vec<PathBuf>,
        publish: impl Fn(&Path, Value) + 'static,
    ) {
        self.cancel();
        if !folders.is_empty() {
            self.next.replace(Some(Job {
                folders,
                publish: Box::new(publish),
            }));
        }
        self.start();
    }

    pub fn invalidate(self: &Rc<Self>, paths: impl IntoIterator<Item = PathBuf>) {
        self.invalidations.borrow_mut().extend(paths);
        self.cancel();
        self.start();
    }

    fn start(self: &Rc<Self>) {
        if self.running.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            loop {
                let Some(queue) = weak.upgrade() else { return };
                let invalidations = queue.invalidations.take();
                let job = queue.next.take();
                if job.is_none() && invalidations.is_empty() {
                    queue.cancel.take();
                    queue.running.set(false);
                    return;
                }
                let cancel = Arc::new(AtomicBool::new(false));
                queue.cancel.replace(Some(cancel.clone()));
                let path = queue.cache_file.clone();
                drop(queue);

                let (folders, publish) = job.map_or_else(
                    || (Vec::new(), None),
                    |job| (job.folders, Some(job.publish)),
                );
                let (send, recv) = async_channel::bounded(16);
                let token = cancel.clone();
                let worker = gio::spawn_blocking(move || {
                    work(&path, folders, invalidations, &token, |path, value| {
                        let _ = send.send_blocking((path, value));
                    });
                });
                while let Ok((path, value)) = recv.recv().await {
                    if !cancel.load(Ordering::Relaxed)
                        && let Some(publish) = publish.as_ref()
                        && weak.strong_count() > 0
                    {
                        publish(&path, value);
                    }
                }
                if worker.await.is_err() {
                    diagnostics::event("files", "folder_size_worker_failed", 0);
                }
            }
        });
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Stamp {
    dev: u64,
    ino: u64,
    modified: (i64, u64),
    changed: (i64, u64),
}

impl From<Stat> for Stamp {
    fn from(stat: Stat) -> Self {
        Self {
            dev: stat.st_dev,
            ino: stat.st_ino,
            modified: (stat.st_mtime, stat.st_mtime_nsec),
            changed: (stat.st_ctime, stat.st_ctime_nsec),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ScanError {
    Cancelled,
    Unavailable,
    Limit,
}

impl From<rustix::io::Errno> for ScanError {
    fn from(_: rustix::io::Errno) -> Self {
        Self::Unavailable
    }
}

const DIRECTORY_FLAGS: OFlags = OFlags::RDONLY
    .union(OFlags::DIRECTORY)
    .union(OFlags::NOFOLLOW)
    .union(OFlags::CLOEXEC)
    .union(OFlags::NONBLOCK);

fn directory_stamp(path: &Path) -> Result<Stamp, ScanError> {
    let stat = statat(CWD, path, AtFlags::SYMLINK_NOFOLLOW)?;
    if FileType::from_raw_mode(stat.st_mode) != FileType::Directory {
        return Err(ScanError::Unavailable);
    }
    Ok(stat.into())
}

fn scan(path: &Path, cancel: &AtomicBool) -> Result<(u64, Stamp), ScanError> {
    if cancel.load(Ordering::Relaxed) {
        return Err(ScanError::Cancelled);
    }
    let started = Instant::now();
    let root = open(path, DIRECTORY_FLAGS, Mode::empty())?;
    let stamp = Stamp::from(fstat(&root)?);
    let mut stack = vec![Dir::new(root)?];
    let mut bytes = 0u64;
    let mut visited = 0usize;

    while let Some(depth) = stack.len().checked_sub(1) {
        let parent = &mut stack[depth];
        if cancel.load(Ordering::Relaxed) {
            return Err(ScanError::Cancelled);
        }
        let Some(child) = parent.read() else {
            stack.pop();
            continue;
        };
        let name = child?.file_name().to_owned();
        if matches!(name.to_bytes(), b"." | b"..") {
            continue;
        }
        visited += 1;
        if visited > MAX_OBJECTS
            || (visited.is_multiple_of(256) && started.elapsed() > MAX_SCAN_TIME)
        {
            return Err(ScanError::Limit);
        }
        let parent = parent.fd()?;
        let info = statat(parent, &name, AtFlags::SYMLINK_NOFOLLOW)?;
        match FileType::from_raw_mode(info.st_mode) {
            FileType::RegularFile => {
                let size = u64::try_from(info.st_size).map_err(|_| ScanError::Unavailable)?;
                bytes = bytes.checked_add(size).ok_or(ScanError::Limit)?;
            }
            FileType::Directory => {
                if depth + 1 >= MAX_DEPTH {
                    return Err(ScanError::Limit);
                }
                // Resolve every child relative to the open parent. NOFOLLOW also
                // protects the interval between reading metadata and opening a child.
                let child = Dir::new(openat(parent, &name, DIRECTORY_FLAGS, Mode::empty())?)?;
                stack.push(child);
            }
            _ => {}
        }
    }
    if directory_stamp(path)? != stamp {
        return Err(ScanError::Unavailable);
    }
    Ok((bytes, stamp))
}

#[derive(Clone, Serialize, Deserialize)]
struct Record {
    path: Vec<u8>,
    stamp: Stamp,
    bytes: u64,
    measured_at: u64,
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    version: u8,
    entries: Vec<Record>,
}

type Records = HashMap<PathBuf, Record>;

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn affected(path: &Path, invalidations: &HashSet<PathBuf>) -> bool {
    invalidations
        .iter()
        .any(|changed| path.starts_with(changed) || changed.starts_with(path))
}

fn work(
    path: &Path,
    folders: Vec<PathBuf>,
    invalidations: HashSet<PathBuf>,
    cancel: &AtomicBool,
    mut publish: impl FnMut(PathBuf, Value),
) {
    let records = read_cache(path);
    let now = timestamp();
    for folder in &folders {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if let Some(record) = records.get(folder)
            && !affected(folder, &invalidations)
            && now.saturating_sub(record.measured_at) <= MAX_CACHE_AGE
            && directory_stamp(folder) == Ok(record.stamp)
        {
            publish(folder.clone(), Value::Cached(record.bytes));
        }
    }

    let mut updates = HashMap::new();
    for folder in folders {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let (value, record) = match scan(&folder, cancel) {
            Ok((bytes, stamp)) => (
                Value::Measured(bytes),
                Some(Record {
                    path: folder.as_os_str().as_encoded_bytes().to_vec(),
                    stamp,
                    bytes,
                    measured_at: timestamp(),
                }),
            ),
            Err(ScanError::Cancelled) => break,
            Err(ScanError::Unavailable) => (Value::Unavailable, None),
            Err(ScanError::Limit) => (Value::Limit, None),
        };
        publish(folder.clone(), value);
        if folder.as_os_str().as_encoded_bytes().len() <= MAX_PATH_BYTES {
            updates.insert(folder, record);
        }
    }
    if (!updates.is_empty() || !invalidations.is_empty())
        && let Err(error) = write_cache(path, &invalidations, updates)
    {
        diagnostics::event(
            "files",
            "folder_sizes_cache_failed",
            error.raw_os_error().unwrap_or(0),
        );
    }
}

fn read_cache(path: &Path) -> Records {
    let Ok(fd) = open(
        path,
        OFlags::RDONLY | OFlags::NOFOLLOW | OFlags::NONBLOCK | OFlags::CLOEXEC,
        Mode::empty(),
    ) else {
        return Records::new();
    };
    let file = File::from(fd);
    if !file
        .metadata()
        .is_ok_and(|data| data.is_file() && data.len() <= MAX_CACHE_BYTES as u64)
    {
        return Records::new();
    }
    let mut bytes = Vec::new();
    if file
        .take(MAX_CACHE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .is_err()
        || bytes.len() > MAX_CACHE_BYTES
    {
        return Records::new();
    }
    let Ok(cache) = serde_json::from_slice::<CacheFile>(&bytes) else {
        return Records::new();
    };
    if cache.version != CACHE_VERSION || cache.entries.len() > MAX_RECORDS {
        return Records::new();
    }
    cache
        .entries
        .into_iter()
        .filter_map(|record| {
            if record.path.len() > MAX_PATH_BYTES || record.path.contains(&0) {
                return None;
            }
            let path = PathBuf::from(OsString::from_vec(record.path.clone()));
            path.is_absolute().then_some((path, record))
        })
        .collect()
}

fn write_cache(
    path: &Path,
    invalidations: &HashSet<PathBuf>,
    updates: HashMap<PathBuf, Option<Record>>,
) -> io::Result<()> {
    let parent = path.parent().ok_or(io::ErrorKind::InvalidInput)?;
    DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(parent)?;
    let lock = File::from(open(
        parent.join("folder-sizes.lock"),
        OFlags::CREATE | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK,
        Mode::RUSR | Mode::WUSR,
    )?);
    // Merge with another session's changes without blocking a foreground scan.
    match lock.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
        Err(std::fs::TryLockError::Error(error)) => return Err(error),
    }

    let mut records = read_cache(path);
    records.retain(|path, _| !affected(path, invalidations));
    let now = timestamp();
    for (folder, record) in updates {
        if let Some(record) = record {
            records.insert(folder, record);
        } else {
            records.remove(&folder);
        }
    }
    let mut entries: Vec<_> = records
        .into_values()
        .filter(|record| now.saturating_sub(record.measured_at) <= MAX_CACHE_AGE)
        .collect();
    entries.sort_unstable_by(|a, b| b.measured_at.cmp(&a.measured_at).then(a.path.cmp(&b.path)));
    entries.truncate(MAX_RECORDS);
    let cache = CacheFile {
        version: CACHE_VERSION,
        entries,
    };
    let mut file = tempfile::NamedTempFile::with_prefix_in(".folder-sizes-", parent)?;
    {
        let mut writer = io::BufWriter::new(file.as_file_mut());
        serde_json::to_writer(&mut writer, &cache)?;
        writer.flush()?;
    }
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{self, Permissions};
    use std::os::unix::fs::{PermissionsExt, symlink};

    #[test]
    fn measures_deep_tree_without_following_links() {
        let scratch = tempfile::tempdir().unwrap();
        let root = scratch.path().join("tree");
        let outside = scratch.path().join("outside");
        let deep = (0..150).fold(root.clone(), |path, _| path.join("d"));
        fs::create_dir_all(&deep).unwrap();
        fs::write(&outside, [0; 24]).unwrap();
        fs::write(deep.join("nested"), [0; 3]).unwrap();
        symlink(&outside, deep.join("file-link")).unwrap();
        symlink(&root, deep.join("loop")).unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(scan(&root, &cancel).unwrap().0, 3);
        assert_eq!(
            scan(&deep.join("loop"), &cancel),
            Err(ScanError::Unavailable)
        );
        cancel.store(true, Ordering::Relaxed);
        assert_eq!(scan(&root, &cancel), Err(ScanError::Cancelled));
    }

    #[test]
    fn inaccessible_child_does_not_produce_a_partial_size() {
        let scratch = tempfile::tempdir().unwrap();
        let blocked = scratch.path().join("blocked");
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("file"), [0; 11]).unwrap();
        fs::set_permissions(&blocked, Permissions::from_mode(0o000)).unwrap();
        let result = scan(scratch.path(), &AtomicBool::new(false));
        let accessible = open(&blocked, DIRECTORY_FLAGS, Mode::empty()).is_ok();
        fs::set_permissions(&blocked, Permissions::from_mode(0o700)).unwrap();
        if accessible {
            assert_eq!(result.unwrap().0, 11);
        } else {
            assert_eq!(result, Err(ScanError::Unavailable));
        }
    }

    #[test]
    fn replaces_corrupt_cache_and_revalidates_nested_changes() {
        let scratch = tempfile::tempdir().unwrap();
        let cache = scratch.path().join("cache.json");
        let folder = scratch.path().join("folder");
        let nested = folder.join("nested");
        fs::create_dir_all(&nested).unwrap();
        let file = nested.join("file");
        fs::write(&cache, b"{").unwrap();
        fs::write(&file, [0; 7]).unwrap();
        assert!(read_cache(&cache).is_empty());
        let measure = |invalidations| {
            let mut values = Vec::new();
            work(
                &cache,
                vec![folder.clone()],
                invalidations,
                &AtomicBool::new(false),
                |_, value| values.push(value),
            );
            values
        };
        assert_eq!(measure(HashSet::new()), [Value::Measured(7)]);
        let stamp = directory_stamp(&folder).unwrap();
        fs::write(&file, [0; 9]).unwrap();
        assert_eq!(directory_stamp(&folder).unwrap(), stamp);
        assert_eq!(
            measure(HashSet::new()),
            [Value::Cached(7), Value::Measured(9)]
        );
        assert_eq!(read_cache(&cache)[&folder].bytes, 9);
        assert_eq!(measure(HashSet::from([nested])), [Value::Measured(9)]);
    }
}
