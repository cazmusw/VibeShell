use rustix::fs::{CWD, Mode, OFlags, RenameFlags, open, renameat_with};
use std::{
    collections::HashSet,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};
use tempfile::{NamedTempFile, TempDir};
use zip::{
    CompressionMethod, ZipReadOptions, ZipWriter,
    read::{Config, ZipFileEntry},
    result::ZipError,
    unstable::read::ZipIterable,
    write::SimpleFileOptions,
};

const MAX_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
const MAX_TOTAL_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const MAX_OPEN_BYTES: u64 = 64 * 1024 * 1024;
const MAX_MEMBERS: usize = 4096;
const STAGING_PREFIX: &str = ".mosaic-archive-";

#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    Cancelled,
    Conflict,
    Changed,
    Io,
    Invalid,
    Limit,
    UnsafePath,
    Unsupported,
}

impl From<io::Error> for Error {
    fn from(error: io::Error) -> Self {
        match error.kind() {
            io::ErrorKind::AlreadyExists => Self::Conflict,
            _ => Self::Io,
        }
    }
}

impl From<ZipError> for Error {
    fn from(error: ZipError) -> Self {
        match error {
            ZipError::Io(error) => error.into(),
            ZipError::UnsupportedArchive(_) => Self::Unsupported,
            _ => Self::Invalid,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub path: PathBuf,
    pub directory: bool,
    pub size: u64,
}

fn check_cancel(cancel: &AtomicBool) -> Result<(), Error> {
    if cancel.load(Ordering::Relaxed) {
        return Err(Error::Cancelled);
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<File, Error> {
    let file = File::from(
        open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC | OFlags::NOFOLLOW,
            Mode::empty(),
        )
        .map_err(io::Error::from)?,
    );
    if !file.metadata()?.is_file() {
        return Err(Error::UnsafePath);
    }
    Ok(file)
}

fn relative_member(name: &str) -> Result<&Path, Error> {
    if name.is_empty()
        || name.len() > 1024
        || name.contains(['\\', '\0'])
        || name.split('/').count() > 48
        || name.split('/').any(|part| matches!(part, "" | "." | ".."))
        || name.as_bytes().get(1) == Some(&b':')
    {
        return Err(Error::UnsafePath);
    }
    Ok(Path::new(name))
}

fn visit(
    path: &Path,
    cancel: &AtomicBool,
    mut member: impl FnMut(Member, ZipFileEntry<'_>, &mut BufReader<File>) -> Result<(), Error>,
) -> Result<(), Error> {
    check_cancel(cancel)?;
    let file = regular_file(path)?;
    if file.metadata()?.len() > MAX_ARCHIVE_BYTES {
        return Err(Error::Limit);
    }
    let mut zip = ZipIterable::try_new(BufReader::new(file), Config::default())?
        .enforce_header_consistency(true);
    if zip.len() > MAX_MEMBERS {
        return Err(Error::Limit);
    }
    let mut names = HashSet::new();
    let mut total = 0;
    let entries = zip.files()?;
    while let Some(entry) = entries.next() {
        check_cancel(cancel)?;
        let entry = entry?;
        let directory = entry.is_dir();
        let file_type = entry.unix_mode().unwrap_or(0) & 0o170000;
        if !matches!(
            (directory, file_type),
            (_, 0) | (true, 0o040000) | (false, 0o100000)
        ) || entry.is_symlink()
        {
            return Err(Error::UnsafePath);
        }
        if entry.encrypted()
            || !matches!(
                entry.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
        {
            return Err(Error::Unsupported);
        }
        let name = entry.name()?;
        let name = if directory {
            name.strip_suffix('/').unwrap_or(&name)
        } else {
            &name
        };
        let path = relative_member(name)?.to_owned();
        if !names.insert(path.clone()) {
            return Err(Error::Conflict);
        }
        let size = entry.size();
        total = add_bytes(total, size, MAX_TOTAL_BYTES)?;
        member(
            Member {
                path,
                directory,
                size,
            },
            entry,
            entries.reader_mut(),
        )?;
    }
    Ok(())
}

fn add_bytes(current: u64, additional: u64, max: u64) -> Result<u64, Error> {
    current
        .checked_add(additional)
        .filter(|&sum| sum <= max)
        .ok_or(Error::Limit)
}

pub fn list(path: &Path, cancel: &AtomicBool) -> Result<Vec<Member>, Error> {
    let mut members = Vec::new();
    visit(path, cancel, |member, _, _| {
        members.push(member);
        Ok(())
    })?;
    Ok(members)
}

fn copy(
    input: &mut impl Read,
    output: &mut impl Write,
    expected: u64,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    let mut buffer = [0; 64 * 1024];
    let mut copied = 0;
    loop {
        check_cancel(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            return if copied == expected {
                Ok(())
            } else {
                Err(Error::Changed)
            };
        }
        copied = add_bytes(copied, count as u64, expected)?;
        output.write_all(&buffer[..count])?;
    }
}

fn unpack_member<'a>(
    target: &Path,
    member: &Member,
    entry: ZipFileEntry<'a>,
    reader: &'a mut BufReader<File>,
    cancel: &AtomicBool,
) -> Result<(), Error> {
    if member.directory {
        fs::create_dir_all(target)?;
    } else {
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut input = entry.with_reader(reader, ZipReadOptions::new())?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(target)?;
        copy(&mut input, &mut output, member.size, cancel)?;
    }
    Ok(())
}

pub fn unpack(path: &Path, directory: &Path, cancel: &AtomicBool) -> Result<PathBuf, Error> {
    check_cancel(cancel)?;
    let name = path
        .file_stem()
        .filter(|name| *name != "." && *name != "..")
        .ok_or(Error::UnsafePath)?;
    let directory = directory.canonicalize()?;
    let target = directory.join(name);
    if target.symlink_metadata().is_ok() {
        return Err(Error::Conflict);
    }
    let staged = tempfile::Builder::new()
        .prefix(STAGING_PREFIX)
        .tempdir_in(directory)?;
    visit(path, cancel, |member, entry, reader| {
        unpack_member(
            &staged.path().join(&member.path),
            &member,
            entry,
            reader,
            cancel,
        )
    })?;
    check_cancel(cancel)?;
    renameat_with(CWD, staged.path(), CWD, &target, RenameFlags::NOREPLACE)
        .map_err(io::Error::from)?;
    let _ = staged.keep();
    Ok(target)
}

pub fn stage_member(
    path: &Path,
    selected: &Member,
    cancel: &AtomicBool,
) -> Result<(TempDir, PathBuf), Error> {
    if selected.directory || selected.size > MAX_OPEN_BYTES {
        return Err(Error::Limit);
    }
    let temp = tempfile::Builder::new().prefix(STAGING_PREFIX).tempdir()?;
    let target = temp
        .path()
        .join(selected.path.file_name().ok_or(Error::UnsafePath)?);
    let mut copied = false;
    visit(path, cancel, |member, entry, reader| {
        if member.path == selected.path {
            if &member != selected {
                return Err(Error::Changed);
            }
            unpack_member(&target, &member, entry, reader, cancel)?;
            copied = true;
        }
        Ok(())
    })?;
    if !copied {
        return Err(Error::Changed);
    }
    Ok((temp, target))
}

struct Pack<'a> {
    zip: ZipWriter<File>,
    cancel: &'a AtomicBool,
    count: usize,
    size: u64,
}

impl Pack<'_> {
    fn add(&mut self, source: &Path, name: &str) -> Result<(), Error> {
        check_cancel(self.cancel)?;
        relative_member(name)?;
        self.count += 1;
        if self.count > MAX_MEMBERS {
            return Err(Error::Limit);
        }
        let metadata = source.symlink_metadata()?;
        let options = SimpleFileOptions::default();
        if metadata.is_dir() {
            self.zip.add_directory(name, options)?;
            for entry in fs::read_dir(source)? {
                let entry = entry?;
                let basename = entry.file_name();
                let child = basename.to_str().ok_or(Error::Unsupported)?;
                self.add(&entry.path(), &format!("{name}/{child}"))?;
            }
        } else if metadata.is_file() {
            let mut file = regular_file(source)?;
            let size = file.metadata()?.len();
            self.size = add_bytes(self.size, size, MAX_TOTAL_BYTES)?;
            self.zip.start_file(
                name,
                options.compression_method(CompressionMethod::Deflated),
            )?;
            copy(&mut file, &mut self.zip, size, self.cancel)?;
        } else {
            return Err(Error::UnsafePath);
        }
        Ok(())
    }
}

pub fn pack(sources: &[PathBuf], destination: &Path, cancel: &AtomicBool) -> Result<(), Error> {
    check_cancel(cancel)?;
    let parent = destination
        .parent()
        .ok_or(Error::UnsafePath)?
        .canonicalize()?;
    if destination.symlink_metadata().is_ok() {
        return Err(Error::Conflict);
    }
    for source in sources {
        if source.symlink_metadata()?.is_dir() && parent.starts_with(source.canonicalize()?) {
            return Err(Error::UnsafePath);
        }
    }
    let staged = NamedTempFile::with_prefix_in(STAGING_PREFIX, &parent)?;
    let mut pack = Pack {
        zip: ZipWriter::new(staged.reopen()?),
        cancel,
        count: 0,
        size: 0,
    };
    for source in sources {
        let name = source
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or(Error::Unsupported)?;
        pack.add(source, name)?;
    }
    pack.zip.finish()?;
    if staged.as_file().metadata()?.len() > MAX_ARCHIVE_BYTES {
        return Err(Error::Limit);
    }
    check_cancel(cancel)?;
    staged
        .persist_noclobber(destination)
        .map_err(|error| Error::from(error.error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zip_round_trip_and_conflicts_leave_existing_data_intact() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        fs::create_dir_all(source.join("empty")).unwrap();
        fs::write(source.join("note.txt"), "Mosaic ZIP").unwrap();
        let archive = temp.path().join("backup.zip");
        let cancel = AtomicBool::new(false);
        pack(std::slice::from_ref(&source), &archive, &cancel).unwrap();

        assert_eq!(
            pack(std::slice::from_ref(&source), &archive, &cancel),
            Err(Error::Conflict)
        );
        let entries = list(&archive, &cancel).unwrap();
        let entry = entries.iter().find(|item| !item.directory).unwrap();
        let (_copy, opened) = stage_member(&archive, entry, &cancel).unwrap();
        assert_eq!(fs::read_to_string(opened).unwrap(), "Mosaic ZIP");

        let unpacked = unpack(&archive, temp.path(), &cancel).unwrap();
        assert!(unpacked.join("source/empty").is_dir());
        let file = unpacked.join("source/note.txt");
        assert_eq!(fs::read_to_string(&file).unwrap(), "Mosaic ZIP");
        fs::write(&file, "existing content").unwrap();
        assert_eq!(unpack(&archive, temp.path(), &cancel), Err(Error::Conflict));
        assert_eq!(fs::read_to_string(file).unwrap(), "existing content");
    }

    #[test]
    fn unsafe_entries_never_publish_a_partly_unpacked_archive() {
        let temp = tempfile::tempdir().unwrap();
        for symlink in [false, true] {
            let archive = temp.path().join("unsafe.zip");
            let mut writer = ZipWriter::new(File::create(&archive).unwrap());
            let options =
                SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
            writer.start_file("before.txt", options).unwrap();
            writer.write_all(b"already staged").unwrap();
            if symlink {
                writer.add_symlink("link", "../exit.txt", options).unwrap();
            } else {
                writer.start_file("../exit.txt", options).unwrap();
                writer.write_all(b"escape").unwrap();
            }
            writer.finish().unwrap();
            let cancel = AtomicBool::new(false);
            assert_eq!(
                unpack(&archive, temp.path(), &cancel),
                Err(Error::UnsafePath)
            );
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 1);
        }
    }

    #[test]
    fn cancelled_operations_leave_no_output_and_copy_checks_at_chunk_boundaries() {
        struct CancelAfterRead<'a>(&'a AtomicBool);

        impl Read for CancelAfterRead<'_> {
            fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
                output[0] = 42;
                self.0.store(true, Ordering::Relaxed);
                Ok(1)
            }
        }

        let cancel = AtomicBool::new(false);
        let mut input = CancelAfterRead(&cancel);
        assert_eq!(
            copy(&mut input, &mut Vec::new(), 2, &cancel),
            Err(Error::Cancelled)
        );
        let temp = tempfile::tempdir().unwrap();
        let archive = temp.path().join("cancel.zip");
        assert_eq!(pack(&[], &archive, &cancel), Err(Error::Cancelled));
        assert_eq!(
            unpack(&archive, temp.path(), &cancel),
            Err(Error::Cancelled)
        );
        assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
    }
}
