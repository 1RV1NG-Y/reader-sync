//! How the reader's data is laid out inside the replicated Loro document (PLAN.md §3).
//!
//! - `books`: book id → mergeable map. The id is `<blake3 hex>-<incarnation>`, so devices that
//!   import the same file independently get the same container and their changes merge.
//!   - `hash`, `incarnation`, `size`, `format`, `originalName`: set at import; identical on
//!     every device that imports the same file.
//!   - `title`, `author`: last-writer-wins.
//!   - `addedAt`, `addedBy`.
//!   - `trashed`, `trashedAt`: last-writer-wins (Trash is recoverable).
//!   - `purged`, `purgedAt`, `purgedBy`: "delete forever". One-way: nothing ever writes
//!     `purged = false`, and every write to a purged book is refused, so a device that hadn't
//!     seen the purge can't bring the book back. Importing the file again afterwards creates
//!     the next incarnation, a new book.
//! - `positions`: book id → mergeable map of device id → JSON [`PositionRecord`]. Each device
//!   writes only its own key, so positions never conflict; see `positions.rs` for how one is
//!   chosen.
//! - `holdings`: device id → mergeable map of file hash → JSON [`Holding`]: the files that
//!   device has complete and verified. Written only by that device.
//! - `removedDownloads`: device id → mergeable map of book id → time ("remove download from
//!   this device"). Written only by that device; opening the book again deletes the key.
//! - `settings`: setting name → JSON value, last-writer-wins.
//! - `devices` holds membership; `addresses` each device's current LAN addresses.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use loro::{LoroDoc, LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::positions::PositionRecord;

pub const BOOKS: &str = "books";
pub const POSITIONS: &str = "positions";
pub const HOLDINGS: &str = "holdings";
pub const REMOVED_DOWNLOADS: &str = "removedDownloads";
pub const SETTINGS: &str = "settings";
pub const DEVICES: &str = "devices";
/// Map of endpoint id → JSON list of socket addresses, written only by that device.
pub const ADDRESSES: &str = "addresses";

/// Largest book file accepted. Generous for books and PDFs; it bounds what a peer can make
/// another device download.
pub const MAX_BOOK_SIZE: u64 = 2 * 1024 * 1024 * 1024;
const MAX_SETTINGS: usize = 100;
const MAX_SETTING_JSON: usize = 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    pub id: String,
    pub hash: String,
    pub incarnation: u32,
    pub size: u64,
    pub format: String,
    pub original_name: String,
    pub title: String,
    pub author: String,
    pub added_at: String,
    pub added_by: String,
    pub trashed: bool,
    pub trashed_at: Option<String>,
    pub purged: bool,
    pub purged_at: Option<String>,
    pub purged_by: Option<String>,
}

/// A file being added to the library. The front end reads title and author from the book.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewBook {
    pub hash: String,
    pub size: u64,
    pub format: String,
    pub original_name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub author: String,
}

/// Fields a caller may change. `None` leaves the field as it is.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BookPatch {
    pub title: Option<String>,
    pub author: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Holding {
    pub verified_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceRecord {
    pub name: String,
    pub platform: String,
    pub added_at: String,
    pub added_by: String,
    #[serde(default)]
    pub removed: bool,
}

pub fn now_iso() -> String {
    humantime::format_rfc3339_millis(std::time::SystemTime::now()).to_string()
}

fn json_value<T: for<'de> Deserialize<'de>>(map: &LoroMap, key: &str) -> Option<T> {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => serde_json::from_str(&s).ok(),
        _ => None,
    }
}

fn str_field(map: &LoroMap, key: &str) -> String {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => s.to_string(),
        _ => String::new(),
    }
}

fn opt_str_field(map: &LoroMap, key: &str) -> Option<String> {
    Some(str_field(map, key)).filter(|s| !s.is_empty())
}

fn bool_field(map: &LoroMap, key: &str) -> bool {
    matches!(map.get(key), Some(ValueOrContainer::Value(LoroValue::Bool(true))))
}

fn i64_field(map: &LoroMap, key: &str) -> i64 {
    match map.get(key) {
        Some(ValueOrContainer::Value(LoroValue::I64(n))) => n,
        _ => 0,
    }
}

fn child_map(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key) {
        Some(ValueOrContainer::Container(c)) => c.into_map().ok(),
        _ => None,
    }
}

pub fn book_id(hash: &str, incarnation: u32) -> String {
    format!("{hash}-{incarnation}")
}

fn book_map(doc: &LoroDoc, id: &str) -> Option<LoroMap> {
    child_map(&doc.get_map(BOOKS), id)
}

pub fn read_book(doc: &LoroDoc, id: &str) -> Option<Book> {
    let m = book_map(doc, id)?;
    let hash = str_field(&m, "hash");
    if hash.is_empty() {
        return None;
    }
    Some(Book {
        id: id.to_owned(),
        hash,
        incarnation: i64_field(&m, "incarnation") as u32,
        size: i64_field(&m, "size") as u64,
        format: str_field(&m, "format"),
        original_name: str_field(&m, "originalName"),
        title: str_field(&m, "title"),
        author: str_field(&m, "author"),
        added_at: str_field(&m, "addedAt"),
        added_by: str_field(&m, "addedBy"),
        trashed: bool_field(&m, "trashed"),
        trashed_at: opt_str_field(&m, "trashedAt"),
        purged: bool_field(&m, "purged"),
        purged_at: opt_str_field(&m, "purgedAt"),
        purged_by: opt_str_field(&m, "purgedBy"),
    })
}

/// Every book, including purged ones, newest first.
pub fn list_books(doc: &LoroDoc) -> Vec<Book> {
    let books = doc.get_map(BOOKS);
    let mut out: Vec<Book> = books.keys().filter_map(|k| read_book(doc, &k)).collect();
    out.sort_by(|a, b| b.added_at.cmp(&a.added_at).then_with(|| a.id.cmp(&b.id)));
    out
}

/// A book that exists and isn't purged, for operations that change it.
fn live_book(doc: &LoroDoc, id: &str) -> Result<(Book, LoroMap)> {
    let m = book_map(doc, id).context("That book isn't in the library.")?;
    let book = read_book(doc, id).context("That book isn't in the library.")?;
    if book.purged {
        bail!("“{}” was deleted forever.", display_title(&book));
    }
    Ok((book, m))
}

fn display_title(book: &Book) -> &str {
    if book.title.is_empty() { &book.original_name } else { &book.title }
}

fn validate_hash(hash: &str) -> Result<()> {
    if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)) {
        bail!("Invalid file hash.");
    }
    Ok(())
}

fn validate_text(what: &str, value: &str, max: usize) -> Result<()> {
    if value.chars().count() > max || value.chars().any(|c| c.is_control()) {
        bail!("The {what} is too long or contains invalid characters.");
    }
    Ok(())
}

/// Formats become file extensions, so only short lowercase alphanumerics are accepted.
pub fn validate_format(format: &str) -> Result<()> {
    if format.is_empty() || format.len() > 10 || format == "part" || !format.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()) {
        bail!("Unknown book format.");
    }
    Ok(())
}

fn validate_new(book: &NewBook) -> Result<()> {
    validate_hash(&book.hash)?;
    if book.size == 0 || book.size > MAX_BOOK_SIZE {
        bail!("Book files must be between 1 byte and {} GB.", MAX_BOOK_SIZE >> 30);
    }
    validate_format(&book.format)?;
    validate_text("file name", &book.original_name, 255)?;
    validate_text("title", &book.title, 500)?;
    validate_text("author", &book.author, 500)?;
    Ok(())
}

/// Adds a book, or finds it if the same file is already in the library. When every earlier
/// incarnation of this file was deleted forever, this is a deliberate re-import and creates
/// the next incarnation. Returns the book id and whether a book was added. The caller commits.
pub fn add_book(doc: &LoroDoc, new: &NewBook, by: &str) -> Result<(String, bool)> {
    validate_new(new)?;
    let books = doc.get_map(BOOKS);
    let prefix = format!("{}-", new.hash);
    let latest = books
        .keys()
        .filter(|k| k.starts_with(&prefix))
        .filter_map(|k| read_book(doc, &k))
        .max_by_key(|b| b.incarnation);
    let incarnation = match latest {
        Some(b) if !b.purged => return Ok((b.id, false)),
        Some(b) => b.incarnation + 1,
        None => 1,
    };
    let id = book_id(&new.hash, incarnation);
    let m = books.ensure_mergeable_map(&id)?;
    m.insert("hash", new.hash.as_str())?;
    m.insert("incarnation", incarnation as i64)?;
    m.insert("size", new.size as i64)?;
    m.insert("format", new.format.as_str())?;
    m.insert("originalName", new.original_name.as_str())?;
    m.insert("title", new.title.trim())?;
    m.insert("author", new.author.trim())?;
    m.insert("addedAt", now_iso())?;
    m.insert("addedBy", by)?;
    m.insert("trashed", false)?;
    Ok((id, true))
}

/// Returns whether anything changed. The caller commits.
pub fn update_book(doc: &LoroDoc, id: &str, patch: &BookPatch) -> Result<bool> {
    let (book, m) = live_book(doc, id)?;
    let mut changed = false;
    for (key, value, current) in [("title", &patch.title, &book.title), ("author", &patch.author, &book.author)] {
        if let Some(value) = value {
            validate_text(key, value, 500)?;
            if value.trim() != current {
                m.insert(key, value.trim())?;
                changed = true;
            }
        }
    }
    Ok(changed)
}

/// Moves a book to Trash or back. The caller commits.
pub fn set_trashed(doc: &LoroDoc, id: &str, trashed: bool) -> Result<bool> {
    let (book, m) = live_book(doc, id)?;
    if book.trashed == trashed {
        return Ok(false);
    }
    m.insert("trashed", trashed)?;
    m.insert("trashedAt", if trashed { now_iso() } else { String::new() })?;
    Ok(true)
}

/// "Delete forever": only for books in Trash. The caller commits.
pub fn purge(doc: &LoroDoc, id: &str, by: &str) -> Result<bool> {
    let m = book_map(doc, id).context("That book isn't in the library.")?;
    let book = read_book(doc, id).context("That book isn't in the library.")?;
    if book.purged {
        return Ok(false);
    }
    if !book.trashed {
        bail!("Move “{}” to Trash before deleting it forever.", display_title(&book));
    }
    m.insert("purged", true)?;
    m.insert("purgedAt", now_iso())?;
    m.insert("purgedBy", by)?;
    Ok(true)
}

// ---- Positions ---------------------------------------------------------------------------

/// Every device's latest replicated position for a book, by device id.
pub fn read_positions(doc: &LoroDoc, book: &str) -> BTreeMap<String, PositionRecord> {
    let Some(m) = child_map(&doc.get_map(POSITIONS), book) else { return BTreeMap::new() };
    m.keys().filter_map(|k| json_value::<PositionRecord>(&m, &k).map(|r| (k.to_string(), r))).collect()
}

/// Writes this device's position for a book. The caller commits.
pub fn write_position(doc: &LoroDoc, book: &str, me: &str, record: &PositionRecord) -> Result<()> {
    live_book(doc, book)?;
    record.validate()?;
    let m = doc.get_map(POSITIONS).ensure_mergeable_map(book)?;
    m.insert(me, serde_json::to_string(record)?)?;
    Ok(())
}

// ---- Holdings and removed downloads --------------------------------------------------------

/// File hashes each device holds complete and verified.
pub fn read_holdings(doc: &LoroDoc) -> BTreeMap<String, BTreeSet<String>> {
    let root = doc.get_map(HOLDINGS);
    root.keys()
        .filter_map(|device| child_map(&root, &device).map(|m| (device.to_string(), m.keys().map(|k| k.to_string()).collect())))
        .collect()
}

/// Records (or with `None`, withdraws) this device's verified copy of a file. The caller commits.
pub fn write_holding(doc: &LoroDoc, me: &str, hash: &str, holding: Option<&Holding>) -> Result<bool> {
    validate_hash(hash)?;
    let m = doc.get_map(HOLDINGS).ensure_mergeable_map(me)?;
    match holding {
        Some(h) => m.insert(hash, serde_json::to_string(h)?)?,
        None if m.get(hash).is_some() => m.delete(hash)?,
        None => return Ok(false),
    }
    Ok(true)
}

/// Books each device has chosen not to keep a download of.
pub fn read_removed_downloads(doc: &LoroDoc, device: &str) -> BTreeSet<String> {
    child_map(&doc.get_map(REMOVED_DOWNLOADS), device).map(|m| m.keys().map(|k| k.to_string()).collect()).unwrap_or_default()
}

/// The caller commits.
pub fn set_download_removed(doc: &LoroDoc, me: &str, book: &str, removed: bool) -> Result<bool> {
    let m = doc.get_map(REMOVED_DOWNLOADS).ensure_mergeable_map(me)?;
    match (removed, m.get(book).is_some()) {
        (true, false) => m.insert(book, now_iso())?,
        (false, true) => m.delete(book)?,
        _ => return Ok(false),
    }
    Ok(true)
}

// ---- Settings ------------------------------------------------------------------------------

pub fn read_settings(doc: &LoroDoc) -> serde_json::Map<String, Value> {
    let m = doc.get_map(SETTINGS);
    m.keys().filter_map(|k| json_value::<Value>(&m, &k).map(|v| (k.to_string(), v))).collect()
}

/// Writes the settings in `patch` that differ; a `null` value removes the setting.
/// Returns whether anything changed. The caller commits.
pub fn write_settings(doc: &LoroDoc, patch: &serde_json::Map<String, Value>) -> Result<bool> {
    let current = read_settings(doc);
    let m = doc.get_map(SETTINGS);
    let mut changed = false;
    for (key, value) in patch {
        if key.is_empty() || key.len() > 64 || !key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-') {
            bail!("Invalid setting name.");
        }
        if value.is_null() {
            if current.contains_key(key) {
                m.delete(key)?;
                changed = true;
            }
            continue;
        }
        let json = serde_json::to_string(value)?;
        if json.len() > MAX_SETTING_JSON {
            bail!("The value of setting {key} is too large.");
        }
        if current.get(key) != Some(value) {
            m.insert(key, json)?;
            changed = true;
        }
    }
    if read_settings(doc).len() > MAX_SETTINGS {
        bail!("Too many settings.");
    }
    Ok(changed)
}

// ---- Devices -------------------------------------------------------------------------------

pub fn read_devices(doc: &LoroDoc) -> Vec<(String, DeviceRecord)> {
    let devices = doc.get_map(DEVICES);
    devices.keys().filter_map(|k| json_value::<DeviceRecord>(&devices, &k).map(|r| (k.to_string(), r))).collect()
}

pub fn write_device(doc: &LoroDoc, id: &str, record: &DeviceRecord) -> Result<()> {
    doc.get_map(DEVICES).insert(id, serde_json::to_string(record)?)?;
    Ok(())
}

pub fn read_addresses(doc: &LoroDoc, id: &str) -> Vec<std::net::SocketAddr> {
    match doc.get_map(ADDRESSES).get(id) {
        Some(ValueOrContainer::Value(LoroValue::String(s))) => serde_json::from_str(&s).unwrap_or_default(),
        _ => vec![],
    }
}

/// Publishes this device's addresses; returns whether they changed. The caller commits.
pub fn write_addresses(doc: &LoroDoc, id: &str, addrs: &[std::net::SocketAddr]) -> Result<bool> {
    let mut addrs = addrs.to_vec();
    addrs.sort();
    if read_addresses(doc, id) == addrs {
        return Ok(false);
    }
    doc.get_map(ADDRESSES).insert(id, serde_json::to_string(&addrs)?)?;
    Ok(true)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::positions::Seen;

    pub(crate) fn new_book(seed: u8) -> NewBook {
        NewBook {
            hash: format!("{seed:02x}").repeat(32),
            size: 1234,
            format: "epub".into(),
            original_name: format!("book-{seed}.epub"),
            title: format!("Book {seed}"),
            author: "Someone".into(),
        }
    }

    pub(crate) fn sync(a: &LoroDoc, b: &LoroDoc) {
        a.commit();
        b.commit();
        b.import(&a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap()).unwrap();
        a.import(&b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap()).unwrap();
    }

    fn record(rev: u64) -> PositionRecord {
        PositionRecord {
            rev,
            seen: Seen::from([("A".into(), rev)]),
            locator: serde_json::json!({ "cfi": "epubcfi(/6/4!/4/2)" }),
            fraction: 0.5,
            snippet: "text".into(),
            at: now_iso(),
        }
    }

    #[test]
    fn the_same_file_imported_on_two_devices_is_one_book() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        let (id_a, added_a) = add_book(&a, &new_book(1), "A").unwrap();
        let (id_b, added_b) = add_book(&b, &new_book(1), "B").unwrap();
        assert!(added_a && added_b);
        assert_eq!(id_a, id_b);
        update_book(&a, &id_a, &BookPatch { title: Some("Renamed on A".into()), ..Default::default() }).unwrap();
        update_book(&b, &id_b, &BookPatch { author: Some("Author from B".into()), ..Default::default() }).unwrap();
        sync(&a, &b);
        assert_eq!(list_books(&a), list_books(&b));
        let book = read_book(&a, &id_a).unwrap();
        assert_eq!((book.title.as_str(), book.author.as_str()), ("Renamed on A", "Author from B"));
        assert_eq!(list_books(&a).len(), 1);
        assert!(!add_book(&a, &new_book(1), "A").unwrap().1, "importing again finds the existing book");
    }

    #[test]
    fn trash_is_recoverable_and_purge_needs_trash() {
        let doc = LoroDoc::new();
        let (id, _) = add_book(&doc, &new_book(2), "A").unwrap();
        assert!(purge(&doc, &id, "A").is_err(), "only books in Trash can be deleted forever");
        assert!(set_trashed(&doc, &id, true).unwrap());
        assert!(set_trashed(&doc, &id, false).unwrap());
        assert!(!read_book(&doc, &id).unwrap().trashed);
        set_trashed(&doc, &id, true).unwrap();
        assert!(purge(&doc, &id, "A").unwrap());
        assert!(read_book(&doc, &id).unwrap().purged);
    }

    #[test]
    fn a_stale_device_cannot_bring_a_purged_book_back() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        let (id, _) = add_book(&a, &new_book(3), "A").unwrap();
        set_trashed(&a, &id, true).unwrap();
        sync(&a, &b);
        // A deletes it forever; B, offline, restores it from Trash and reads in it.
        purge(&a, &id, "A").unwrap();
        set_trashed(&b, &id, false).unwrap();
        write_position(&b, &id, "B", &record(1)).unwrap();
        update_book(&b, &id, &BookPatch { title: Some("Still here?".into()), ..Default::default() }).unwrap();
        sync(&a, &b);
        for doc in [&a, &b] {
            assert!(read_book(doc, &id).unwrap().purged);
            assert!(set_trashed(doc, &id, false).is_err());
            assert!(write_position(doc, &id, "B", &record(2)).is_err());
            assert!(update_book(doc, &id, &BookPatch { title: Some("x".into()), ..Default::default() }).is_err());
        }
    }

    #[test]
    fn importing_after_a_purge_creates_a_new_incarnation_and_reimports_converge() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        let (old, _) = add_book(&a, &new_book(4), "A").unwrap();
        write_position(&a, &old, "A", &record(5)).unwrap();
        set_trashed(&a, &old, true).unwrap();
        purge(&a, &old, "A").unwrap();
        sync(&a, &b);
        let (new_a, added) = add_book(&a, &new_book(4), "A").unwrap();
        assert!(added && new_a != old);
        assert_eq!(read_book(&a, &new_a).unwrap().incarnation, 2);
        assert!(read_positions(&a, &new_a).is_empty(), "the new book has no old positions");
        let (new_b, _) = add_book(&b, &new_book(4), "B").unwrap();
        assert_eq!(new_a, new_b, "both devices re-importing after seeing the purge get the same book");
        sync(&a, &b);
        let live: Vec<_> = list_books(&a).into_iter().filter(|b| !b.purged).collect();
        assert_eq!(live.len(), 1);
    }

    #[test]
    fn holdings_removed_downloads_and_settings() {
        let doc = LoroDoc::new();
        let hash = new_book(5).hash;
        let held = Holding { verified_at: now_iso() };
        assert!(write_holding(&doc, "A", &hash, Some(&held)).unwrap());
        assert!(read_holdings(&doc)["A"].contains(&hash));
        assert!(write_holding(&doc, "A", &hash, None).unwrap());
        assert!(!write_holding(&doc, "A", &hash, None).unwrap());
        assert!(write_holding(&doc, "A", "../etc/passwd", Some(&held)).is_err());

        assert!(set_download_removed(&doc, "A", "book", true).unwrap());
        assert!(read_removed_downloads(&doc, "A").contains("book"));
        assert!(set_download_removed(&doc, "A", "book", false).unwrap());
        assert!(read_removed_downloads(&doc, "A").is_empty());

        let patch = serde_json::json!({ "fontSize": 18, "theme": "sepia" });
        assert!(write_settings(&doc, patch.as_object().unwrap()).unwrap());
        assert!(!write_settings(&doc, patch.as_object().unwrap()).unwrap(), "unchanged values write nothing");
        assert_eq!(read_settings(&doc)["fontSize"], 18);
        assert!(write_settings(&doc, serde_json::json!({ "theme": null }).as_object().unwrap()).unwrap());
        assert!(!read_settings(&doc).contains_key("theme"));
        assert!(write_settings(&doc, serde_json::json!({ "bad key": 1 }).as_object().unwrap()).is_err());
    }

    #[test]
    fn invalid_books_are_refused() {
        let doc = LoroDoc::new();
        for bad in [
            NewBook { hash: "abc".into(), ..new_book(6) },
            NewBook { size: 0, ..new_book(6) },
            NewBook { size: MAX_BOOK_SIZE + 1, ..new_book(6) },
            NewBook { format: "EP/UB".into(), ..new_book(6) },
        ] {
            assert!(add_book(&doc, &bad, "A").is_err());
        }
    }
}
