use gtk::{gio, glib, prelude::*};
use std::{
    collections::HashSet,
    fmt, fs, io,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Copy,
    Move,
}

#[derive(Clone)]
pub struct Batch {
    pub sources: Vec<PathBuf>,
    pub kind: Kind,
}

impl Batch {
    pub fn after_paste(mut self, completed: &HashSet<PathBuf>) -> Option<Self> {
        if self.kind == Kind::Move {
            self.sources.retain(|path| !completed.contains(path));
        }
        (!self.sources.is_empty()).then_some(self)
    }
}

#[derive(Debug)]
pub enum TransferError {
    Conflict,
    Io(io::Error),
    Gio(glib::Error),
}

impl From<io::Error> for TransferError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for TransferError {
    fn fmt(&self, text: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Conflict => text.write_str("Имя уже используется"),
            Self::Io(error) => error.fmt(text),
            Self::Gio(error) => error.fmt(text),
        }
    }
}

pub fn transfer(
    source: &Path,
    directory: &Path,
    kind: Kind,
    keep_both: bool,
) -> Result<PathBuf, TransferError> {
    let name = source
        .file_name()
        .ok_or_else(|| io::Error::other("Нужен файл или папка"))?;
    let directory = directory.canonicalize()?;
    let same_folder = source.parent().is_some_and(|parent| {
        parent
            .canonicalize()
            .is_ok_and(|parent| parent == directory)
    });
    let metadata = fs::symlink_metadata(source)?;
    if metadata.is_dir() && directory.starts_with(source.canonicalize()?) {
        return Err(io::Error::other("Папку нельзя поместить внутрь неё самой").into());
    }
    if same_folder && kind == Kind::Move {
        return Ok(directory.join(name));
    }
    let keep_both = keep_both || same_folder;
    let target = available_name(&directory, Path::new(name), metadata.is_dir(), keep_both)?;
    let prepared = if kind == Kind::Copy {
        let staging = Staging::new(&directory)?;
        let staged_item = staging.path.join("item");
        copy_tree(source, &staged_item, staging.identity)?;
        Some((staging, staged_item))
    } else {
        None
    };
    let item = prepared.as_ref().map_or(source, |(_, staged)| staged);
    match gio::File::for_path(item).move_(
        &gio::File::for_path(&target),
        gio::FileCopyFlags::NOFOLLOW_SYMLINKS | gio::FileCopyFlags::NO_FALLBACK_FOR_MOVE,
        gio::Cancellable::NONE,
        None,
    ) {
        Ok(()) => Ok(target),
        Err(error)
            if error.matches(gio::IOErrorEnum::Exists)
                || error.matches(gio::IOErrorEnum::WouldMerge)
                || error.matches(gio::IOErrorEnum::IsDirectory) =>
        {
            Err(TransferError::Conflict)
        }
        Err(error) => Err(TransferError::Gio(error)),
    }
}

fn available_name(
    directory: &Path,
    name: &Path,
    is_directory: bool,
    keep_both: bool,
) -> Result<PathBuf, TransferError> {
    let extension = (!is_directory).then(|| name.extension()).flatten();
    let stem = extension
        .and_then(|_| name.file_stem())
        .unwrap_or(name.as_os_str());
    for index in 0..10_000 {
        let mut candidate = name.as_os_str().to_os_string();
        if index > 0 {
            candidate = stem.to_os_string();
            candidate.push(" — копия");
            if index > 1 {
                candidate.push(format!(" ({index})"));
            }
            if let Some(extension) = extension {
                candidate.push(".");
                candidate.push(extension);
            }
        }
        let destination = directory.join(candidate);
        match fs::symlink_metadata(&destination) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(destination),
            Err(error) => return Err(error.into()),
            Ok(_) if !keep_both => return Err(TransferError::Conflict),
            Ok(_) => {}
        }
    }
    Err(TransferError::Conflict)
}

fn copy_tree(source: &Path, target: &Path, stage: (u64, u64)) -> Result<(), TransferError> {
    let mut directories = Vec::new();
    copy_entry(source, target, stage, &mut directories)?;
    while let Some(directory) = directories.last_mut() {
        if let Some(entry) = directory.children.next() {
            let entry = entry?;
            let target = directory.target.join(entry.file_name());
            copy_entry(&entry.path(), &target, stage, &mut directories)?;
        } else {
            fs::set_permissions(
                &directory.target,
                fs::Permissions::from_mode(directory.mode & 0o777),
            )?;
            directories.pop();
        }
    }
    Ok(())
}

struct CopyDirectory {
    children: fs::ReadDir,
    target: PathBuf,
    mode: u32,
}

fn copy_entry(
    source: &Path,
    target: &Path,
    stage: (u64, u64),
    directories: &mut Vec<CopyDirectory>,
) -> Result<(), TransferError> {
    let entry = fs::symlink_metadata(source)?;
    if entry.is_dir() {
        if (entry.dev(), entry.ino()) == stage {
            return Err(io::Error::other("Папку нельзя поместить внутрь неё самой").into());
        }
        fs::create_dir(target)?;
        directories.push(CopyDirectory {
            children: fs::read_dir(source)?,
            target: target.to_owned(),
            mode: entry.mode(),
        });
    } else if entry.is_file() || entry.is_symlink() {
        gio::File::for_path(source)
            .copy(
                &gio::File::for_path(target),
                gio::FileCopyFlags::NOFOLLOW_SYMLINKS,
                gio::Cancellable::NONE,
                None,
            )
            .map_err(TransferError::Gio)?;
    } else {
        return Err(io::Error::other("Этот тип объекта нельзя скопировать").into());
    }
    Ok(())
}

struct Staging {
    path: PathBuf,
    identity: (u64, u64),
}

impl Staging {
    fn new(parent: &Path) -> io::Result<Self> {
        let path = glib::mkdtemp(parent.join(".mosaic-transfer-XXXXXX"))
            .ok_or_else(io::Error::last_os_error)?;
        let metadata = path.symlink_metadata()?;
        Ok(Self {
            path,
            identity: (metadata.dev(), metadata.ino()),
        })
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        if !self.path.symlink_metadata().is_ok_and(|metadata| {
            metadata.is_dir() && (metadata.dev(), metadata.ino()) == self.identity
        }) {
            return;
        }
        if fs::remove_dir_all(&self.path).is_ok() {
            return;
        }
        // Staged directories may already have inherited read-only permissions.
        let mut pending = vec![self.path.clone()];
        while let Some(path) = pending.pop() {
            if path
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.is_dir())
            {
                let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o700));
                if let Ok(entries) = fs::read_dir(path) {
                    pending.extend(entries.filter_map(Result::ok).map(|entry| entry.path()));
                }
            }
        }
        let _ = fs::remove_dir_all(&self.path);
    }
}
