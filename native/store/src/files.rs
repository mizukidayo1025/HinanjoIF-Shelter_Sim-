//! A file only appended to, and reading one at an offset.
//!
//! A stored Run's two files (data.pack, journal.jsonl) are appended to by one writer, which holds
//! the branch's lock, and read by any number of readers, in this process or another, which take no
//! lock. Each append is written whole and flushed to the disk (`sync`) before the next item is
//! written, so a reader that sees a journal line also finds the bytes it points at. The files are
//! opened with the standard library's sharing (on Windows: reading, writing and deleting shared),
//! so a reader can open them while a writer has them open.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::Path;

/// A file appended to: where its end is, kept as it is written.
#[derive(Debug)]
pub struct AppendFile {
    file: File,
    len: u64,
}

impl AppendFile {
    /// The file at `path`, made where it is not there, emptied first when `truncate`.
    pub fn open(path: &Path, truncate: bool) -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).create(true).truncate(false).open(path)?;
        if truncate {
            file.set_len(0)?;
        }
        let len = file.metadata()?.len();
        Ok(Self { file, len })
    }

    /// The bytes in the file.
    pub fn len(&self) -> u64 {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Writes `bytes` at the end: where they start.
    pub fn append(&mut self, bytes: &[u8]) -> io::Result<u64> {
        let offset = self.len;
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(bytes)?;
        self.len = offset + bytes.len() as u64;
        Ok(offset)
    }

    /// Flushes what was written to the disk.
    pub fn sync(&self) -> io::Result<()> {
        self.file.sync_data()
    }

    /// Cuts the file to `size` bytes and flushes it (nothing where it is not longer).
    pub fn truncate(&mut self, size: u64) -> io::Result<()> {
        if self.len > size {
            self.file.set_len(size)?;
            self.file.sync_all()?;
            self.len = size;
        }
        Ok(())
    }

    /// `length` bytes from `offset`, fewer where the file ends before.
    pub fn read_at(&self, offset: u64, length: u64) -> io::Result<Vec<u8>> {
        read_at(&self.file, offset, length)
    }

    /// The whole file.
    pub fn read_all(&self) -> io::Result<Vec<u8>> {
        read_at(&self.file, 0, self.len)
    }
}

/// `length` bytes of `file` from `offset`, fewer where the file ends before.
pub fn read_at(mut file: &File, offset: u64, length: u64) -> io::Result<Vec<u8>> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = Vec::with_capacity(usize::try_from(length).unwrap_or(usize::MAX).min(1 << 26));
    file.take(length).read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// The bytes of the file at `path`, or None where there is none.
pub fn read_if_there(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

/// Flushes a folder's entries (a file made or renamed in it) to the disk; nothing on Windows,
/// which has no handle of a folder to flush (Python's `ProjectStore._write` does the same).
pub fn sync_dir(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}
