//! A device's view of one synchronized library: storage, pairing, and live sync sessions.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    net::{Ipv4Addr, SocketAddr, SocketAddrV4},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use anyhow::{Context, Result, anyhow, bail};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayMode, SecretKey, TransportAddr,
    endpoint::{Connection, IdleTimeout, QuicTransportConfig, presets},
    protocol::{AcceptError, ProtocolHandler, Router},
};
use iroh_mdns_address_lookup::{DiscoveryEvent, MdnsAddressLookup};
use loro::{ID, LoroDoc, VersionVector};
use n0_future::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::sync::{Notify, broadcast, mpsc, watch};

use crate::{
    model::{self, Book, BookPatch, DeviceRecord, NewBook},
    positions::{self, Decision, Location, Placed, PositionRecord, RevRef, Selection},
    proto::{self, PAIR_ALPN, PROTOCOL_VERSION, PairMsg, SYNC_ALPN, SyncMsg, Ticket, TicketKind},
    store::{DocStore, write_atomic},
};

const INVITE_LIFETIME: Duration = Duration::from_secs(10 * 60);
const DIAL_INTERVAL: Duration = Duration::from_secs(2);
/// Sync dials are retried often, so give up on unreachable addresses quickly.
const DIAL_TIMEOUT: Duration = Duration::from_secs(4);
const PAIR_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub struct NodeConfig {
    pub data_dir: PathBuf,
    pub device_name: String,
    pub platform: String,
    /// IPv4 UDP port to listen on. 0 picks any free port; a busy port falls back to any.
    pub port: u16,
    /// Advertise and discover paired devices on the local network.
    pub mdns: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Event {
    /// Books, positions or settings changed (locally or from another device).
    LibraryChanged,
    StatusChanged,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    pub device_id: String,
    pub device_name: String,
    pub platform: String,
    pub library_id: String,
    pub addresses: Vec<String>,
    /// Some local changes are only in memory because writing them failed.
    pub unsaved: bool,
    pub devices: Vec<DeviceStatus>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub id: String,
    pub name: String,
    pub platform: String,
    pub is_self: bool,
    pub removed: bool,
    pub connected: bool,
    /// The device has durably confirmed every change this device has.
    pub has_all_my_changes: bool,
    pub last_seen: Option<String>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub ticket: String,
    pub qr_svg: String,
    pub expires_at: String,
    pub kind: TicketKind,
}

/// A book as the library shows it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookView {
    #[serde(flatten)]
    pub book: Book,
    /// Furthest any device has read (display only; never used to pick a position).
    pub progress: Option<f64>,
    /// Devices holding a complete, verified copy of the file.
    pub held_by: Vec<String>,
}

/// Whether another device could continue this book right now (PLAN.md §5).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Handoff {
    pub device: String,
    pub name: String,
    pub connected: bool,
    /// The device durably holds this device's latest saved position for the book (or this
    /// device has none). A newer local checkpoint makes this false until it is committed and
    /// acknowledged; the front end also clears it as soon as the reader moves.
    pub has_place: bool,
    /// The device holds the complete, verified book file.
    pub has_file: bool,
}

/// What the reader chose when positions competed, or when continuing elsewhere.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Choice {
    /// Go to one of the heads that was shown.
    Head(RevRef),
    /// Stay at (or go back to) a location on this device.
    Here(Location),
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairResult {
    /// The other device's name.
    pub device: String,
    /// True when this device joined the other's library; false when it added the other.
    pub joined: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Meta {
    library_id: String,
    device_name: String,
    /// Layout of the stored document; see [`SCHEMA`].
    #[serde(default)]
    schema: u32,
}

/// Version of the document layout (see `model.rs`).
const SCHEMA: u32 = 1;

/// Local, unreplicated knowledge about other devices.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PeerInfo {
    addrs: Vec<SocketAddr>,
    /// Encoded version vector the peer has durably acknowledged.
    acked: Option<Vec<u8>>,
    last_seen: Option<String>,
}

/// This device's own positions as saved locally (`positions.local.json`). Checkpoints land
/// here first and reach the replicated document less often (PLAN.md §4).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalPositions {
    /// Last revision number used by this device; never reused.
    counter: u64,
    books: BTreeMap<String, LocalPosition>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalPosition {
    record: PositionRecord,
    /// The revision last committed to the replicated document, and the operation that did it,
    /// so the handoff status can check that exact change against a peer's acknowledgement.
    committed: Option<Committed>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Committed {
    rev: u64,
    peer: u64,
    counter: i32,
}

struct State {
    doc: LoroDoc,
    store: DocStore,
    meta: Meta,
    peers: HashMap<String, PeerInfo>,
}

struct Session {
    conn: Connection,
    dialer: EndpointId,
    token: u64,
}

struct PendingInvite {
    kind: TicketKind,
    secret: [u8; 32],
    expires: Instant,
}

struct Inner {
    _lock: std::fs::File,
    id: EndpointId,
    dir: PathBuf,
    platform: String,
    endpoint: Endpoint,
    router: OnceLock<Router>,
    mdns: Option<MdnsAddressLookup>,
    state: Mutex<State>,
    /// Lock order: `state` before `local`, never the reverse.
    local: Mutex<LocalPositions>,
    events: broadcast::Sender<Event>,
    changed: watch::Sender<u64>,
    sessions: Mutex<HashMap<EndpointId, Session>>,
    dialing: Mutex<HashSet<EndpointId>>,
    backoff: Mutex<HashMap<EndpointId, (Instant, Duration)>>,
    errors: Mutex<HashMap<EndpointId, String>>,
    invites: Mutex<Vec<PendingInvite>>,
    next_token: AtomicU64,
    wake: Notify,
    closing: watch::Sender<bool>,
}

#[derive(Clone)]
pub struct Node(Arc<Inner>);

impl std::fmt::Debug for Node {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Node").field("id", &self.0.id).finish()
    }
}

impl Node {
    pub async fn open(cfg: NodeConfig) -> Result<Self> {
        let dir = cfg.data_dir.clone();
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        // Only one process may write a library at a time; the lock is released on exit.
        // A process that is shutting down (app update, crash restart) may still hold it
        // briefly, so wait a few seconds before giving up.
        let lock = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join("lock"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        while !try_lock(&lock).context("locking the library")? {
            if Instant::now() > deadline {
                bail!("Another copy of reader-sync is already using {}.", dir.display());
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let key = load_or_create_key(&dir.join("device.key"))?;
        let id = key.public();

        let meta_path = dir.join("meta.json");
        let mut meta: Meta = match std::fs::read(&meta_path) {
            Ok(bytes) => serde_json::from_slice(&bytes).context("meta.json is damaged")?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Meta {
                library_id: uuid::Uuid::new_v4().to_string(),
                device_name: cfg.device_name.clone(),
                schema: SCHEMA,
            },
            Err(e) => return Err(e).context("reading meta.json"),
        };
        if meta.schema != SCHEMA {
            bail!(
                "This library ({}) was made by a different version of reader-sync (layout {}; this one reads {SCHEMA}). \
                 Update the app.",
                dir.display(),
                meta.schema
            );
        }
        if !cfg.device_name.trim().is_empty() {
            meta.device_name = clean_name(&cfg.device_name);
        }
        write_atomic(&meta_path, &serde_json::to_vec_pretty(&meta)?)?;
        let peers = match std::fs::read(dir.join("peers.json")) {
            Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
            Err(_) => HashMap::new(),
        };

        let doc = LoroDoc::new();
        let mut store = DocStore::open(&dir, &doc)?;
        let me = id.to_string();
        let local = load_local_positions(&dir, &doc, &me)?;
        let own = model::read_devices(&doc).into_iter().find(|(k, _)| *k == me).map(|(_, r)| r);
        if own.as_ref().is_none_or(|r| r.name != meta.device_name || r.platform != cfg.platform) {
            let record = DeviceRecord {
                name: meta.device_name.clone(),
                platform: cfg.platform.clone(),
                added_at: own.as_ref().map_or_else(model::now_iso, |r| r.added_at.clone()),
                added_by: own.as_ref().map_or_else(|| me.clone(), |r| r.added_by.clone()),
                removed: own.as_ref().is_some_and(|r| r.removed),
            };
            model::write_device(&doc, &me, &record)?;
            doc.commit();
            store.persist(&doc)?;
        }

        let endpoint = bind_endpoint(&key, cfg.port).await?;
        let mdns = if cfg.mdns {
            let mdns = MdnsAddressLookup::builder().build(id).map_err(|e| anyhow!("starting mDNS: {e}"))?;
            endpoint.address_lookup().map_err(|e| anyhow!("{e}"))?.add(mdns.clone());
            Some(mdns)
        } else {
            None
        };

        let node = Node(Arc::new(Inner {
            _lock: lock,
            id,
            dir,
            platform: cfg.platform,
            endpoint: endpoint.clone(),
            router: OnceLock::new(),
            mdns,
            state: Mutex::new(State { doc, store, meta, peers }),
            local: Mutex::new(local),
            events: broadcast::channel(256).0,
            changed: watch::channel(0).0,
            sessions: Mutex::default(),
            dialing: Mutex::default(),
            backoff: Mutex::default(),
            errors: Mutex::default(),
            invites: Mutex::default(),
            next_token: AtomicU64::new(1),
            wake: Notify::new(),
            closing: watch::channel(false).0,
        }));
        let router = Router::builder(endpoint)
            .accept(SYNC_ALPN, SyncHandler(node.clone()))
            .accept(PAIR_ALPN, PairHandler(node.clone()))
            .spawn();
        let _ = node.0.router.set(router);
        node.recover_positions().await;
        node.publish_addresses().await;
        tokio::spawn(node.clone().dial_loop());
        if node.0.mdns.is_some() {
            tokio::spawn(node.clone().mdns_loop());
        }
        tracing::info!(%id, "node ready");
        Ok(node)
    }

    pub fn id(&self) -> EndpointId {
        self.0.id
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.0.events.subscribe()
    }

    /// Current address, including direct IP addresses (for tests and diagnostics).
    pub fn addr(&self) -> EndpointAddr {
        self.0.endpoint.addr()
    }

    /// Records addresses to try for a device (normally learned from mDNS or tickets).
    pub fn add_peer_addrs(&self, id: EndpointId, addrs: impl IntoIterator<Item = SocketAddr>) {
        let mut st = self.0.state.lock().unwrap();
        let info = st.peers.entry(id.to_string()).or_default();
        let before = info.addrs.clone();
        for a in addrs {
            if !info.addrs.contains(&a) {
                info.addrs.insert(0, a);
            }
        }
        info.addrs.truncate(8);
        // mDNS repeats announcements every few seconds; only touch the disk on a change.
        if info.addrs != before {
            self.save_peers(&st);
        }
        drop(st);
        if !self.0.sessions.lock().unwrap().contains_key(&id) {
            self.0.backoff.lock().unwrap().remove(&id);
            self.0.wake.notify_one();
        }
    }

    /// Call when the network may have changed (app resumed, Wi-Fi switched): rebinds
    /// interfaces and retries every paired device immediately.
    pub async fn network_changed(&self) {
        self.0.endpoint.network_change().await;
        self.0.backoff.lock().unwrap().clear();
        self.0.wake.notify_one();
    }

    pub async fn shutdown(&self) {
        let _ = self.0.closing.send(true);
        if let Some(router) = self.0.router.get() {
            let _ = router.shutdown().await;
        }
        self.0.endpoint.close().await;
    }

    // ---- Books -------------------------------------------------------------------------

    /// The library, without books deleted forever.
    pub fn books(&self) -> Vec<BookView> {
        let st = self.0.state.lock().unwrap();
        let local = self.0.local.lock().unwrap();
        let holdings = model::read_holdings(&st.doc);
        let me = self.0.id.to_string();
        model::list_books(&st.doc)
            .into_iter()
            .filter(|b| !b.purged)
            .map(|book| {
                let mut positions = model::read_positions(&st.doc, &book.id);
                if let Some(own) = local.books.get(&book.id) {
                    positions.insert(me.clone(), own.record.clone());
                }
                let progress = positions.values().map(|p| p.fraction).reduce(f64::max);
                let held_by = holdings.iter().filter(|(_, h)| h.contains(&book.hash)).map(|(d, _)| d.clone()).collect();
                BookView { book, progress, held_by }
            })
            .collect()
    }

    /// Records a book in the library, or finds it if the same file is already there.
    /// Returns the book and whether it was added. (Storing and sending the file itself is the
    /// file store's job.)
    pub async fn add_book(&self, new: NewBook) -> Result<(Book, bool)> {
        let by = self.0.id.to_string();
        self.mutate(move |doc| {
            let (id, added) = model::add_book(doc, &new, &by)?;
            Ok((model::read_book(doc, &id).expect("just added"), added))
        })
        .await
    }

    pub async fn update_book(&self, id: String, patch: BookPatch) -> Result<Book> {
        self.mutate(move |doc| {
            model::update_book(doc, &id, &patch)?;
            Ok(model::read_book(doc, &id).expect("exists"))
        })
        .await
    }

    /// Moves books to Trash or back, in one change.
    pub async fn set_trashed(&self, ids: Vec<String>, trashed: bool) -> Result<usize> {
        self.mutate(move |doc| {
            let mut changed = 0;
            for id in &ids {
                changed += model::set_trashed(doc, id, trashed)? as usize;
            }
            Ok(changed)
        })
        .await
    }

    /// Deletes books in Trash forever, on every device.
    pub async fn delete_forever(&self, ids: Vec<String>) -> Result<usize> {
        let by = self.0.id.to_string();
        self.mutate(move |doc| {
            let mut changed = 0;
            for id in &ids {
                changed += model::purge(doc, id, &by)? as usize;
            }
            Ok(changed)
        })
        .await
    }

    // ---- Settings ------------------------------------------------------------------------

    pub fn settings(&self) -> serde_json::Map<String, serde_json::Value> {
        model::read_settings(&self.0.state.lock().unwrap().doc)
    }

    /// Changes the settings in `patch` (a `null` value removes one) and returns them all.
    pub async fn set_settings(&self, patch: serde_json::Map<String, serde_json::Value>) -> Result<serde_json::Map<String, serde_json::Value>> {
        self.mutate(move |doc| {
            model::write_settings(doc, &patch)?;
            Ok(model::read_settings(doc))
        })
        .await
    }

    // ---- Positions -----------------------------------------------------------------------

    /// Every non-removed device's latest position for a book, with this device's own taken
    /// from its local checkpoints (which may be ahead of the replicated document).
    fn position_records(&self, book: &str) -> Result<BTreeMap<String, PositionRecord>> {
        let st = self.0.state.lock().unwrap();
        let b = model::read_book(&st.doc, book).context("That book isn't in the library.")?;
        if b.purged {
            bail!("That book was deleted forever.");
        }
        let removed: HashSet<String> = model::read_devices(&st.doc).into_iter().filter(|(_, r)| r.removed).map(|(id, _)| id).collect();
        let mut records = model::read_positions(&st.doc, book);
        records.retain(|device, _| !removed.contains(device));
        let me = self.0.id.to_string();
        if let Some(own) = self.0.local.lock().unwrap().books.get(book) {
            if records.get(&me).is_none_or(|r| r.rev <= own.record.rev) {
                records.insert(me, own.record.clone());
            }
        }
        Ok(records)
    }

    /// Where the book would open now, without saving anything. The reader uses this when
    /// positions change mid-session: anything other than `mine` shows the banner.
    pub fn peek_position(&self, book: &str) -> Result<Selection> {
        Ok(positions::select(&self.position_records(book)?, &self.0.id.to_string()))
    }

    /// Opens a book: recovers an uncommitted checkpoint first, then decides where to open.
    /// A continuation from another device is saved immediately as this device's position.
    pub async fn open_book(&self, book: String) -> Result<Selection> {
        self.commit_position(book.clone()).await?;
        let selection = self.peek_position(&book)?;
        if selection.decision == Decision::Continued {
            let head = selection.position.clone().expect("a continuation has a position");
            self.save_position(&book, location_of(&head.record), &selection.heads).await?;
        }
        Ok(selection)
    }

    /// Settles a choice: the divergence prompt, "Back to where I was here", or the banner's
    /// Go there / Stay here. `shown` are the heads the reader was shown; all of them count as
    /// seen, so the choice resolves the divergence on every device. Saved immediately.
    pub async fn resolve_position(&self, book: String, shown: Vec<RevRef>, choice: Choice) -> Result<Placed> {
        let records = self.position_records(&book)?;
        let acknowledged: Vec<Placed> = shown
            .iter()
            .filter_map(|r| {
                let current = records.get(&r.device)?;
                // A newer revision may have arrived since; acknowledge only what was shown.
                let record = if current.rev == r.rev {
                    current.clone()
                } else if r.rev < current.rev {
                    PositionRecord { rev: r.rev, seen: Default::default(), ..current.clone() }
                } else {
                    return None;
                };
                Some(Placed { device: r.device.clone(), record })
            })
            .collect();
        let location = match choice {
            Choice::Here(location) => location,
            Choice::Head(head) => {
                let placed = acknowledged
                    .iter()
                    .find(|p| p.device == head.device && p.record.rev == head.rev && records[&p.device].rev == head.rev)
                    .context("That position has changed; choose again.")?;
                location_of(&placed.record)
            }
        };
        let record = self.save_position(&book, location, &acknowledged).await?;
        Ok(Placed { device: self.0.id.to_string(), record })
    }

    /// Saves the reader's location locally (the frequent, debounced checkpoint).
    pub async fn checkpoint(&self, book: String, location: Location) -> Result<u64> {
        let node = self.clone();
        Ok(tokio::task::spawn_blocking(move || node.write_checkpoint(&book, location, &[])).await??.rev)
    }

    /// Saves the location and commits it to the replicated document at once: for closing,
    /// going to background, and handing off.
    pub async fn flush_position(&self, book: String, location: Location) -> Result<u64> {
        Ok(self.save_position(&book, location, &[]).await?.rev)
    }

    async fn save_position(&self, book: &str, location: Location, acknowledged: &[Placed]) -> Result<PositionRecord> {
        let node = self.clone();
        let (b, ack) = (book.to_owned(), acknowledged.to_vec());
        let record = tokio::task::spawn_blocking(move || node.write_checkpoint(&b, location, &ack)).await??;
        self.commit_position(book.to_owned()).await?;
        Ok(record)
    }

    /// Writes a new revision of this device's position to `positions.local.json`.
    fn write_checkpoint(&self, book: &str, location: Location, acknowledged: &[Placed]) -> Result<PositionRecord> {
        let me = self.0.id.to_string();
        let replicated_own = {
            let st = self.0.state.lock().unwrap();
            let b = model::read_book(&st.doc, book).context("That book isn't in the library.")?;
            if b.purged {
                bail!("That book was deleted forever.");
            }
            model::read_positions(&st.doc, book).remove(&me)
        };
        let mut local = self.0.local.lock().unwrap();
        let previous = match (local.books.get(book).map(|l| &l.record), replicated_own.as_ref()) {
            (Some(l), Some(r)) if r.rev > l.rev => Some(r.clone()),
            (Some(l), _) => Some(l.clone()),
            (None, r) => r.cloned(),
        };
        let rev = local.counter + 1;
        let seen = positions::next_seen(previous.as_ref(), acknowledged.iter().map(|p| (p.device.as_str(), &p.record)), &me, rev);
        let record = PositionRecord { rev, seen, locator: location.locator, fraction: location.fraction, snippet: location.snippet, at: model::now_iso() };
        record.validate()?;
        let mut next = local.clone();
        next.counter = rev;
        let committed = next.books.get(book).and_then(|l| l.committed);
        next.books.insert(book.to_owned(), LocalPosition { record: record.clone(), committed });
        write_atomic(&self.0.dir.join(LOCAL_POSITIONS), &serde_json::to_vec(&next)?).context("Couldn't save the reading position")?;
        *local = next;
        drop(local);
        let _ = self.0.events.send(Event::StatusChanged);
        Ok(record)
    }

    /// Commits this device's latest local position for `book` to the replicated document,
    /// if it isn't there yet.
    pub async fn commit_position(&self, book: String) -> Result<()> {
        let Some(record) = self.0.local.lock().unwrap().books.get(&book).filter(|l| l.committed.is_none_or(|c| c.rev != l.record.rev)).map(|l| l.record.clone()) else {
            return Ok(());
        };
        let me = self.0.id.to_string();
        let (b, r) = (book.clone(), record.clone());
        let (peer, counter) = self
            .mutate(move |doc| {
                model::write_position(doc, &b, &me, &r)?;
                doc.commit();
                let peer = doc.peer_id();
                let counter = doc.oplog_vv().get_last(peer).context("no operation was recorded")?;
                Ok((peer, counter))
            })
            .await?;
        let node = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut local = node.0.local.lock().unwrap();
            let mut next = local.clone();
            if let Some(entry) = next.books.get_mut(&book) {
                if entry.committed.is_none_or(|c| c.rev <= record.rev) {
                    entry.committed = Some(Committed { rev: record.rev, peer, counter });
                }
            }
            // If this fails the handoff status stays pessimistic and the next commit retries.
            match serde_json::to_vec(&next).map_err(anyhow::Error::from).and_then(|b| write_atomic(&node.0.dir.join(LOCAL_POSITIONS), &b)) {
                Ok(()) => *local = next,
                Err(e) => tracing::warn!("could not record the committed position: {e:#}"),
            }
        })
        .await?;
        let _ = self.0.events.send(Event::StatusChanged);
        Ok(())
    }

    /// After a crash or power loss, commits checkpoints that never reached the replicated
    /// document, so other devices (and the next selection) see the real last position.
    async fn recover_positions(&self) {
        let pending: Vec<String> = self
            .0
            .local
            .lock()
            .unwrap()
            .books
            .iter()
            .filter(|(_, l)| l.committed.is_none_or(|c| c.rev != l.record.rev))
            .map(|(b, _)| b.clone())
            .collect();
        for book in pending {
            if let Err(e) = self.commit_position(book.clone()).await {
                tracing::warn!(%book, "could not recover the reading position: {e:#}");
            }
        }
    }

    /// For each other device: does it hold my latest saved position and the book file?
    pub fn handoff(&self, book: &str) -> Result<Vec<Handoff>> {
        let st = self.0.state.lock().unwrap();
        let b = model::read_book(&st.doc, book).context("That book isn't in the library.")?;
        let me = self.0.id.to_string();
        let own = self.0.local.lock().unwrap().books.get(book).cloned();
        let has_own_replicated = model::read_positions(&st.doc, book).contains_key(&me);
        let holdings = model::read_holdings(&st.doc);
        let sessions = self.0.sessions.lock().unwrap();
        Ok(model::read_devices(&st.doc)
            .into_iter()
            .filter(|(id, r)| *id != me && !r.removed)
            .map(|(id, r)| {
                let acked = st.peers.get(&id).and_then(|i| i.acked.as_ref()).and_then(|v| VersionVector::decode(v).ok());
                let has_place = match &own {
                    None => !has_own_replicated,
                    Some(l) => l.committed.is_some_and(|c| {
                        c.rev == l.record.rev && acked.as_ref().is_some_and(|vv| vv.includes_id(ID::new(c.peer, c.counter)))
                    }),
                };
                Handoff {
                    connected: id.parse::<EndpointId>().is_ok_and(|k| sessions.contains_key(&k)),
                    has_file: holdings.get(&id).is_some_and(|h| h.contains(&b.hash)),
                    has_place,
                    name: r.name,
                    device: id,
                }
            })
            .collect())
    }

    /// Applies a local change and returns only after it is durable on disk. It is
    /// all-or-nothing: if `f` fails partway or the change can't be written, it is rolled
    /// back, so nothing unsaved lingers in memory to be written or synced later.
    async fn mutate<T: Send + 'static>(&self, f: impl FnOnce(&LoroDoc) -> Result<T> + Send + 'static) -> Result<T> {
        let node = self.clone();
        tokio::task::spawn_blocking(move || {
            let mut st = node.0.state.lock().unwrap();
            let st = &mut *st;
            let before = st.doc.state_frontiers();
            let out = f(&st.doc);
            st.doc.commit();
            let result = out.and_then(|out| {
                st.store.persist(&st.doc).context("Couldn't write to disk, so nothing was changed")?;
                Ok(out)
            });
            if result.is_err() && st.doc.state_frontiers() != before {
                // The undo is itself a change; it reaches the disk (and peers) together with
                // the change it cancels, once writing works again.
                if let Err(e) = st.doc.revert_to(&before) {
                    tracing::error!("could not roll back a failed change: {e}");
                }
                st.doc.commit();
                let _ = st.store.persist(&st.doc);
            }
            node.after_change();
            result
        })
        .await?
    }

    fn after_change(&self) {
        self.0.changed.send_modify(|v| *v += 1);
        let _ = self.0.events.send(Event::LibraryChanged);
        let _ = self.0.events.send(Event::StatusChanged);
    }

    // ---- Devices and status ------------------------------------------------------------

    pub fn status(&self) -> Status {
        let st = self.0.state.lock().unwrap();
        let sessions = self.0.sessions.lock().unwrap();
        let errors = self.0.errors.lock().unwrap();
        let mine = st.doc.oplog_vv();
        let me = self.0.id.to_string();
        let mut devices: Vec<DeviceStatus> = model::read_devices(&st.doc)
            .into_iter()
            .map(|(id, r)| {
                let info = st.peers.get(&id);
                let key: Option<EndpointId> = id.parse().ok();
                DeviceStatus {
                    is_self: id == me,
                    connected: key.is_some_and(|k| sessions.contains_key(&k)),
                    has_all_my_changes: id == me
                        || info
                            .and_then(|i| i.acked.as_ref())
                            .and_then(|b| VersionVector::decode(b).ok())
                            .is_some_and(|vv| vv.includes_vv(&mine)),
                    last_seen: info.and_then(|i| i.last_seen.clone()),
                    last_error: key.and_then(|k| errors.get(&k).cloned()),
                    name: r.name,
                    platform: r.platform,
                    removed: r.removed,
                    id,
                }
            })
            .collect();
        devices.sort_by_key(|d| (!d.is_self, d.removed, d.name.clone()));
        Status {
            device_id: me,
            device_name: st.meta.device_name.clone(),
            platform: self.0.platform.clone(),
            library_id: st.meta.library_id.clone(),
            addresses: self.0.endpoint.addr().ip_addrs().map(|a| a.to_string()).collect(),
            unsaved: !st.store.persisted().includes_vv(&mine),
            devices,
        }
    }

    fn trusted_peers(&self) -> Vec<EndpointId> {
        let st = self.0.state.lock().unwrap();
        let devices = model::read_devices(&st.doc);
        let me = self.0.id.to_string();
        if devices.iter().any(|(id, r)| *id == me && r.removed) {
            return vec![];
        }
        devices.into_iter().filter(|(id, r)| *id != me && !r.removed).filter_map(|(id, _)| id.parse().ok()).collect()
    }

    fn is_trusted(&self, peer: &EndpointId) -> bool {
        self.trusted_peers().contains(peer)
    }

    pub async fn remove_device(&self, id: String) -> Result<()> {
        if id == self.0.id.to_string() {
            bail!("This device can't remove itself.");
        }
        let key: EndpointId = id.parse().context("Unknown device")?;
        self.mutate(move |doc| {
            let (_, mut record) = model::read_devices(doc).into_iter().find(|(k, _)| *k == id).context("Unknown device")?;
            record.removed = true;
            model::write_device(doc, &id, &record)
        })
        .await?;
        if let Some(s) = self.0.sessions.lock().unwrap().remove(&key) {
            s.conn.close(1u32.into(), b"removed");
        }
        let _ = self.0.events.send(Event::StatusChanged);
        Ok(())
    }

    // ---- Pairing -----------------------------------------------------------------------

    /// A pairing code for this device. A set-up device shows a code to join its library; a
    /// device with no paired devices yet shows a code asking to be added, so a phone can
    /// bring in a PC that has no camera (and whose other paired devices may be off).
    pub fn create_invite(&self) -> Result<Invite> {
        let kind = if self.trusted_peers().is_empty() { TicketKind::Add } else { TicketKind::Join };
        let secret: [u8; 32] = rand::random();
        let mut invites = self.0.invites.lock().unwrap();
        invites.retain(|i| i.expires > Instant::now());
        invites.push(PendingInvite { kind, secret, expires: Instant::now() + INVITE_LIFETIME });
        let full = self.0.endpoint.addr();
        let addr = EndpointAddr::from_parts(full.id, full.ip_addrs().filter(|a| usable_lan_addr(a)).map(|a| TransportAddr::Ip(*a)));
        let ticket = Ticket { kind, addr, secret }.encode();
        let expires = std::time::SystemTime::now() + INVITE_LIFETIME;
        Ok(Invite {
            qr_svg: proto::qr_svg(&ticket)?,
            ticket,
            expires_at: humantime::format_rfc3339_seconds(expires).to_string(),
            kind,
        })
    }

    /// Uses a scanned or pasted pairing code: joins the other device's library (`Join`
    /// codes) or adds the other device to this one's library (`Add` codes). Either way,
    /// notes already on both devices are kept and merged.
    pub async fn use_code(&self, ticket: &str) -> Result<PairResult> {
        let ticket = Ticket::decode(ticket)?;
        let other = ticket.addr.id;
        if other == self.0.id {
            bail!("That code came from this device.");
        }
        let unpaired = self.trusted_peers().is_empty();
        if ticket.kind == TicketKind::Join && !unpaired {
            bail!("This device is already connected to a library. Show this device's code on the other device instead.");
        }
        let conn = tokio::time::timeout(PAIR_TIMEOUT, self.0.endpoint.connect(ticket.addr.clone(), PAIR_ALPN))
            .await
            .map_err(|_| anyhow!("Couldn't reach the other device. Make sure both are on the same Wi-Fi network."))?
            .map_err(|e| anyhow!("Couldn't reach the other device: {e}"))?;
        let (mut send, mut recv) = conn.open_bi().await?;
        let (name, library) = {
            let st = self.0.state.lock().unwrap();
            (st.meta.device_name.clone(), st.meta.library_id.clone())
        };
        let platform = self.0.platform.clone();
        let msg = match ticket.kind {
            TicketKind::Join => PairMsg::Request { secret: ticket.secret, name, platform },
            TicketKind::Add => PairMsg::Offer { secret: ticket.secret, library, name, platform },
        };
        proto::write_msg(&mut send, &msg).await?;
        send.finish()?;
        let reply = tokio::time::timeout(PAIR_TIMEOUT, proto::read_msg::<_, PairMsg>(&mut recv))
            .await
            .map_err(|_| anyhow!("The other device didn't answer."))??;
        conn.close(0u32.into(), b"paired");
        let (library, name, platform) = match reply {
            Some(PairMsg::Accepted { library, name, platform }) => (library, name, platform),
            Some(PairMsg::Rejected { reason }) => bail!(reason),
            _ => bail!("The other device gave an unexpected answer."),
        };
        match ticket.kind {
            TicketKind::Join => self.adopt_library(library, other, name.clone(), platform).await?,
            TicketKind::Add => self
                .trust(other, name.clone(), platform)
                .await
                .context("Couldn't save the new device; show its code again and retry")?,
        }
        self.add_peer_addrs(other, ticket.addr.ip_addrs().copied());
        let _ = self.0.events.send(Event::StatusChanged);
        Ok(PairResult { device: name, joined: ticket.kind == TicketKind::Join })
    }

    /// Joins `library` and trusts `peer`. The library id is saved first and the peer
    /// second: if either step fails (or the app dies in between) this device still has
    /// no trusted peers, so pairing can simply be retried with a new code.
    async fn adopt_library(&self, library: String, peer: EndpointId, name: String, platform: String) -> Result<()> {
        {
            let mut st = self.0.state.lock().unwrap();
            let meta = Meta { library_id: library, ..st.meta.clone() };
            write_atomic(&self.0.dir.join("meta.json"), &serde_json::to_vec_pretty(&meta)?)
                .context("Couldn't save the pairing. Nothing was changed; try again.")?;
            st.meta = meta;
        }
        self.trust(peer, name, platform).await.context("Couldn't save the pairing; try again with a new code")
    }

    async fn trust(&self, peer: EndpointId, name: String, platform: String) -> Result<()> {
        let record = DeviceRecord {
            name: clean_name(&name),
            platform: clean_name(&platform),
            added_at: model::now_iso(),
            added_by: self.0.id.to_string(),
            removed: false,
        };
        self.mutate(move |doc| model::write_device(doc, &peer.to_string(), &record)).await
    }

    /// Spends a pending code of `kind`; false if it is unknown, expired, or used.
    fn take_invite(&self, kind: TicketKind, secret: &[u8; 32]) -> bool {
        let mut invites = self.0.invites.lock().unwrap();
        invites.retain(|i| i.expires > Instant::now());
        match invites.iter().position(|i| i.kind == kind && constant_time_eq(&i.secret, secret)) {
            Some(pos) => {
                invites.remove(pos);
                true
            }
            None => false,
        }
    }

    async fn handle_pair(&self, conn: Connection) -> Result<()> {
        let peer = conn.remote_id();
        let (mut send, mut recv) = conn.accept_bi().await?;
        let expired = || PairMsg::Rejected { reason: "This code has expired or was already used.".into() };
        let reply = match proto::read_msg(&mut recv).await? {
            Some(PairMsg::Request { secret, name, platform }) => {
                if self.take_invite(TicketKind::Join, &secret) {
                    self.trust(peer, name, platform).await?;
                    self.accepted()
                } else {
                    expired()
                }
            }
            Some(PairMsg::Offer { secret, library, name, platform }) => {
                if !self.take_invite(TicketKind::Add, &secret) {
                    expired()
                } else if !self.trusted_peers().is_empty() {
                    PairMsg::Rejected { reason: "That device is already connected to a library.".into() }
                } else {
                    match self.adopt_library(library, peer, name, platform).await {
                        Ok(()) => self.accepted(),
                        Err(e) => PairMsg::Rejected { reason: format!("{e:#}") },
                    }
                }
            }
            _ => bail!("malformed pairing request"),
        };
        if matches!(reply, PairMsg::Accepted { .. }) {
            tracing::info!(%peer, "paired");
        }
        proto::write_msg(&mut send, &reply).await?;
        send.finish()?;
        // Wait for the other side to close so the reply is delivered.
        let _ = tokio::time::timeout(Duration::from_secs(5), conn.closed()).await;
        let _ = self.0.events.send(Event::StatusChanged);
        self.0.wake.notify_one();
        Ok(())
    }

    fn accepted(&self) -> PairMsg {
        let st = self.0.state.lock().unwrap();
        PairMsg::Accepted { library: st.meta.library_id.clone(), name: st.meta.device_name.clone(), platform: self.0.platform.clone() }
    }

    // ---- Sync sessions -----------------------------------------------------------------

    fn hello(&self) -> SyncMsg {
        let st = self.0.state.lock().unwrap();
        SyncMsg::Hello { protocol: PROTOCOL_VERSION, library: st.meta.library_id.clone(), vv: st.store.persisted().encode() }
    }

    fn check_hello(&self, msg: Option<SyncMsg>) -> Result<Vec<u8>> {
        match msg {
            Some(SyncMsg::Hello { protocol, library, vv }) => {
                if protocol != PROTOCOL_VERSION {
                    bail!("The other device uses sync version {protocol}; this one uses {PROTOCOL_VERSION}. Update both apps.");
                }
                if library != self.0.state.lock().unwrap().meta.library_id {
                    bail!("The other device belongs to a different library.");
                }
                Ok(vv)
            }
            Some(SyncMsg::Reject { reason }) => bail!(reason),
            _ => bail!("The other device didn't start sync correctly."),
        }
    }

    async fn run_session(&self, conn: Connection, dialer: bool) -> Result<()> {
        let peer = conn.remote_id();
        if !self.is_trusted(&peer) {
            conn.close(403u32.into(), b"not paired");
            bail!("refused connection from unpaired device {peer}");
        }
        let (mut send, mut recv) = if dialer { conn.open_bi().await? } else { conn.accept_bi().await? };
        let their_vv = if dialer {
            proto::write_msg(&mut send, &self.hello()).await?;
            self.check_hello(proto::read_msg(&mut recv).await?)?
        } else {
            match self.check_hello(proto::read_msg(&mut recv).await?) {
                Ok(vv) => {
                    proto::write_msg(&mut send, &self.hello()).await?;
                    vv
                }
                Err(e) => {
                    let _ = proto::write_msg(&mut send, &SyncMsg::Reject { reason: e.to_string() }).await;
                    let _ = send.finish();
                    let _ = tokio::time::timeout(Duration::from_secs(2), conn.closed()).await;
                    return Err(e);
                }
            }
        };

        let _session = self.register(peer, &conn, if dialer { self.0.id } else { peer })?;
        self.0.errors.lock().unwrap().remove(&peer);
        if let Some(info) = self.0.endpoint.remote_info(peer).await {
            let addrs: Vec<SocketAddr> =
                info.addrs().filter_map(|a| if let TransportAddr::Ip(ip) = a.addr() { Some(*ip) } else { None }).collect();
            self.add_peer_addrs(peer, addrs);
        }
        self.record_ack(peer, &their_vv);
        tracing::info!(%peer, dialer, "sync session started");

        let known = Mutex::new(VersionVector::decode(&their_vv)?);
        let (tx, mut rx) = mpsc::channel::<SyncMsg>(32);
        let writer = async {
            while let Some(msg) = rx.recv().await {
                proto::write_msg(&mut send, &msg).await?;
            }
            anyhow::Ok(())
        };
        let pusher = async {
            let mut changed = self.0.changed.subscribe();
            loop {
                if let Some(msg) = self.export_missing(&known)? {
                    tx.send(msg).await?;
                }
                if changed.changed().await.is_err() {
                    return anyhow::Ok(());
                }
            }
        };
        let reader = async {
            loop {
                match proto::read_msg::<_, SyncMsg>(&mut recv).await? {
                    None => return anyhow::Ok(()),
                    Some(SyncMsg::Updates { data, vv }) => {
                        let durable = self.import_remote(data).await?;
                        known.lock().unwrap().merge(&VersionVector::decode(&vv)?);
                        // The sender holds everything it sent us, so this counts as its acknowledgement too.
                        self.record_ack(peer, &vv);
                        tx.send(SyncMsg::Ack { vv: durable }).await?;
                    }
                    Some(SyncMsg::Ack { vv }) => {
                        known.lock().unwrap().merge(&VersionVector::decode(&vv)?);
                        self.record_ack(peer, &vv);
                    }
                    Some(SyncMsg::Reject { reason }) => bail!(reason),
                    Some(SyncMsg::Hello { .. }) => {}
                }
            }
        };
        let mut closing = self.0.closing.subscribe();
        let result = tokio::select! {
            r = writer => r,
            r = pusher => r,
            r = reader => r,
            _ = conn.closed() => Ok(()),
            _ = closing.wait_for(|c| *c) => Ok(()),
        };
        tracing::info!(%peer, ?result, "sync session ended");
        result
    }

    /// Updates the peer lacks, judged by what it has told us or we have sent.
    /// Only durable changes are sent: while a local change failed to reach the disk,
    /// nothing goes out, so no peer can report holding a change this device could lose.
    fn export_missing(&self, known: &Mutex<VersionVector>) -> Result<Option<SyncMsg>> {
        let st = self.0.state.lock().unwrap();
        let ours = st.doc.oplog_vv();
        if !st.store.persisted().includes_vv(&ours) {
            return Ok(None);
        }
        let mut known = known.lock().unwrap();
        if known.includes_vv(&ours) {
            return Ok(None);
        }
        let data = st.doc.export(loro::ExportMode::updates(&known))?;
        known.merge(&ours);
        Ok(Some(SyncMsg::Updates { data, vv: ours.encode() }))
    }

    /// Imports remote updates and returns our durable version once they are on disk.
    async fn import_remote(&self, data: Vec<u8>) -> Result<Vec<u8>> {
        let node = self.clone();
        let (durable, changed) = tokio::task::spawn_blocking(move || {
            let mut st = node.0.state.lock().unwrap();
            let st = &mut *st;
            let status = st.doc.import(&data).context("received changes could not be applied")?;
            if status.pending.is_some() {
                tracing::warn!("received changes with missing history; they will be re-requested");
            }
            let changed = st.store.persist(&st.doc)?;
            anyhow::Ok((st.store.persisted().encode(), changed))
        })
        .await??;
        if changed {
            self.after_change();
            self.drop_untrusted_sessions();
            // Received changes may carry new device addresses; retry unreachable peers now.
            self.0.backoff.lock().unwrap().clear();
            self.0.wake.notify_one();
        }
        Ok(durable)
    }

    fn drop_untrusted_sessions(&self) {
        let trusted: HashSet<EndpointId> = self.trusted_peers().into_iter().collect();
        self.0.sessions.lock().unwrap().retain(|peer, s| {
            let keep = trusted.contains(peer);
            if !keep {
                s.conn.close(1u32.into(), b"removed");
            }
            keep
        });
    }

    fn record_ack(&self, peer: EndpointId, vv: &[u8]) {
        let Ok(vv) = VersionVector::decode(vv) else { return };
        let mut st = self.0.state.lock().unwrap();
        let info = st.peers.entry(peer.to_string()).or_default();
        let mut merged = info.acked.as_ref().and_then(|b| VersionVector::decode(b).ok()).unwrap_or_default();
        if merged.includes_vv(&vv) {
            return;
        }
        merged.merge(&vv);
        info.acked = Some(merged.encode());
        info.last_seen = Some(model::now_iso());
        self.save_peers(&st);
        drop(st);
        let _ = self.0.events.send(Event::StatusChanged);
    }

    fn save_peers(&self, st: &State) {
        if let Err(e) = serde_json::to_vec_pretty(&st.peers).map_err(anyhow::Error::from).and_then(|b| write_atomic(&self.0.dir.join("peers.json"), &b)) {
            tracing::warn!("could not save peers.json: {e:#}");
        }
    }

    /// Keeps one connection per peer: when both sides dial at once, both keep the
    /// connection dialed by the smaller endpoint id.
    fn register(&self, peer: EndpointId, conn: &Connection, dialer: EndpointId) -> Result<SessionGuard> {
        let mut sessions = self.0.sessions.lock().unwrap();
        if let Some(existing) = sessions.get(&peer) {
            let preferred = self.0.id.min(peer);
            if existing.dialer != dialer && dialer != preferred {
                conn.close(0u32.into(), b"duplicate");
                bail!("duplicate connection");
            }
            existing.conn.close(0u32.into(), b"replaced");
        }
        let token = self.0.next_token.fetch_add(1, Ordering::Relaxed);
        sessions.insert(peer, Session { conn: conn.clone(), dialer, token });
        drop(sessions);
        let _ = self.0.events.send(Event::StatusChanged);
        Ok(SessionGuard { node: self.clone(), peer, token })
    }

    // ---- Connection management ---------------------------------------------------------

    /// Retries writing changes that failed to reach the disk; once they do, they sync.
    async fn retry_unsaved(&self) {
        let node = self.clone();
        let wrote = tokio::task::spawn_blocking(move || {
            let mut st = node.0.state.lock().unwrap();
            let st = &mut *st;
            if st.store.persisted().includes_vv(&st.doc.oplog_vv()) {
                return Ok(false);
            }
            st.store.persist(&st.doc)
        })
        .await;
        match wrote {
            Ok(Ok(true)) => {
                tracing::info!("unsaved changes are now on disk");
                self.after_change();
            }
            Ok(Err(e)) => tracing::debug!("still can't save: {e:#}"),
            _ => {}
        }
    }

    /// Shares this device's current addresses through the document, so peers can find
    /// it again after its address or port changes even without local discovery.
    async fn publish_addresses(&self) {
        let addrs: Vec<SocketAddr> = self.0.endpoint.addr().ip_addrs().filter(|a| usable_lan_addr(a)).copied().collect();
        if addrs.is_empty() {
            return;
        }
        let me = self.0.id.to_string();
        let unchanged = model::read_addresses(&self.0.state.lock().unwrap().doc, &me) == {
            let mut sorted = addrs.clone();
            sorted.sort();
            sorted
        };
        if !unchanged {
            if let Err(e) = self.mutate(move |doc| model::write_addresses(doc, &me, &addrs)).await {
                tracing::warn!("could not publish addresses: {e:#}");
            }
        }
    }

    async fn dial_loop(self) {
        let mut closing = self.0.closing.subscribe();
        let mut last_publish = Instant::now();
        loop {
            self.retry_unsaved().await;
            if last_publish.elapsed() > Duration::from_secs(30) {
                last_publish = Instant::now();
                self.publish_addresses().await;
            }
            for peer in self.trusted_peers() {
                if self.0.sessions.lock().unwrap().contains_key(&peer) {
                    continue;
                }
                if self.0.backoff.lock().unwrap().get(&peer).is_some_and(|(at, _)| *at > Instant::now()) {
                    continue;
                }
                if !self.0.dialing.lock().unwrap().insert(peer) {
                    continue;
                }
                tokio::spawn(self.clone().dial(peer));
            }
            tokio::select! {
                _ = tokio::time::sleep(DIAL_INTERVAL) => {}
                _ = self.0.wake.notified() => {}
                _ = closing.wait_for(|c| *c) => return,
            }
        }
    }

    async fn dial(self, peer: EndpointId) {
        let addrs: Vec<SocketAddr> = {
            let st = self.0.state.lock().unwrap();
            let mut addrs = st.peers.get(&peer.to_string()).map(|p| p.addrs.clone()).unwrap_or_default();
            for a in model::read_addresses(&st.doc, &peer.to_string()) {
                if !addrs.contains(&a) {
                    addrs.push(a);
                }
            }
            addrs
        };
        let addr = EndpointAddr::from_parts(peer, addrs.into_iter().map(TransportAddr::Ip));
        let result = match tokio::time::timeout(DIAL_TIMEOUT, self.0.endpoint.connect(addr, SYNC_ALPN)).await {
            Ok(Ok(conn)) => self.run_session(conn, true).await,
            Ok(Err(e)) => Err(anyhow!("couldn't connect: {e}")),
            Err(_) => Err(anyhow!("couldn't connect: timed out")),
        };
        let mut backoff = self.0.backoff.lock().unwrap();
        match result {
            Ok(()) => {
                backoff.insert(peer, (Instant::now() + Duration::from_secs(1), Duration::from_secs(1)));
            }
            Err(e) => {
                let message = format!("{e:#}");
                if message != "duplicate connection" {
                    tracing::debug!(%peer, "dial failed: {message}");
                    let delay = backoff.get(&peer).map_or(Duration::from_secs(2), |(_, d)| (*d * 2).min(MAX_BACKOFF));
                    backoff.insert(peer, (Instant::now() + delay, delay));
                    self.0.errors.lock().unwrap().insert(peer, message);
                    let _ = self.0.events.send(Event::StatusChanged);
                }
            }
        }
        drop(backoff);
        self.0.dialing.lock().unwrap().remove(&peer);
    }

    async fn mdns_loop(self) {
        let Some(mdns) = self.0.mdns.clone() else { return };
        let mut events = mdns.subscribe().await;
        let mut closing = self.0.closing.subscribe();
        loop {
            let event = tokio::select! {
                e = events.next() => e,
                _ = closing.wait_for(|c| *c) => return,
            };
            match event {
                Some(DiscoveryEvent::Discovered { endpoint_info, .. }) => {
                    let addr = endpoint_info.into_endpoint_addr();
                    if self.is_trusted(&addr.id) {
                        tracing::debug!(peer = %addr.id, "discovered on local network");
                        self.add_peer_addrs(addr.id, addr.ip_addrs().copied());
                    }
                }
                Some(_) => {}
                None => return,
            }
        }
    }
}

struct SessionGuard {
    node: Node,
    peer: EndpointId,
    token: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        let mut sessions = self.node.0.sessions.lock().unwrap();
        if sessions.get(&self.peer).is_some_and(|s| s.token == self.token) {
            sessions.remove(&self.peer);
        }
        drop(sessions);
        if let Ok(mut st) = self.node.0.state.lock() {
            st.peers.entry(self.peer.to_string()).or_default().last_seen = Some(model::now_iso());
            self.node.save_peers(&st);
        }
        let _ = self.node.0.events.send(Event::StatusChanged);
        self.node.0.wake.notify_one();
    }
}

#[derive(Debug, Clone)]
struct SyncHandler(Node);

impl ProtocolHandler for SyncHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if let Err(e) = self.0.run_session(connection, false).await {
            tracing::debug!("incoming sync session: {e:#}");
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct PairHandler(Node);

impl ProtocolHandler for PairHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        if let Err(e) = self.0.handle_pair(connection).await {
            tracing::warn!("pairing failed: {e:#}");
        }
        Ok(())
    }
}

async fn bind_endpoint(key: &SecretKey, port: u16) -> Result<Endpoint> {
    // Notice a vanished peer (sleep, Wi-Fi loss, crash) within seconds rather than the
    // QUIC default, so the device list and "received by" status stay honest.
    let transport = || {
        QuicTransportConfig::builder()
            .keep_alive_interval(Duration::from_secs(2))
            .max_idle_timeout(Some(IdleTimeout::try_from(Duration::from_secs(8)).expect("valid timeout")))
            .build()
    };
    let builder = || {
        Endpoint::builder(presets::Minimal)
            .secret_key(key.clone())
            .relay_mode(RelayMode::Disabled)
            .transport_config(transport())
    };
    if port != 0 {
        match builder().bind_addr(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port))?.bind().await {
            Ok(e) => return Ok(e),
            Err(e) => tracing::warn!("port {port} unavailable ({e}); using any free port"),
        }
    }
    builder().bind().await.map_err(|e| anyhow!("could not open a network socket: {e}"))
}

/// Takes an exclusive advisory lock without blocking; `Ok(false)` means another process
/// holds it. (std's `File::try_lock` is unsupported on Android, so use `flock` on Unix.)
fn try_lock(file: &std::fs::File) -> std::io::Result<bool> {
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // SAFETY: flock on a valid, open file descriptor owned by `file`.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            return Ok(true);
        }
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::EWOULDBLOCK) { Ok(false) } else { Err(error) }
    }
    #[cfg(not(unix))]
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(std::fs::TryLockError::WouldBlock) => Ok(false),
        Err(std::fs::TryLockError::Error(e)) => Err(e),
    }
}

fn load_or_create_key(path: &Path) -> Result<SecretKey> {
    match std::fs::read(path) {
        Ok(bytes) => {
            let bytes: [u8; 32] = bytes.try_into().map_err(|_| anyhow!("device.key is damaged"))?;
            Ok(SecretKey::from_bytes(&bytes))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            let key = SecretKey::generate();
            write_atomic(path, &key.to_bytes())?;
            Ok(key)
        }
        Err(e) => Err(e).context("reading device.key"),
    }
}

/// Addresses worth putting in an invitation: skip loopback and link-local.
fn usable_lan_addr(a: &SocketAddr) -> bool {
    match a {
        SocketAddr::V4(v4) => !v4.ip().is_loopback() && !v4.ip().is_link_local(),
        SocketAddr::V6(v6) => !v6.ip().is_loopback() && !v6.ip().is_unicast_link_local(),
    }
}

fn clean_name(name: &str) -> String {
    let name: String = name.trim().chars().filter(|c| !c.is_control()).take(64).collect();
    if name.is_empty() { "Unnamed device".into() } else { name }
}

const LOCAL_POSITIONS: &str = "positions.local.json";

/// Loads this device's local positions, never letting the revision counter fall behind a
/// revision already in the replicated document (e.g. if the file was lost).
fn load_local_positions(dir: &Path, doc: &LoroDoc, me: &str) -> Result<LocalPositions> {
    let mut local: LocalPositions = match std::fs::read(dir.join(LOCAL_POSITIONS)) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("positions.local.json is damaged")?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => LocalPositions::default(),
        Err(e) => return Err(e).context("reading positions.local.json"),
    };
    for book in model::list_books(doc) {
        if let Some(own) = model::read_positions(doc, &book.id).remove(me) {
            local.counter = local.counter.max(own.rev);
            if local.books.get(&book.id).is_none_or(|l| l.record.rev < own.rev) {
                // Unknown which operation committed it: re-committed once on startup.
                local.books.insert(book.id.clone(), LocalPosition { record: own, committed: None });
            }
        }
    }
    Ok(local)
}

fn location_of(record: &PositionRecord) -> Location {
    Location { locator: record.locator.clone(), fraction: record.fraction, snippet: record.snippet.clone() }
}

fn constant_time_eq(a: &[u8; 32], b: &[u8; 32]) -> bool {
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
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
        let deadline = Instant::now() + Duration::from_secs(20);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    fn fail_writes(node: &Node, fail: bool) {
        node.0.state.lock().unwrap().store.fail_writes = fail;
    }

    fn has(node: &Node, title: &str) -> bool {
        node.books().iter().any(|b| b.book.title == title)
    }

    fn titled(seed: u8, title: &str) -> NewBook {
        NewBook { title: title.into(), ..new_book(seed) }
    }

    fn connected(a: &Node, b: &Node) -> bool {
        a.0.sessions.lock().unwrap().contains_key(&b.id())
    }

    fn at(cfi: &str) -> Location {
        Location { locator: serde_json::json!({ "cfi": cfi }), fraction: 0.25, snippet: cfi.into() }
    }

    fn cfi(p: &Option<Placed>) -> String {
        p.as_ref().map(|p| p.record.locator["cfi"].as_str().unwrap().to_owned()).unwrap_or_default()
    }

    fn has_place(node: &Node, book: &str, device: &str) -> bool {
        node.handoff(book).unwrap().iter().any(|h| h.name == device && h.has_place)
    }

    async fn pair(a: &Node, b: &Node) {
        b.use_code(&a.create_invite().unwrap().ticket).await.unwrap();
        wait_for("connected", || connected(a, b) && connected(b, a)).await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn unsaved_changes_are_never_advertised() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "A").await;
        let b = open(db.path(), "B").await;
        pair(&a, &b).await;

        fail_writes(&a, true);
        assert!(a.add_book(titled(1, "not on disk")).await.is_err());
        assert!(!has(&a, "not on disk"), "a failed save is rolled back");
        // Another change arriving meanwhile must not be acknowledged either.
        b.add_book(titled(2, "from B")).await.unwrap();
        tokio::time::sleep(Duration::from_secs(2)).await;
        assert!(!has(&b, "not on disk"), "an unsaved change was sent");
        assert!(!a.status().devices.iter().any(|d| d.name == "B" && d.has_all_my_changes));
        let b_view = b.status();
        assert!(!b_view.devices.iter().any(|d| d.name == "A" && d.has_all_my_changes), "B must not think A durably has B's book");

        // Once the disk recovers, the retry writes the change and its undo, then syncs.
        fail_writes(&a, false);
        wait_for("A caught up with B", || has(&a, "from B")).await;
        wait_for("A reports saved", || !a.status().unsaved).await;
        wait_for("B has all of A's changes", || a.status().devices.iter().any(|d| d.name == "B" && d.has_all_my_changes)).await;
        assert!(!has(&b, "not on disk"), "the rolled-back book never appears elsewhere");
        a.shutdown().await;
        drop(a);
        let a = open(da.path(), "A").await;
        assert!(has(&a, "from B") && !has(&a, "not on disk"));
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn failed_pairing_can_be_retried() {
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        let a = open(dirs[0].path(), "A").await;
        let b = open(dirs[1].path(), "B").await;

        // "Add" code: A (unpaired) shows it, B scans; A adopts B's library. Fail A's writes.
        let library_before = a.status().library_id;
        std::fs::create_dir(dirs[0].path().join("meta.json.tmp")).unwrap();
        assert!(b.use_code(&a.create_invite().unwrap().ticket).await.is_err());
        assert!(a.trusted_peers().is_empty() && b.trusted_peers().is_empty());
        assert_eq!(a.status().library_id, library_before);
        std::fs::remove_dir(dirs[0].path().join("meta.json.tmp")).unwrap();

        fail_writes(&a, true);
        assert!(b.use_code(&a.create_invite().unwrap().ticket).await.is_err());
        assert!(a.trusted_peers().is_empty(), "the failed trust was rolled back");
        fail_writes(&a, false);
        a.shutdown().await;
        drop(a);
        let a = open(dirs[0].path(), "A").await;
        assert!(a.trusted_peers().is_empty(), "a half-finished pairing must not count as paired");
        let result = b.use_code(&a.create_invite().unwrap().ticket).await.unwrap();
        assert!(!result.joined && result.device == "A");
        assert_eq!(b.status().library_id, a.status().library_id);

        // "Join" code: A is paired now, so its code lets C join. Fail C's writes.
        let c = open(dirs[2].path(), "C").await;
        std::fs::create_dir(dirs[2].path().join("meta.json.tmp")).unwrap();
        assert!(c.use_code(&a.create_invite().unwrap().ticket).await.is_err());
        assert!(c.trusted_peers().is_empty());
        std::fs::remove_dir(dirs[2].path().join("meta.json.tmp")).unwrap();
        fail_writes(&c, true);
        assert!(c.use_code(&a.create_invite().unwrap().ticket).await.is_err());
        assert!(c.trusted_peers().is_empty(), "the failed trust was rolled back");
        fail_writes(&c, false);
        let result = c.use_code(&a.create_invite().unwrap().ticket).await.unwrap();
        assert!(result.joined && result.device == "A");

        b.add_book(titled(3, "after retry")).await.unwrap();
        wait_for("sync after retried pairings", || has(&a, "after retry") && has(&c, "after retry")).await;
        for n in [a, b, c] {
            n.shutdown().await;
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_checkpoint_never_committed_is_recovered_on_restart() {
        let dir = tempfile::tempdir().unwrap();
        let node = open(dir.path(), "Linux").await;
        let (book, _) = node.add_book(new_book(1)).await.unwrap();
        node.flush_position(book.id.clone(), at("committed")).await.unwrap();
        let rev = node.checkpoint(book.id.clone(), at("only-local")).await.unwrap();
        // Crash: the checkpoint never reached the replicated document.
        node.shutdown().await;
        drop(node);
        let node = open(dir.path(), "Linux").await;
        let me = node.id().to_string();
        let replicated = model::read_positions(&node.0.state.lock().unwrap().doc, &book.id);
        assert_eq!(replicated[&me].rev, rev, "recovered on startup, before anything else");
        let sel = node.open_book(book.id.clone()).await.unwrap();
        assert_eq!((sel.decision, cfi(&sel.position)), (Decision::Mine, "only-local".into()));
        // The counter survives the restart: no revision is reused.
        assert!(node.checkpoint(book.id.clone(), at("next")).await.unwrap() > rev);
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_lost_local_file_never_reuses_revisions() {
        let dir = tempfile::tempdir().unwrap();
        let node = open(dir.path(), "Linux").await;
        let (book, _) = node.add_book(new_book(1)).await.unwrap();
        let rev = node.flush_position(book.id.clone(), at("x")).await.unwrap();
        node.shutdown().await;
        drop(node);
        std::fs::remove_file(dir.path().join(LOCAL_POSITIONS)).unwrap();
        let node = open(dir.path(), "Linux").await;
        assert!(node.checkpoint(book.id.clone(), at("y")).await.unwrap() > rev);
        assert_eq!(cfi(&node.peek_position(&book.id).unwrap().position), "y");
        node.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn handoff_tracks_the_exact_latest_position() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        pair(&a, &b).await;
        let (book, _) = a.add_book(new_book(1)).await.unwrap();
        a.flush_position(book.id.clone(), at("p1")).await.unwrap();
        wait_for("phone has the place", || has_place(&a, &book.id, "Phone")).await;
        // A newer checkpoint turns it off at once, even though it isn't replicated yet.
        a.checkpoint(book.id.clone(), at("p2")).await.unwrap();
        assert!(!has_place(&a, &book.id, "Phone"));
        // Other changes being acknowledged don't count for the position.
        a.add_book(new_book(2)).await.unwrap();
        wait_for("phone has everything committed", || a.status().devices.iter().any(|d| d.name == "Phone" && d.has_all_my_changes)).await;
        assert!(!has_place(&a, &book.id, "Phone"));
        a.commit_position(book.id.clone()).await.unwrap();
        wait_for("phone has the newer place", || has_place(&a, &book.id, "Phone")).await;
        assert_eq!(cfi(&b.peek_position(&book.id).unwrap().position), "p2");
        assert!(a.handoff(&book.id).unwrap().iter().all(|h| !h.has_file), "files arrive with the file store");
        a.shutdown().await;
        b.shutdown().await;
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_choice_is_saved_without_moving_and_survives_an_immediate_crash() {
        let (da, db) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let a = open(da.path(), "Linux").await;
        let b = open(db.path(), "Phone").await;
        pair(&a, &b).await;
        let (book, _) = a.add_book(new_book(1)).await.unwrap();
        a.flush_position(book.id.clone(), at("start")).await.unwrap();
        wait_for("phone has it", || has_place(&a, &book.id, "Phone")).await;
        let sel = b.open_book(book.id.clone()).await.unwrap();
        assert_eq!((sel.decision, cfi(&sel.position)), (Decision::Continued, "start".into()));

        // Both read apart; the phone's newer position arrives while Linux is still reading.
        a.0.sessions.lock().unwrap().values().for_each(|s| s.conn.close(0u32.into(), b"test"));
        a.flush_position(book.id.clone(), at("linux-apart")).await.unwrap();
        b.flush_position(book.id.clone(), at("phone-apart")).await.unwrap();
        wait_for("both positions on both", || {
            [&a, &b].iter().all(|n| n.peek_position(&book.id).unwrap().decision == Decision::Divergent)
        })
        .await;
        let sel = a.peek_position(&book.id).unwrap();
        let shown: Vec<RevRef> = sel.heads.iter().map(|h| RevRef { device: h.device.clone(), rev: h.record.rev }).collect();
        // "Stay here", then the app dies right away.
        a.resolve_position(book.id.clone(), shown, Choice::Here(at("linux-apart"))).await.unwrap();
        a.shutdown().await;
        drop(a);
        let a = open(da.path(), "Linux").await;
        assert_eq!(a.peek_position(&book.id).unwrap().decision, Decision::Mine);
        wait_for("the phone continues from Linux's choice", || {
            let sel = b.peek_position(&book.id).unwrap();
            sel.decision == Decision::Continued && cfi(&sel.position) == "linux-apart"
        })
        .await;
        a.shutdown().await;
        b.shutdown().await;
    }
}
