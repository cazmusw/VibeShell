use serde::{Deserialize, Serialize};
use std::{
    env,
    ffi::OsString,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::{DirBuilderExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

const MAX_FILE_BYTES: usize = 64 * 1024;
const MAX_BOOKMARKS: usize = 128;

pub struct Bookmarks {
    config: PathBuf,
    paths: Vec<PathBuf>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct BookmarkFile {
    folders: Vec<SavedPath>,
}

#[derive(Deserialize, Serialize)]
#[serde(untagged)]
enum SavedPath {
    Text(String),
    Bytes(Vec<u8>),
}

impl Bookmarks {
    pub fn load() -> io::Result<Self> {
        let base = env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| {
                env::var_os("HOME")
                    .map(PathBuf::from)
                    .filter(|path| path.is_absolute())
                    .map(|path| path.join(".config"))
            })
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no configuration directory"))?;
        Self::load_from(base.join("mosaic-files/bookmarks.toml"))
    }

    fn load_from(config: PathBuf) -> io::Result<Self> {
        let paths = read_paths(&config)?;
        Ok(Self { config, paths })
    }

    pub fn paths(&self) -> &[PathBuf] {
        &self.paths
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.paths.iter().any(|saved| saved == path)
    }

    pub fn toggle(&mut self, path: &Path) -> io::Result<()> {
        self.update(path, true)
    }

    pub fn remove(&mut self, path: &Path) -> io::Result<()> {
        self.update(path, false)
    }

    fn update(&mut self, path: &Path, add_if_missing: bool) -> io::Result<()> {
        if !valid_path(path) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "bookmarks require absolute local paths",
            ));
        }
        let parent = self.config.parent().expect("bookmarks file has a parent");
        DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(parent)?;
        // Use a separate lock file so replacing the data never replaces the lock.
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(parent.join("bookmarks.lock"))?;
        lock.lock()?;

        let mut paths = read_paths(&self.config)?;
        if let Some(index) = paths.iter().position(|saved| saved == path) {
            paths.remove(index);
        } else if add_if_missing {
            if paths.len() >= MAX_BOOKMARKS {
                return Err(io::Error::other("bookmark limit reached"));
            }
            paths.push(path.to_path_buf());
        } else {
            self.paths = paths;
            return Ok(());
        }
        write_paths(&self.config, parent, &paths)?;
        self.paths = paths;
        Ok(())
    }
}

fn valid_path(path: &Path) -> bool {
    path.is_absolute() && !path.as_os_str().as_bytes().contains(&0)
}

fn read_paths(config: &Path) -> io::Result<Vec<PathBuf>> {
    match fs::symlink_metadata(config) {
        Ok(metadata) if metadata.is_file() => {}
        Ok(_) => return Err(io::Error::other("bookmarks file must be a regular file")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    }

    let mut data = Vec::new();
    File::open(config)?
        .take(MAX_FILE_BYTES as u64 + 1)
        .read_to_end(&mut data)?;
    if data.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("bookmarks file exceeds 64 KiB"));
    }
    let data = std::str::from_utf8(&data).map_err(io::Error::other)?;
    let file: BookmarkFile = toml::from_str(data).map_err(io::Error::other)?;
    if file.folders.len() > MAX_BOOKMARKS {
        return Err(io::Error::other("bookmark limit exceeded"));
    }
    let mut paths = Vec::new();
    for folder in file.folders {
        let path = match folder {
            SavedPath::Text(text) => PathBuf::from(text),
            SavedPath::Bytes(bytes) => PathBuf::from(OsString::from_vec(bytes)),
        };
        if !valid_path(&path) {
            return Err(io::Error::other("bookmarks file contains an invalid path"));
        }
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    Ok(paths)
}

fn write_paths(config: &Path, parent: &Path, paths: &[PathBuf]) -> io::Result<()> {
    let file = BookmarkFile {
        folders: paths
            .iter()
            .map(|path| match path.to_str() {
                Some(text) => SavedPath::Text(text.to_owned()),
                None => SavedPath::Bytes(path.as_os_str().as_bytes().to_vec()),
            })
            .collect(),
    };
    let data = toml::to_string(&file).map_err(io::Error::other)?;
    if data.len() > MAX_FILE_BYTES {
        return Err(io::Error::other("bookmarks file exceeds 64 KiB"));
    }

    let directory = File::open(parent)?;
    let temp = parent.join(".bookmarks.toml.tmp");
    // The caller holds the lock, so a leftover temporary file is from an interrupted write.
    match fs::remove_file(&temp) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    let result = (|| {
        file.write_all(data.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temp, config)
    })();
    if result.is_ok() {
        // After rename the change is committed and the caller must update its in-memory state.
        let _ = directory.sync_all();
    } else {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            static NEXT_ID: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "mosaic-files-bookmarks-test-{}-{}",
                std::process::id(),
                NEXT_ID.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn config(&self) -> PathBuf {
            self.0.join("bookmarks.toml")
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn reload_preserves_order_and_non_utf8_paths() {
        let directory = TestDirectory::new();
        let mut bookmarks = Bookmarks::load_from(directory.config()).unwrap();
        let ordinary_path = Path::new("/home/test/Papers #1");
        let byte_path = PathBuf::from(OsString::from_vec(b"/home/test/R\xffsum\xe9".to_vec()));
        bookmarks.toggle(ordinary_path).unwrap();
        bookmarks.toggle(&byte_path).unwrap();

        let mut reopened = Bookmarks::load_from(directory.config()).unwrap();
        assert_eq!(
            reopened.paths(),
            &[ordinary_path.to_path_buf(), byte_path.clone()]
        );
        reopened.toggle(ordinary_path).unwrap();
        assert!(!reopened.contains(ordinary_path));
        reopened.remove(&byte_path).unwrap();
        assert!(
            Bookmarks::load_from(directory.config())
                .unwrap()
                .paths()
                .is_empty()
        );
    }

    #[test]
    fn saves_keep_changes_from_another_loaded_store() {
        let directory = TestDirectory::new();
        let mut first = Bookmarks::load_from(directory.config()).unwrap();
        let mut second = Bookmarks::load_from(directory.config()).unwrap();
        first.toggle(Path::new("/home/test/Documents")).unwrap();
        second.toggle(Path::new("/home/test/Downloads")).unwrap();
        assert_eq!(
            second.paths(),
            &[
                PathBuf::from("/home/test/Documents"),
                PathBuf::from("/home/test/Downloads"),
            ]
        );
    }

    #[test]
    fn a_failed_save_preserves_loaded_paths_and_existing_file() {
        let directory = TestDirectory::new();
        let mut bookmarks = Bookmarks::load_from(directory.config()).unwrap();
        let path = Path::new("/home/test/Documents");
        bookmarks.toggle(path).unwrap();
        fs::write(directory.config(), "folders = [").unwrap();

        assert!(bookmarks.remove(path).is_err());
        assert!(bookmarks.contains(path));
        assert_eq!(
            fs::read_to_string(directory.config()).unwrap(),
            "folders = ["
        );
    }
}
