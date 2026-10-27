//! On-disk store for book files (PLAN.md §2, §6), outside the replicated document.
//!
//! Files live at `books/<blake3 hex>.<format>`; a download in progress is
//! `books/<blake3 hex>.part`, and an import in progress `books/import-<uuid>.tmp`. A file
//! only takes its final name after it is complete, fsynced and verified, and the directory is
//! fsynced after every rename, so a crash leaves either the old state or the new one. Names
//! that don't match these patterns are never touched.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};

use crate::model::MAX_BOOK_SIZE;

const CHUNK: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Entry {
    /// A complete file under its final name.
    Complete { hash: String, path: PathBuf, size: u64 },
    /// A partial download.
    Part { hash: String, len: u64 },
    /// A leftover import temp file.
    ImportTemp { path: PathBuf },
}

/// Outcome of finishing a download.
#[derive(Debug, PartialEq, Eq)]
pub enum Finished {
    Verified(PathBuf),
    /// The bytes don't match the hash (or size); the partial file was deleted.
    Corrupt,
}

pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn open(dir: PathBuf) -> Result<Self> {
        fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn final_path(&self, hash: &str, format: &str) -> PathBuf {
        self.dir.join(format!("{hash}.{format}"))
    }

    pub fn part_path(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.part"))
    }

    /// Everything in the store this module recognises.
    pub fn list(&self) -> Result<Vec<Entry>> {
        let mut out = vec![];
        for entry in fs::read_dir(&self.dir).with_context(|| format!("reading {}", self.dir.display()))? {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let path = entry.path();
            if !entry.file_type()?.is_file() {
                continue;
            }
            if name.starts_with("import-") && name.ends_with(".tmp") {
                out.push(Entry::ImportTemp { path });
                continue;
            }
            let Some((hash, ext)) = name.split_once('.') else { continue };
            if !is_hash(hash) || ext.is_empty() || ext.contains('.') {
                continue;
            }
            let len = entry.metadata()?.len();
            if ext == "part" {
                out.push(Entry::Part { hash: hash.to_owned(), len });
            } else {
                out.push(Entry::Complete { hash: hash.to_owned(), path, size: len });
            }
        }
        Ok(out)
    }

    /// The complete file for `hash`, if present.
    pub fn find(&self, hash: &str) -> Result<Option<(PathBuf, u64)>> {
        Ok(self.list()?.into_iter().find_map(|e| match e {
            Entry::Complete { hash: h, path, size } if h == hash => Some((path, size)),
            _ => None,
        }))
    }

    pub fn part_len(&self, hash: &str) -> u64 {
        fs::metadata(self.part_path(hash)).map(|m| m.len()).unwrap_or(0)
    }

    /// Copies a book into the store while hashing it. Returns (hash, size, final path).
    pub fn import(&self, mut source: impl Read, format: &str) -> Result<(String, u64, PathBuf)> {
        let tmp = self.dir.join(format!("import-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut out = File::create(&tmp)?;
            let mut hasher = blake3::Hasher::new();
            let mut size = 0u64;
            let mut buf = vec![0u8; CHUNK];
            loop {
                let n = source.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                size += n as u64;
                if size > MAX_BOOK_SIZE {
                    bail!("Book files are limited to {} GB.", MAX_BOOK_SIZE >> 30);
                }
                hasher.update(&buf[..n]);
                out.write_all(&buf[..n]).map_err(disk_error)?;
            }
            if size == 0 {
                bail!("That file is empty.");
            }
            out.sync_all().map_err(disk_error)?;
            let hash = hasher.finalize().to_hex().to_string();
            let path = match self.find(&hash)? {
                Some((existing, _)) => {
                    fs::remove_file(&tmp)?;
                    existing
                }
                None => {
                    let path = self.final_path(&hash, format);
                    fs::rename(&tmp, &path)?;
                    sync_dir(&self.dir)?;
                    path
                }
            };
            Ok((hash, size, path))
        })();
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    /// Opens the partial download for appending at `offset`, discarding anything past it.
    pub fn open_part(&self, hash: &str, offset: u64) -> Result<File> {
        let mut f = OpenOptions::new().create(true).truncate(false).write(true).open(self.part_path(hash))?;
        f.set_len(offset)?;
        use std::io::Seek;
        f.seek(io::SeekFrom::Start(offset))?;
        Ok(f)
    }

    /// Verifies a completed download and moves it to its final name.
    pub fn finish_part(&self, hash: &str, format: &str, size: u64) -> Result<Finished> {
        let part = self.part_path(hash);
        let (actual, len) = hash_file(&part)?;
        if actual != hash || len != size {
            fs::remove_file(&part)?;
            return Ok(Finished::Corrupt);
        }
        let path = self.final_path(hash, format);
        fs::rename(&part, &path)?;
        sync_dir(&self.dir)?;
        Ok(Finished::Verified(path))
    }

    /// Deletes the complete file and any partial download for `hash`.
    pub fn delete(&self, hash: &str) -> Result<()> {
        if let Some((path, _)) = self.find(hash)? {
            fs::remove_file(path)?;
        }
        self.delete_part(hash)?;
        sync_dir(&self.dir)
    }

    pub fn delete_part(&self, hash: &str) -> Result<()> {
        match fs::remove_file(self.part_path(hash)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }

    pub fn available_space(&self) -> Result<u64> {
        Ok(fs4::available_space(&self.dir)?)
    }
}

pub fn is_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// BLAKE3 hex and size of a file.
pub fn hash_file(path: &Path) -> Result<(String, u64)> {
    let mut f = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; CHUNK];
    let mut size = 0u64;
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        size += n as u64;
        hasher.update(&buf[..n]);
    }
    Ok((hasher.finalize().to_hex().to_string(), size))
}

/// Is this error the disk being full?
pub fn is_disk_full(e: &anyhow::Error) -> bool {
    e.chain().any(|c| c.downcast_ref::<io::Error>().is_some_and(|io| matches!(io.kind(), io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded)))
        || e.chain().any(|c| c.downcast_ref::<DiskFull>().is_some())
}

/// Marks an I/O error as "disk full" when it is one, so callers can tell it apart.
fn disk_error(e: io::Error) -> anyhow::Error {
    if matches!(e.kind(), io::ErrorKind::StorageFull | io::ErrorKind::QuotaExceeded) {
        anyhow::Error::new(DiskFull).context(e.to_string())
    } else {
        e.into()
    }
}

#[derive(Debug)]
pub struct DiskFull;

impl std::fmt::Display for DiskFull {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("There isn't enough free space on this device.")
    }
}

impl std::error::Error for DiskFull {}

fn sync_dir(dir: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_hashes_dedupes_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path().join("books")).unwrap();
        let (hash, size, path) = store.import(&b"an epub"[..], "epub").unwrap();
        assert_eq!(hash, blake3::hash(b"an epub").to_hex().to_string());
        assert_eq!((size, path.extension().unwrap().to_str().unwrap()), (7, "epub"));
        let (again, _, path2) = store.import(&b"an epub"[..], "epub").unwrap();
        assert_eq!((again, path2), (hash.clone(), path));
        std::fs::write(store.dir().join("notes.txt"), "not ours").unwrap();
        assert_eq!(store.list().unwrap().len(), 1, "unknown names are ignored");
        assert!(store.import(&b""[..], "epub").is_err());
        assert!(store.list().unwrap().iter().all(|e| !matches!(e, Entry::ImportTemp { .. })), "failed import cleans up");
    }

    #[test]
    fn a_part_resumes_and_is_verified_before_taking_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let store = FileStore::open(dir.path().to_owned()).unwrap();
        let data = b"0123456789".repeat(1000);
        let hash = blake3::hash(&data).to_hex().to_string();
        store.open_part(&hash, 0).unwrap().write_all(&data[..4000]).unwrap();
        assert_eq!(store.part_len(&hash), 4000);
        store.open_part(&hash, 4000).unwrap().write_all(&data[4000..]).unwrap();
        assert!(matches!(store.finish_part(&hash, "epub", data.len() as u64).unwrap(), Finished::Verified(_)));
        assert_eq!(store.find(&hash).unwrap().unwrap().1, data.len() as u64);

        // Corrupted bytes in a resumed part are caught by the whole-file check.
        let other = blake3::hash(b"other").to_hex().to_string();
        store.open_part(&other, 0).unwrap().write_all(b"othxr").unwrap();
        assert_eq!(store.finish_part(&other, "epub", 5).unwrap(), Finished::Corrupt);
        assert_eq!(store.part_len(&other), 0, "the corrupt part is gone");
        assert!(store.find(&other).unwrap().is_none());

        store.delete(&hash).unwrap();
        assert!(store.list().unwrap().is_empty());
    }
}
