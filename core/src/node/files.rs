//! The file manager (PLAN.md §2, §6): moves book files between devices and keeps this
//! device's files, its holdings in the replicated document, and the library in agreement.
//!
//! Every change to which files this device holds (import, a download finishing, removing a
//! download, reconciliation after a purge or a crash) runs under `Files::lock`, so they happen
//! in a fixed order. A file is renamed into place before its holding is recorded, and a
//! holding is withdrawn before its file is deleted; startup reconciliation repairs whatever a
//! crash left between those steps.

use std::sync::atomic::AtomicU64;

use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::*;
use crate::{
    files::{DiskFull, Entry, FileStore, Finished, is_disk_full},
    model::Holding,
    proto::FileMsg,
};

const MAX_TRANSFERS: usize = 2;
/// Free space kept in reserve beyond the file itself.
const SPACE_MARGIN: u64 = 16 * 1024 * 1024;
/// While paused for space, how often free space is checked (no transfer is retried).
const SPACE_CHECK: Duration = Duration::from_secs(60);
const MAX_VERIFY_FAILURES: u32 = 3;
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);
const CHUNK: usize = 64 * 1024;
const PROGRESS_EVERY: u64 = 512 * 1024;

pub(super) struct Files {
    pub(super) store: FileStore,
    /// Serialises every change to which files this device holds.
    lock: tokio::sync::Mutex<()>,
    transfers: Mutex<HashMap<String, Transfer>>,
    retries: Mutex<HashMap<(String, EndpointId), Retry>>,
    space: Mutex<Option<SpaceIssue>>,
    /// Errors to show on a book, by file hash.
    errors: Mutex<HashMap<String, String>>,
    pub(super) wake: Notify,
    #[cfg(test)]
    pub(super) hooks: Mutex<Hooks>,
}

impl Files {
    pub(super) fn new(store: FileStore) -> Self {
        Self {
            store,
            lock: Default::default(),
            transfers: Default::default(),
            retries: Default::default(),
            space: Default::default(),
            errors: Default::default(),
            wake: Notify::new(),
            #[cfg(test)]
            hooks: Default::default(),
        }
    }
}

struct Transfer {
    book: String,
    peer: EndpointId,
    size: u64,
    received: Arc<AtomicU64>,
    task: tokio::task::AbortHandle,
}

#[derive(Default)]
struct Retry {
    failures: u32,
    verify_failures: u32,
    next: Option<Instant>,
    /// The peer said it doesn't have the file: its holdings were stale. Cleared when the
    /// document changes.
    missing: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpaceIssue {
    pub book: String,
    pub title: String,
    pub needed: u64,
    pub available: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TransferView {
    pub book: String,
    pub from: String,
    pub received: u64,
    pub size: u64,
}

/// Download state for the front end.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloads {
    pub transfers: Vec<TransferView>,
    /// Downloads are paused until there is room (or the reader taps Retry).
    pub paused_for_space: Option<SpaceIssue>,
    /// Errors by book id.
    pub errors: BTreeMap<String, String>,
}

/// Where an imported book comes from.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportSource {
    Path(PathBuf),
    #[serde(skip)]
    Bytes(Vec<u8>),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportMeta {
    pub format: String,
    #[serde(default)]
    pub original_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum Outcome {
    Done,
    /// The peer doesn't have it after all.
    Missing,
    /// Paused for lack of space; the partial file is kept.
    Paused,
    /// Bytes didn't verify; the partial file was deleted.
    Corrupt,
    /// The book was purged or removed from this device meanwhile.
    NotWanted,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct Hooks {
    /// Sender sleeps this long per chunk (a slow link).
    pub send_delay: Duration,
    /// Receiver sleeps this long per chunk (a slow disk).
    pub write_delay: Duration,
    pub available_space: Option<u64>,
    /// Receiver's disk "fills up" once the partial file reaches this length.
    pub disk_full_at: Option<u64>,
    /// Sender flips a byte in this many upcoming transfers.
    pub corrupt_sends: u32,
    /// Sender appends a byte past the announced size.
    pub extra_byte: bool,
    /// Requests this device has served.
    pub served: u32,
    /// Offsets this device has requested from.
    pub requested_offsets: Vec<u64>,
}

/// What the document says about files, for this device.
struct Wanted {
    /// Hash → the live book using it, for files this device keeps (in Trash or not).
    keep: HashMap<String, Book>,
    /// Hashes this device should download if missing (kept and not in Trash).
    download: HashSet<String>,
    /// Hashes this device's holdings list.
    held: HashSet<String>,
}

impl Node {
    fn wanted(&self) -> Wanted {
        let st = self.0.state.lock().unwrap();
        let me = self.0.id.to_string();
        let removed = model::read_removed_downloads(&st.doc, &me);
        let mut keep = HashMap::new();
        let mut download = HashSet::new();
        for book in model::list_books(&st.doc) {
            if book.purged || removed.contains(&book.id) {
                continue;
            }
            if !book.trashed {
                download.insert(book.hash.clone());
            }
            keep.insert(book.hash.clone(), book);
        }
        let held = model::read_holdings(&st.doc).remove(&me).unwrap_or_default().into_iter().collect();
        Wanted { keep, download, held }
    }

    async fn set_holdings(&self, changes: Vec<(String, bool)>) -> Result<()> {
        if changes.is_empty() {
            return Ok(());
        }
        let me = self.0.id.to_string();
        self.mutate(move |doc| {
            for (hash, held) in &changes {
                model::write_holding(doc, &me, hash, held.then(|| Holding { verified_at: model::now_iso() }).as_ref())?;
            }
            Ok(())
        })
        .await
    }

    // ---- Import, open, remove --------------------------------------------------------------

    /// Adds a book file to the library (or finds it if already there) and records that this
    /// device holds it. Importing a file whose every earlier copy was deleted forever adds it
    /// as a new book.
    pub async fn import_book(&self, source: ImportSource, meta: ImportMeta) -> Result<(Book, bool)> {
        model::validate_format(&meta.format)?;
        let _guard = self.0.files.lock.lock().await;
        let node = self.clone();
        let format = meta.format.clone();
        let (hash, size, _) = tokio::task::spawn_blocking(move || match source {
            ImportSource::Path(path) => {
                let file = std::fs::File::open(&path).with_context(|| format!("Couldn't open {}", path.display()))?;
                node.0.files.store.import(file, &format)
            }
            ImportSource::Bytes(bytes) => node.0.files.store.import(&bytes[..], &format),
        })
        .await??;
        let me = self.0.id.to_string();
        let new = NewBook { hash: hash.clone(), size, format: meta.format, original_name: meta.original_name, title: meta.title, author: meta.author };
        // If this fails, the file is left without a book; reconciliation deletes it.
        let result = self
            .mutate(move |doc| {
                let (id, added) = model::add_book(doc, &new, &me)?;
                model::set_download_removed(doc, &me, &id, false)?;
                model::write_holding(doc, &me, &hash, Some(&Holding { verified_at: model::now_iso() }))?;
                Ok((model::read_book(doc, &id).expect("just added"), added))
            })
            .await;
        self.0.files.wake.notify_one();
        result
    }

    /// The local file for a book, if this device holds it.
    pub fn book_file(&self, book: &str) -> Result<Option<PathBuf>> {
        let b = model::read_book(&self.0.state.lock().unwrap().doc, book).context("That book isn't in the library.")?;
        if b.purged || !self.wanted().held.contains(&b.hash) {
            return Ok(None);
        }
        Ok(self.0.files.store.find(&b.hash)?.map(|(path, _)| path))
    }

    /// True when no other device is known to hold this book's file (for the warning before
    /// "remove download").
    pub fn only_copy(&self, book: &str) -> Result<bool> {
        let st = self.0.state.lock().unwrap();
        let b = model::read_book(&st.doc, book).context("That book isn't in the library.")?;
        let me = self.0.id.to_string();
        let active: HashSet<String> = model::read_devices(&st.doc).into_iter().filter(|(_, r)| !r.removed).map(|(id, _)| id).collect();
        Ok(!model::read_holdings(&st.doc).iter().any(|(d, h)| *d != me && active.contains(d) && h.contains(&b.hash)))
    }

    /// "Remove download from this device": the book stays in the library everywhere.
    pub async fn remove_download(&self, book: String) -> Result<()> {
        let _guard = self.0.files.lock.lock().await;
        let me = self.0.id.to_string();
        let b = book.clone();
        let hash = self
            .mutate(move |doc| {
                let hash = model::read_book(doc, &b).context("That book isn't in the library.")?.hash;
                model::set_download_removed(doc, &me, &b, true)?;
                model::write_holding(doc, &me, &hash, None)?;
                Ok(hash)
            })
            .await?;
        self.cancel_transfer(&hash);
        // If deleting fails, reconciliation deletes it later: no book keeps it now.
        if let Err(e) = self.0.files.store.delete(&hash) {
            tracing::warn!("could not delete a removed download: {e:#}");
        }
        Ok(())
    }

    /// Opening a book whose download was removed brings it back.
    pub(super) async fn restore_download(&self, book: &str) -> Result<()> {
        let me = self.0.id.to_string();
        if !model::read_removed_downloads(&self.0.state.lock().unwrap().doc, &me).contains(book) {
            return Ok(());
        }
        let b = book.to_owned();
        self.mutate(move |doc| model::set_download_removed(doc, &me, &b, false)).await?;
        self.0.files.wake.notify_one();
        Ok(())
    }

    pub fn downloads(&self) -> Downloads {
        let by_hash: HashMap<String, String> = {
            let st = self.0.state.lock().unwrap();
            model::list_books(&st.doc).into_iter().filter(|b| !b.purged).map(|b| (b.hash, b.id)).collect()
        };
        let transfers = self
            .0
            .files
            .transfers
            .lock()
            .unwrap()
            .values()
            .map(|t| TransferView { book: t.book.clone(), from: t.peer.to_string(), received: t.received.load(Ordering::Relaxed), size: t.size })
            .collect();
        let errors = self.0.files.errors.lock().unwrap().iter().filter_map(|(h, e)| Some((by_hash.get(h)?.clone(), e.clone()))).collect();
        Downloads { transfers, paused_for_space: self.0.files.space.lock().unwrap().clone(), errors }
    }

    /// "Retry": resumes downloads paused for space and gives failed ones another go.
    pub fn retry_downloads(&self) {
        *self.0.files.space.lock().unwrap() = None;
        self.0.files.errors.lock().unwrap().clear();
        self.0.files.retries.lock().unwrap().clear();
        self.0.files.wake.notify_one();
        let _ = self.0.events.send(Event::StatusChanged);
    }

    fn cancel_transfer(&self, hash: &str) {
        if let Some(t) = self.0.files.transfers.lock().unwrap().remove(hash) {
            t.task.abort();
            let _ = self.0.events.send(Event::StatusChanged);
        }
    }

    fn available_space(&self) -> Result<u64> {
        #[cfg(test)]
        if let Some(space) = self.0.files.hooks.lock().unwrap().available_space {
            return Ok(space);
        }
        self.0.files.store.available_space()
    }

    // ---- Reconciliation and scheduling -----------------------------------------------------

    pub(super) async fn files_loop(self) {
        if let Err(e) = self.reconcile(true).await {
            tracing::warn!("file reconciliation failed: {e:#}");
        }
        let mut changed = self.0.changed.subscribe();
        let mut closing = self.0.closing.subscribe();
        let mut last_space_check = Instant::now();
        loop {
            if let Err(e) = self.reconcile(false).await {
                tracing::warn!("file reconciliation failed: {e:#}");
            }
            if last_space_check.elapsed() >= SPACE_CHECK {
                last_space_check = Instant::now();
                self.recheck_space();
            }
            self.schedule();
            tokio::select! {
                r = changed.changed() => {
                    if r.is_err() {
                        return;
                    }
                    self.0.files.retries.lock().unwrap().values_mut().for_each(|r| r.missing = false);
                }
                _ = self.0.files.wake.notified() => {}
                _ = tokio::time::sleep(Duration::from_secs(5)) => {}
                _ = closing.wait_for(|c| *c) => return,
            }
        }
    }

    /// Brings files, partial downloads and holdings in line with the library (PLAN.md §6).
    /// `startup` also clears leftover import temp files.
    pub(super) async fn reconcile(&self, startup: bool) -> Result<()> {
        let _guard = self.0.files.lock.lock().await;
        let node = self.clone();
        let changes = tokio::task::spawn_blocking(move || node.reconcile_files(startup)).await??;
        self.set_holdings(changes).await
    }

    fn reconcile_files(&self, startup: bool) -> Result<Vec<(String, bool)>> {
        let wanted = self.wanted();
        let store = &self.0.files.store;
        let transferring: HashSet<String> = self.0.files.transfers.lock().unwrap().keys().cloned().collect();
        for hash in &transferring {
            if !wanted.download.contains(hash) {
                self.cancel_transfer(hash);
            }
        }
        let mut changes = vec![];
        let mut complete = HashSet::new();
        for entry in store.list()? {
            match entry {
                Entry::Complete { hash, size, .. } => match wanted.keep.get(&hash) {
                    Some(book) if book.size == size => {
                        if !wanted.held.contains(&hash) {
                            // Renamed into place but the holding wasn't recorded (crash), or
                            // left from before a re-import: trust it only after verifying.
                            let path = store.find(&hash)?.map(|(p, _)| p).context("file vanished")?;
                            if crate::files::hash_file(&path)?.0 == hash {
                                changes.push((hash.clone(), true));
                            } else {
                                store.delete(&hash)?;
                                continue;
                            }
                        }
                        complete.insert(hash);
                    }
                    _ => {
                        store.delete(&hash)?;
                        if wanted.held.contains(&hash) {
                            changes.push((hash, false));
                        }
                    }
                },
                Entry::Part { hash, .. } => {
                    if !wanted.download.contains(&hash) {
                        store.delete_part(&hash)?;
                    }
                }
                Entry::ImportTemp { path } => {
                    if startup {
                        let _ = std::fs::remove_file(path);
                    }
                }
            }
        }
        for hash in &wanted.held {
            if !complete.contains(hash) && !changes.iter().any(|(h, _)| h == hash) {
                changes.push((hash.clone(), false));
            }
        }
        Ok(changes)
    }

    fn recheck_space(&self) {
        let mut space = self.0.files.space.lock().unwrap();
        if let Some(issue) = space.as_ref() {
            if self.available_space().is_ok_and(|a| a >= issue.needed) {
                *space = None;
                drop(space);
                let _ = self.0.events.send(Event::StatusChanged);
            }
        }
    }

    /// Starts downloads for missing files from connected devices that hold them.
    fn schedule(&self) {
        if self.0.files.space.lock().unwrap().is_some() {
            return;
        }
        let wanted = self.wanted();
        let holdings = model::read_holdings(&self.0.state.lock().unwrap().doc);
        let connected: Vec<EndpointId> = self.0.sessions.lock().unwrap().keys().copied().collect();
        let mut books: Vec<&Book> = wanted.keep.values().filter(|b| wanted.download.contains(&b.hash) && !wanted.held.contains(&b.hash)).collect();
        books.sort_by(|a, b| b.added_at.cmp(&a.added_at));
        let mut transfers = self.0.files.transfers.lock().unwrap();
        for book in books {
            if transfers.len() >= MAX_TRANSFERS {
                break;
            }
            if transfers.contains_key(&book.hash) {
                continue;
            }
            let retries = self.0.files.retries.lock().unwrap();
            let peer = connected.iter().copied().find(|p| {
                holdings.get(&p.to_string()).is_some_and(|h| h.contains(&book.hash))
                    && retries.get(&(book.hash.clone(), *p)).is_none_or(|r| {
                        !r.missing && r.verify_failures < MAX_VERIFY_FAILURES && r.next.is_none_or(|at| at <= Instant::now())
                    })
            });
            drop(retries);
            let Some(peer) = peer else { continue };
            let received = Arc::new(AtomicU64::new(0));
            let task = tokio::spawn(self.clone().fetch(peer, book.clone(), received.clone()));
            transfers.insert(book.hash.clone(), Transfer { book: book.id.clone(), peer, size: book.size, received, task: task.abort_handle() });
        }
    }

    async fn fetch(self, peer: EndpointId, book: Book, received: Arc<AtomicU64>) {
        let result = self.download(peer, &book, &received).await;
        self.0.files.transfers.lock().unwrap().remove(&book.hash);
        let key = (book.hash.clone(), peer);
        let mut retries = self.0.files.retries.lock().unwrap();
        match result {
            Ok(Outcome::Done) => {
                retries.retain(|(h, _), _| *h != book.hash);
                self.0.files.errors.lock().unwrap().remove(&book.hash);
            }
            Ok(Outcome::Missing) => retries.entry(key).or_default().missing = true,
            Ok(Outcome::Paused | Outcome::NotWanted) => {}
            Ok(Outcome::Corrupt) => {
                let r = retries.entry(key).or_default();
                r.verify_failures += 1;
                r.next = Some(Instant::now() + Duration::from_secs(r.verify_failures.into()));
                if r.verify_failures >= MAX_VERIFY_FAILURES {
                    let name = model::read_devices(&self.0.state.lock().unwrap().doc).into_iter().find(|(id, _)| *id == peer.to_string()).map_or_else(|| "another device".into(), |(_, r)| r.name);
                    self.0.files.errors.lock().unwrap().insert(book.hash.clone(), format!("Couldn't get a correct copy of this book from {name}."));
                }
            }
            Err(e) if is_disk_full(&e) => {
                self.pause_for_space(&book, book.size.saturating_sub(self.0.files.store.part_len(&book.hash)) + SPACE_MARGIN);
            }
            Err(e) => {
                tracing::debug!(%peer, hash = %book.hash, "download failed: {e:#}");
                let r = retries.entry(key).or_default();
                r.failures += 1;
                let delay = Duration::from_secs(1 << r.failures.min(6)).min(MAX_RETRY_DELAY);
                r.next = Some(Instant::now() + delay);
            }
        }
        drop(retries);
        let _ = self.0.events.send(Event::StatusChanged);
        self.0.files.wake.notify_one();
    }

    fn pause_for_space(&self, book: &Book, needed: u64) {
        let available = self.available_space().unwrap_or(0);
        let title = if book.title.is_empty() { book.original_name.clone() } else { book.title.clone() };
        *self.0.files.space.lock().unwrap() = Some(SpaceIssue { book: book.id.clone(), title, needed, available });
    }

    async fn download(&self, peer: EndpointId, book: &Book, received: &AtomicU64) -> Result<Outcome> {
        let store = &self.0.files.store;
        let hash = &book.hash;
        let mut offset = store.part_len(hash);
        if offset > book.size {
            store.delete_part(hash)?;
            offset = 0;
        }
        let needed = book.size - offset + SPACE_MARGIN;
        if self.available_space()? < needed {
            self.pause_for_space(book, needed);
            return Ok(Outcome::Paused);
        }
        #[cfg(test)]
        self.0.files.hooks.lock().unwrap().requested_offsets.push(offset);
        let conn = self.0.sessions.lock().unwrap().get(&peer).map(|s| s.conn.clone()).context("not connected")?;
        let (mut send, mut recv) = conn.open_bi().await?;
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&data_encoding::HEXLOWER.decode(hash.as_bytes())?);
        proto::write_msg(&mut send, &FileMsg::Want { hash: raw, offset }).await?;
        send.finish()?;
        match proto::read_msg::<_, FileMsg>(&mut recv).await? {
            Some(FileMsg::Have { size }) if size == book.size => {}
            Some(FileMsg::Have { .. }) => return Ok(Outcome::Corrupt),
            Some(FileMsg::Missing) => return Ok(Outcome::Missing),
            Some(FileMsg::Refused { reason }) => bail!("refused: {reason}"),
            _ => bail!("unexpected answer to a file request"),
        }
        let mut file = tokio::fs::File::from_std(store.open_part(hash, offset)?);
        let mut done = offset;
        received.store(done, Ordering::Relaxed);
        let mut last_event = done;
        let mut buf = vec![0u8; CHUNK];
        while done < book.size {
            let want = CHUNK.min((book.size - done) as usize);
            let n = recv.read(&mut buf[..want]).await?.context("the other device stopped sending")?;
            #[cfg(test)]
            {
                let (delay, full_at) = {
                    let h = self.0.files.hooks.lock().unwrap();
                    (h.write_delay, h.disk_full_at)
                };
                tokio::time::sleep(delay).await;
                if full_at.is_some_and(|at| done + n as u64 > at) {
                    file.sync_all().await?;
                    return Err(anyhow::Error::new(DiskFull));
                }
            }
            if let Err(e) = file.write_all(&buf[..n]).await {
                let e = anyhow::Error::from(e);
                let _ = file.sync_all().await;
                return Err(if is_disk_full(&e) { anyhow::Error::new(DiskFull) } else { e });
            }
            done += n as u64;
            received.store(done, Ordering::Relaxed);
            if done - last_event >= PROGRESS_EVERY {
                last_event = done;
                let _ = self.0.events.send(Event::StatusChanged);
            }
        }
        file.sync_all().await?;
        drop(file);
        if recv.read(&mut buf[..1]).await?.is_some_and(|n| n > 0) {
            store.delete_part(hash)?;
            return Ok(Outcome::Corrupt);
        }
        self.finish_download(&book.id).await
    }

    /// Verifies a complete partial file and records the holding, unless the book stopped
    /// being wanted meanwhile — a finishing download never brings a book back.
    pub(super) async fn finish_download(&self, book: &str) -> Result<Outcome> {
        let _guard = self.0.files.lock.lock().await;
        let b = model::read_book(&self.0.state.lock().unwrap().doc, book).context("That book isn't in the library.")?;
        if !self.wanted().keep.contains_key(&b.hash) {
            self.0.files.store.delete_part(&b.hash)?;
            return Ok(Outcome::NotWanted);
        }
        let node = self.clone();
        let (hash, format, size) = (b.hash.clone(), b.format.clone(), b.size);
        match tokio::task::spawn_blocking(move || node.0.files.store.finish_part(&hash, &format, size)).await?? {
            Finished::Corrupt => Ok(Outcome::Corrupt),
            Finished::Verified(_) => {
                self.set_holdings(vec![(b.hash, true)]).await?;
                Ok(Outcome::Done)
            }
        }
    }

    // ---- Serving -------------------------------------------------------------------------

    /// Answers one file request on a stream of an established session, so the peer is a
    /// paired device of this library. Only files of live books this device holds are served.
    pub(super) async fn serve_file(self, mut send: SendStream, mut recv: RecvStream) -> Result<()> {
        // Position sync runs on the session's first stream at the default priority.
        let _ = send.set_priority(-1);
        let Some(FileMsg::Want { hash, offset }) = proto::read_msg(&mut recv).await? else {
            bail!("malformed file request");
        };
        let hash = data_encoding::HEXLOWER.encode(&hash);
        let wanted = self.wanted();
        let found = match wanted.keep.get(&hash) {
            Some(book) if wanted.held.contains(&hash) => self.0.files.store.find(&hash)?.filter(|(_, size)| *size == book.size),
            _ => None,
        };
        let Some((path, size)) = found else {
            proto::write_msg(&mut send, &FileMsg::Missing).await?;
            send.finish()?;
            return Ok(());
        };
        if offset > size {
            proto::write_msg(&mut send, &FileMsg::Refused { reason: "offset past the end of the file".into() }).await?;
            send.finish()?;
            return Ok(());
        }
        proto::write_msg(&mut send, &FileMsg::Have { size }).await?;
        let mut file = tokio::fs::File::open(&path).await?;
        file.seek(std::io::SeekFrom::Start(offset)).await?;
        #[cfg(test)]
        let (delay, corrupt, extra) = {
            let mut h = self.0.files.hooks.lock().unwrap();
            h.served += 1;
            let corrupt = h.corrupt_sends > 0;
            h.corrupt_sends = h.corrupt_sends.saturating_sub(1);
            (h.send_delay, corrupt, h.extra_byte)
        };
        let mut buf = vec![0u8; CHUNK];
        loop {
            let n = tokio::io::AsyncReadExt::read(&mut file, &mut buf).await?;
            if n == 0 {
                break;
            }
            #[cfg(test)]
            {
                tokio::time::sleep(delay).await;
                if corrupt {
                    buf[0] ^= 0xff;
                }
            }
            send.write_all(&buf[..n]).await?;
        }
        #[cfg(test)]
        if extra {
            send.write_all(b"!").await?;
        }
        send.finish()?;
        let _ = send.stopped().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tests::new_book;

    async fn open(dir: &Path, name: &str) -> Node {
        Node::open(NodeConfig { data_dir: dir.to_owned(), device_name: name.into(), platform: "test".into(), port: 0, mdns: false })
            .await
            .unwrap()
    }

    async fn wait_for(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(60);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn pair(a: &Node, b: &Node) {
        b.use_code(&a.create_invite().unwrap().ticket).await.unwrap();
        wait_for("connected", || a.0.sessions.lock().unwrap().contains_key(&b.id()) && b.0.sessions.lock().unwrap().contains_key(&a.id())).await;
    }

    /// Imports `len` pseudo-random bytes as a book.
    async fn import(node: &Node, len: usize, seed: u8) -> Book {
        let mut bytes = vec![0u8; len];
        let mut out = blake3::Hasher::new().update(&[seed]).finalize_xof();
        out.fill(&mut bytes);
        let meta = ImportMeta { format: "epub".into(), original_name: format!("{seed}.epub"), title: format!("Book {seed}"), author: String::new() };
        node.import_book(ImportSource::Bytes(bytes), meta).await.unwrap().0
    }

    fn holds(node: &Node, book: &Book) -> bool {
        node.wanted().held.contains(&book.hash) && node.0.files.store.find(&book.hash).unwrap().is_some()
    }

    fn receiving(node: &Node, book: &Book) -> u64 {
        node.downloads().transfers.iter().find(|t| t.book == book.id).map_or(0, |t| t.received)
    }

    fn hooks(node: &Node) -> std::sync::MutexGuard<'_, Hooks> {
        node.0.files.hooks.lock().unwrap()
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn an_interrupted_transfer_resumes_from_the_partial_file() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        hooks(&b).write_delay = Duration::from_millis(5);
        pair(&a, &b).await;
        let book = import(&a, 3 << 20, 1).await;
        wait_for("part of it arrived", || receiving(&b, &book) > 1 << 20).await;
        b.shutdown().await;
        drop(b);
        let b = open(db.path(), "Phone").await;
        let part = b.0.files.store.part_len(&book.hash);
        assert!(part > 0, "the partial file is kept");
        wait_for("the download finished", || holds(&b, &book)).await;
        assert!(hooks(&b).requested_offsets[0] > 0, "resumed, not restarted");
        assert_eq!(std::fs::read(b.book_file(&book.id).unwrap().unwrap()).unwrap(), std::fs::read(a.book_file(&book.id).unwrap().unwrap()).unwrap(), "the resumed file matches the original");
        // Completing the download records the phone's holding locally; Linux learns it
        // asynchronously through document sync before its only-copy warning can clear.
        wait_for("Linux sees the phone holds it", || a.handoff(&book.id).unwrap().iter().any(|h| h.name == "Phone" && h.has_file)).await;
        assert!(!b.only_copy(&book.id).unwrap() && !a.only_copy(&book.id).unwrap());
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn corrupt_bytes_are_refetched_and_retries_are_bounded() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        pair(&a, &b).await;
        hooks(&a).corrupt_sends = 1;
        let book = import(&a, 200_000, 2).await;
        wait_for("refetched after one corrupt copy", || holds(&b, &book)).await;
        assert_eq!(hooks(&a).served, 2);

        // A peer that always sends bad bytes is given up on after three tries.
        hooks(&a).corrupt_sends = 100;
        hooks(&a).served = 0;
        let bad = import(&a, 200_000, 3).await;
        wait_for("error shown", || b.downloads().errors.contains_key(&bad.id)).await;
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert_eq!(hooks(&a).served, MAX_VERIFY_FAILURES, "no retry loop");
        assert!(!holds(&b, &bad));
        // Extra bytes past the announced size are rejected too.
        hooks(&a).corrupt_sends = 0;
        hooks(&a).extra_byte = true;
        b.retry_downloads();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!holds(&b, &bad));
        hooks(&a).extra_byte = false;
        b.retry_downloads();
        wait_for("good copy after Retry", || holds(&b, &bad)).await;
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn out_of_space_pauses_without_retrying_and_resumes() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        hooks(&b).available_space = Some(1 << 20);
        pair(&a, &b).await;
        let book = import(&a, 2 << 20, 4).await;
        wait_for("paused for space", || b.downloads().paused_for_space.is_some()).await;
        let issue = b.downloads().paused_for_space.unwrap();
        assert_eq!((issue.book.as_str(), issue.title.as_str()), (book.id.as_str(), "Book 4"));
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(hooks(&a).served, 0, "nothing was requested while paused");

        // Space frees up, then the disk fills mid-transfer: the partial file is kept.
        hooks(&b).available_space = None;
        hooks(&b).disk_full_at = Some(1 << 20);
        b.retry_downloads();
        wait_for("paused again mid-transfer", || b.downloads().paused_for_space.is_some()).await;
        let part = b.0.files.store.part_len(&book.hash);
        assert!(part > 0 && part <= 1 << 20, "{part}");
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(hooks(&a).served, 1, "no automatic retry");
        hooks(&b).disk_full_at = None;
        b.retry_downloads();
        wait_for("finished", || holds(&b, &book)).await;
        assert_eq!(hooks(&b).requested_offsets.last(), Some(&part), "resumed from the kept part");
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn purge_or_removal_while_downloading_leaves_nothing_behind() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        hooks(&b).write_delay = Duration::from_millis(5);
        pair(&a, &b).await;

        // Purged on Linux mid-transfer.
        let book = import(&a, 2 << 20, 5).await;
        wait_for("transfer under way", || receiving(&b, &book) > 0).await;
        a.set_trashed(vec![book.id.clone()], true).await.unwrap();
        a.delete_forever(vec![book.id.clone()]).await.unwrap();
        wait_for("nothing left on the phone", || {
            !holds(&b, &book) && b.0.files.store.part_len(&book.hash) == 0 && b.downloads().transfers.is_empty()
        })
        .await;
        wait_for("nothing left on Linux", || a.0.files.store.find(&book.hash).unwrap().is_none() && !a.wanted().held.contains(&book.hash)).await;

        // Removed on the phone after the last chunk, just before the download finishes.
        hooks(&b).write_delay = Duration::ZERO;
        let other = import(&a, 100_000, 6).await;
        wait_for("arrived", || holds(&b, &other)).await;
        b.remove_download(other.id.clone()).await.unwrap();
        assert!(!holds(&b, &other));
        let mut part = b.0.files.store.open_part(&other.hash, 0).unwrap();
        std::io::Write::write_all(&mut part, &std::fs::read(a.book_file(&other.id).unwrap().unwrap()).unwrap()).unwrap();
        drop(part);
        assert_eq!(b.finish_download(&other.id).await.unwrap(), Outcome::NotWanted);
        assert!(!holds(&b, &other) && b.0.files.store.part_len(&other.hash) == 0);
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!holds(&b, &other), "a removed download isn't fetched again");
        // Opening it again brings it back.
        b.open_book(other.id.clone()).await.unwrap();
        wait_for("downloaded again on open", || holds(&b, &other)).await;
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn startup_reconciles_whatever_a_crash_left() {
        let dir = tempfile::tempdir().unwrap();
        let node = open(dir.path(), "Linux").await;
        let me = node.id().to_string();
        let kept = import(&node, 50_000, 7).await;
        let damaged = import(&node, 50_000, 8).await;
        let vanished = import(&node, 50_000, 9).await;
        let removed = import(&node, 50_000, 10).await;
        let wanted_part = new_book(11);
        node.add_book(wanted_part.clone()).await.unwrap();
        // Crash states: file renamed in but holding not recorded; holding withdrawn but file
        // not deleted; holding recorded but file gone; a damaged file; stray parts and temps.
        let (k, d, r) = (kept.hash.clone(), damaged.hash.clone(), removed.id.clone());
        let rh = removed.hash.clone();
        let me2 = me.clone();
        node.mutate(move |doc| {
            model::write_holding(doc, &me2, &k, None)?;
            model::write_holding(doc, &me2, &d, None)?;
            model::set_download_removed(doc, &me2, &r, true)?;
            model::write_holding(doc, &me2, &rh, None)
        })
        .await
        .unwrap();
        let store = &node.0.files.store;
        let damaged_path = store.find(&damaged.hash).unwrap().unwrap().0;
        let mut bytes = std::fs::read(&damaged_path).unwrap();
        bytes[0] ^= 1;
        std::fs::write(&damaged_path, bytes).unwrap();
        std::fs::remove_file(store.find(&vanished.hash).unwrap().unwrap().0).unwrap();
        std::fs::write(store.part_path(&new_book(12).hash), b"unwanted part").unwrap();
        std::fs::write(store.part_path(&wanted_part.hash), b"wanted part").unwrap();
        std::fs::write(store.dir().join("import-1.tmp"), b"leftover").unwrap();
        std::fs::write(store.dir().join("README"), b"not ours").unwrap();
        node.shutdown().await;
        drop(node);

        let node = open(dir.path(), "Linux").await;
        wait_for("reconciled", || node.0.files.store.list().unwrap().len() == 2).await;
        let held = node.wanted().held;
        assert!(held.contains(&kept.hash) && holds(&node, &kept), "verified file got its holding back");
        assert!(!held.contains(&damaged.hash) && node.0.files.store.find(&damaged.hash).unwrap().is_none(), "damaged file deleted");
        assert!(!held.contains(&vanished.hash), "holding for a missing file withdrawn");
        assert!(node.0.files.store.find(&removed.hash).unwrap().is_none(), "removed download deleted");
        assert_eq!(node.0.files.store.part_len(&wanted_part.hash), 11, "wanted part kept");
        assert_eq!(node.0.files.store.part_len(&new_book(12).hash), 0);
        assert!(node.0.files.store.dir().join("README").exists());
        assert!(!node.0.files.store.dir().join("import-1.tmp").exists());
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn requests_are_checked() {
        let (da, db, dc) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        pair(&a, &b).await;
        let book = import(&a, 10_000, 13).await;
        let mut raw = [0u8; 32];
        raw.copy_from_slice(&data_encoding::HEXLOWER.decode(book.hash.as_bytes()).unwrap());
        // Right after pairing, a connection both sides dialed at once may be replaced by the
        // preferred one; a request on the replaced connection fails and is simply asked again.
        let ask = |node: Node, peer: EndpointId, msg: FileMsg| async move {
            for _ in 0..20 {
                let attempt = async {
                    let conn = node.0.sessions.lock().unwrap().get(&peer).map(|s| s.conn.clone()).context("no session")?;
                    let (mut send, mut recv) = conn.open_bi().await?;
                    proto::write_msg(&mut send, &msg).await?;
                    send.finish()?;
                    proto::read_msg::<_, FileMsg>(&mut recv).await
                };
                match attempt.await {
                    Ok(reply) => return reply,
                    Err(e) => eprintln!("asking again: {e:#}"),
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            panic!("no answer");
        };
        assert!(matches!(ask(b.clone(), a.id(), FileMsg::Want { hash: raw, offset: 10_001 }).await, Some(FileMsg::Refused { .. })));
        assert!(matches!(ask(b.clone(), a.id(), FileMsg::Want { hash: [0; 32], offset: 0 }).await, Some(FileMsg::Missing)));
        assert!(matches!(ask(b.clone(), a.id(), FileMsg::Want { hash: raw, offset: 10_000 }).await, Some(FileMsg::Have { size: 10_000 })));

        // An unpaired device can't get a session, so it can't ask at all.
        let c = open(dc.path(), "Stranger").await;
        let attempt = async {
            let conn = c.0.endpoint.connect(a.addr(), SYNC_ALPN).await?;
            let (mut send, mut recv) = conn.open_bi().await?;
            proto::write_msg(&mut send, &FileMsg::Want { hash: raw, offset: 0 }).await?;
            proto::read_msg::<_, FileMsg>(&mut recv).await
        };
        assert!(!matches!(attempt.await, Ok(Some(FileMsg::Have { .. }))));
        for n in [a, b, c] {
            n.shutdown().await;
        }
    }

    /// PLAN.md §8 B2: a position commit is acknowledged within 2 s while a large file
    /// transfers, at full speed and over a slow link with a slow disk.
    #[tokio::test(flavor = "multi_thread")]
    async fn positions_stay_responsive_during_a_large_transfer() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        pair(&a, &b).await;
        let (reading, _) = a.add_book(new_book(20)).await.unwrap();
        let place = |n: u32| Location { locator: serde_json::json!({ "cfi": n.to_string() }), fraction: 0.1, snippet: String::new() };
        let has_place = |a: &Node| a.handoff(&reading.id).unwrap().iter().any(|h| h.name == "Phone" && h.has_place);

        for (seed, slow) in [(21u8, false), (22, true)] {
            if slow {
                hooks(&a).send_delay = Duration::from_millis(5);
                hooks(&b).write_delay = Duration::from_millis(5);
            }
            let big = import(&a, 200 << 20, seed).await;
            wait_for("transfer under way", || receiving(&b, &big) > 8 << 20).await;
            let mut worst = Duration::ZERO;
            for n in 0..3 {
                let started = Instant::now();
                a.flush_position(reading.id.clone(), place(n + seed as u32 * 10)).await.unwrap();
                wait_for("position acknowledged", || has_place(&a)).await;
                worst = worst.max(started.elapsed());
            }
            let still_transferring = receiving(&b, &big) < big.size;
            eprintln!("slow={slow}: worst position ack {worst:?} during transfer (still transferring: {still_transferring})");
            assert!(still_transferring, "the measurement must overlap the transfer");
            assert!(worst < Duration::from_secs(2), "position ack took {worst:?}");
            b.remove_download(big.id.clone()).await.unwrap();
        }
        a.shutdown().await;
        b.shutdown().await;
    }
}
