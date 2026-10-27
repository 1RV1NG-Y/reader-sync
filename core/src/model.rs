//! How Bits data is laid out inside the replicated Loro document, and the merge rules.
//!
//! `bits` maps note id → a map holding (every map below is a *mergeable* child: devices that
//! create the same note or field independently, e.g. by importing the same backup, get the
//! same container, so their changes merge instead of one copy replacing the other):
//! - `title`, `body`, `source`: multi-value registers. Each is a map of edit id → [`Edit`].
//!   An edit lists the edits it replaces, i.e. the versions its author had seen. Edits that
//!   nothing replaces are the field's current versions: normally one; two or more when the
//!   field was changed on devices that hadn't seen each other's change. Those are kept
//!   and reported as a conflict instead of one silently overwriting the other.
//! - `topics`: map of lowercase topic → [`TopicEntry`], so topics added on different
//!   devices combine.
//! - `favorite`, `deleted` (Trash), `updatedAt`: last-writer-wins values. A note deleted on
//!   one device while edited on another stays in Trash with the edit kept.
//! - `createdAt`: set once.
//!
//! `devices` holds membership; `addresses` holds each device's current LAN addresses.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use loro::{LoroDoc, LoroMap, LoroValue, ValueOrContainer};
use serde::{Deserialize, Serialize};

/// Notes in the current layout (schema 3).
pub const BITS: &str = "bits";
/// Notes in the schema 2 layout, read once when upgrading.
pub const LEGACY_NOTES: &str = "notes";
pub const DEVICES: &str = "devices";
/// Map of endpoint id → JSON list of socket addresses, written only by that device.
pub const ADDRESSES: &str = "addresses";

pub const TEXT_FIELDS: [&str; 3] = ["title", "body", "source"];
pub const MAX_BODY: usize = 100_000;
pub const MAX_TOPICS: usize = 30;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Note {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub topics: Vec<String>,
    pub favorite: bool,
    pub created_at: String,
    pub updated_at: String,
    pub deleted: bool,
    /// For each text field, the versions this read reflects. Pass back as
    /// [`NotePatch::base`] so a save replaces only what its author saw.
    pub revs: BTreeMap<String, Vec<String>>,
    /// Text fields with more than one current version, newest first. The note's field
    /// value is the newest of them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty", default)]
    pub conflicts: BTreeMap<String, Vec<Version>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Version {
    pub value: String,
    pub at: String,
    /// Device id of the author.
    pub by: String,
}

/// Fields a caller may change. `None` leaves the field as it is.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NotePatch {
    pub title: Option<String>,
    pub body: Option<String>,
    pub source: Option<String>,
    pub topics: Option<Vec<String>>,
    pub favorite: Option<bool>,
    pub deleted: Option<bool>,
    /// The note's `revs` when the editor opened it. Text fields replace only these
    /// versions; anything newer from another device is kept as a conflict. Without a base,
    /// a write replaces every current version (resolving any conflict).
    pub base: Option<BTreeMap<String, Vec<String>>>,
    /// The topics when the editor opened. With it, only the topics the user added or
    /// removed change, so topics another device added meanwhile are kept. Without it,
    /// the topics become exactly `topics`.
    pub base_topics: Option<Vec<String>>,
}

/// A complete note brought in from a backup or the pre-sync library, keeping its id
/// and dates.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportedNote {
    pub id: String,
    pub title: String,
    pub body: String,
    pub source: String,
    pub topics: Vec<String>,
    pub favorite: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Edit {
    /// The value written.
    v: String,
    /// Edit ids this one replaces.
    s: Vec<String>,
    at: String,
    by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TopicEntry {
    name: String,
    on: bool,
    at: String,
    /// Position within the write, to keep the order the user typed.
    i: usize,
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

fn bool_field(map: &LoroMap, key: &str) -> bool {
    matches!(map.get(key), Some(ValueOrContainer::Value(LoroValue::Bool(true))))
}

fn child_map(map: &LoroMap, key: &str) -> Option<LoroMap> {
    match map.get(key) {
        Some(ValueOrContainer::Container(c)) => c.into_map().ok(),
        _ => None,
    }
}

fn note_map(doc: &LoroDoc, id: &str) -> Option<LoroMap> {
    child_map(&doc.get_map(BITS), id)
}

fn edits(register: &LoroMap) -> Vec<(String, Edit)> {
    register.keys().filter_map(|k| json_value::<Edit>(register, &k).map(|e| (k.to_string(), e))).collect()
}

/// Current versions of a register: edits no other edit replaces, newest first.
fn current(register: &LoroMap) -> Vec<(String, Edit)> {
    let all = edits(register);
    let replaced: BTreeSet<&str> = all.iter().flat_map(|(_, e)| e.s.iter().map(String::as_str)).collect();
    let mut live: Vec<(String, Edit)> = all.iter().filter(|(id, _)| !replaced.contains(id.as_str())).cloned().collect();
    live.sort_by(|(a_id, a), (b_id, b)| b.at.cmp(&a.at).then_with(|| b_id.cmp(a_id)));
    live
}

/// Writes `value` to a register, replacing `base` (or every current version when `None`).
/// Returns whether anything was written.
fn write_register(register: &LoroMap, value: &str, base: Option<&[String]>, by: &str) -> Result<bool> {
    let live = current(register);
    let live_ids: Vec<String> = live.iter().map(|(id, _)| id.clone()).collect();
    let replaces: Vec<String> = match base {
        Some(base) => base.to_vec(),
        None => live_ids.clone(),
    };
    // Nothing to do when the only current version already has this value and it is the
    // one being replaced.
    if live.len() == 1 && live[0].1.v == value && replaces.contains(&live_ids[0]) {
        return Ok(false);
    }
    let edit = Edit { v: value.to_owned(), s: replaces, at: now_iso(), by: by.to_owned() };
    register.insert(&uuid::Uuid::new_v4().to_string(), serde_json::to_string(&edit)?)?;
    // Drop records that are now replaced. Safe under concurrency: the replacement
    // relationship lives in the surviving edits, and edits are never rewritten.
    let replaced: BTreeSet<String> = edits(register).into_iter().flat_map(|(_, e)| e.s).collect();
    for id in register.keys().map(|k| k.to_string()).collect::<Vec<_>>() {
        if replaced.contains(&id) {
            register.delete(&id)?;
        }
    }
    Ok(true)
}

/// Key for an edit that every device importing the same value derives identically, so
/// independent imports of one backup are the same version rather than a conflict.
fn import_edit_id(value: &str, updated_at: &str) -> String {
    format!("import:{:08x}:{updated_at}", crc32fast::hash(value.as_bytes()))
}

fn read_topics(m: &LoroMap) -> Vec<String> {
    let Some(topics) = child_map(m, "topics") else { return vec![] };
    let mut on: Vec<TopicEntry> =
        topics.keys().filter_map(|k| json_value::<TopicEntry>(&topics, &k)).filter(|t| t.on).collect();
    on.sort_by(|a, b| a.at.cmp(&b.at).then(a.i.cmp(&b.i)).then_with(|| a.name.cmp(&b.name)));
    on.into_iter().map(|t| t.name).collect()
}

/// Turns topics on and off individually so changes made on other devices survive. With
/// `base` (the topics the editor opened with), only the user's own additions, removals,
/// and renames are written; without it, the topics become exactly `desired`.
fn write_topics(m: &LoroMap, desired: &[String], base: Option<&[String]>) -> Result<bool> {
    let topics = m.ensure_mergeable_map("topics")?;
    let desired = normalize_topics(desired);
    let at = now_iso();
    let mut changed = false;
    let wanted: BTreeMap<String, (usize, &String)> = desired.iter().enumerate().map(|(i, t)| (t.to_lowercase(), (i, t))).collect();
    let based: Option<BTreeMap<String, String>> =
        base.map(|b| normalize_topics(b).into_iter().map(|t| (t.to_lowercase(), t)).collect());
    for key in topics.keys().map(|k| k.to_string()).collect::<Vec<_>>() {
        let removed_by_user = match &based {
            Some(based) => based.contains_key(&key) && !wanted.contains_key(&key),
            None => !wanted.contains_key(&key),
        };
        if let Some(entry) = json_value::<TopicEntry>(&topics, &key) {
            if entry.on && removed_by_user {
                topics.insert(&key, serde_json::to_string(&TopicEntry { on: false, at: at.clone(), ..entry })?)?;
                changed = true;
            }
        }
    }
    for (key, (i, name)) in wanted {
        let existing = json_value::<TopicEntry>(&topics, &key);
        let write = match &based {
            // Added by the user, or renamed (case) by the user.
            Some(based) => match based.get(&key) {
                None => existing.as_ref().is_none_or(|e| !e.on || e.name != *name),
                Some(was) => was != name && existing.as_ref().is_some_and(|e| e.name != *name),
            },
            None => existing.as_ref().is_none_or(|e| !e.on || e.name != *name),
        };
        if write {
            topics.insert(&key, serde_json::to_string(&TopicEntry { name: name.clone(), on: true, at: at.clone(), i })?)?;
            changed = true;
        }
    }
    Ok(changed)
}

pub fn read_note(doc: &LoroDoc, id: &str) -> Option<Note> {
    let m = note_map(doc, id)?;
    let mut values = BTreeMap::new();
    let mut revs = BTreeMap::new();
    let mut conflicts = BTreeMap::new();
    for field in TEXT_FIELDS {
        let live = child_map(&m, field).map(|r| current(&r)).unwrap_or_default();
        values.insert(field, live.first().map(|(_, e)| e.v.clone()).unwrap_or_default());
        revs.insert(field.to_owned(), live.iter().map(|(id, _)| id.clone()).collect());
        if live.len() > 1 {
            conflicts.insert(
                field.to_owned(),
                live.into_iter().map(|(_, e)| Version { value: e.v, at: e.at, by: e.by }).collect(),
            );
        }
    }
    Some(Note {
        id: id.to_owned(),
        title: values.remove("title").unwrap_or_default(),
        body: values.remove("body").unwrap_or_default(),
        source: values.remove("source").unwrap_or_default(),
        topics: read_topics(&m),
        favorite: bool_field(&m, "favorite"),
        created_at: str_field(&m, "createdAt"),
        updated_at: str_field(&m, "updatedAt"),
        deleted: bool_field(&m, "deleted"),
        revs,
        conflicts,
    })
}

pub fn list_notes(doc: &LoroDoc) -> Vec<Note> {
    let notes = doc.get_map(BITS);
    let mut out: Vec<Note> = notes.keys().filter_map(|k| read_note(doc, &k)).collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

/// Trimmed, case-insensitively unique topics (mirrors `parseTopics` in Bits).
pub fn normalize_topics(topics: &[String]) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    topics
        .iter()
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .collect()
}

fn validate(title: Option<&str>, body: Option<&str>, source: Option<&str>, topics: Option<&[String]>) -> Result<()> {
    if let Some(body) = body {
        if body.trim().is_empty() {
            bail!("A bit needs some information.");
        }
        if body.chars().count() > MAX_BODY {
            bail!("A bit can hold up to {MAX_BODY} characters.");
        }
    }
    if title.is_some_and(|t| t.chars().count() > 200) {
        bail!("Titles are limited to 200 characters.");
    }
    if source.is_some_and(|s| s.chars().count() > 2000) {
        bail!("Sources are limited to 2000 characters.");
    }
    if let Some(topics) = topics {
        let topics = normalize_topics(topics);
        if topics.len() > MAX_TOPICS || topics.iter().any(|t| t.chars().count() > 80) {
            bail!("A bit can have up to {MAX_TOPICS} topics of 80 characters each.");
        }
    }
    Ok(())
}

/// Applies a patch, writing only what differs; returns whether anything changed.
/// `by` is this device's id. The caller commits.
pub fn apply_patch(doc: &LoroDoc, id: &str, patch: &NotePatch, by: &str) -> Result<bool> {
    validate(patch.title.as_deref(), patch.body.as_deref(), patch.source.as_deref(), patch.topics.as_deref())?;
    let m = note_map(doc, id).context("That bit no longer exists.")?;
    let mut changed = false;
    for (field, value) in [("title", &patch.title), ("body", &patch.body), ("source", &patch.source)] {
        if let Some(value) = value {
            let register = m.ensure_mergeable_map(field)?;
            let base = patch.base.as_ref().and_then(|b| b.get(field)).map(Vec::as_slice);
            changed |= write_register(&register, value, base, by)?;
        }
    }
    if let Some(topics) = &patch.topics {
        changed |= write_topics(&m, topics, patch.base_topics.as_deref())?;
    }
    for (key, value) in [("favorite", patch.favorite), ("deleted", patch.deleted)] {
        if let Some(v) = value {
            if bool_field(&m, key) != v {
                m.insert(key, v)?;
                changed = true;
            }
        }
    }
    if changed {
        m.insert("updatedAt", now_iso())?;
    }
    Ok(changed)
}

pub fn create_note(doc: &LoroDoc, patch: &NotePatch, by: &str) -> Result<String> {
    if patch.body.is_none() {
        bail!("A bit needs some information.");
    }
    validate(patch.title.as_deref(), patch.body.as_deref(), patch.source.as_deref(), patch.topics.as_deref())?;
    let id = uuid::Uuid::new_v4().to_string();
    let now = now_iso();
    insert_note(
        doc,
        &ImportedNote {
            id: id.clone(),
            title: patch.title.clone().unwrap_or_default(),
            body: patch.body.clone().unwrap_or_default(),
            source: patch.source.clone().unwrap_or_default(),
            topics: patch.topics.clone().unwrap_or_default(),
            favorite: patch.favorite.unwrap_or(false),
            created_at: now.clone(),
            updated_at: now,
        },
        by,
        false,
    )?;
    Ok(id)
}

/// Adds a note with its own id and dates. Returns false (and changes nothing) if a note
/// with that id already exists, so importing the same backup twice adds nothing.
pub fn import_note(doc: &LoroDoc, note: &ImportedNote, by: &str) -> Result<bool> {
    if note.id.is_empty() || note.id.len() > 100 || note.id.contains(['/', '\0']) {
        bail!("A bit has an invalid id.");
    }
    validate(Some(&note.title), Some(&note.body), Some(&note.source), Some(&note.topics))?;
    if note_map(doc, &note.id).is_some() {
        return Ok(false);
    }
    insert_note(doc, note, by, true)?;
    Ok(true)
}

fn insert_note(doc: &LoroDoc, note: &ImportedNote, by: &str, deterministic: bool) -> Result<()> {
    let m = doc.get_map(BITS).ensure_mergeable_map(&note.id)?;
    m.insert("createdAt", note.created_at.as_str())?;
    m.insert("updatedAt", note.updated_at.as_str())?;
    for (field, value) in [("title", &note.title), ("body", &note.body), ("source", &note.source)] {
        let register = m.ensure_mergeable_map(field)?;
        let (key, by) = if deterministic {
            (import_edit_id(value, &note.updated_at), "import".to_owned())
        } else {
            (uuid::Uuid::new_v4().to_string(), by.to_owned())
        };
        let edit = Edit { v: value.clone(), s: vec![], at: note.updated_at.clone(), by };
        register.insert(&key, serde_json::to_string(&edit)?)?;
    }
    write_topics(&m, &note.topics, None)?;
    m.insert("favorite", note.favorite)?;
    m.insert("deleted", false)?;
    Ok(())
}

/// Moves notes from the schema 2 layout (plain child containers, which don't merge when
/// two devices create the same note) into the mergeable layout. Current versions keep
/// their ids, which every device already shares, so devices upgrading independently
/// produce identical results. Returns how many notes were moved. The caller commits.
pub fn upgrade_from_v2(doc: &LoroDoc) -> Result<usize> {
    let legacy = doc.get_map(LEGACY_NOTES);
    let bits = doc.get_map(BITS);
    let mut moved = 0;
    for id in legacy.keys().map(|k| k.to_string()).collect::<Vec<_>>() {
        let Some(old) = child_map(&legacy, &id) else { continue };
        if child_map(&bits, &id).is_some() {
            continue;
        }
        let m = bits.ensure_mergeable_map(&id)?;
        for key in ["createdAt", "updatedAt"] {
            m.insert(key, str_field(&old, key))?;
        }
        for key in ["favorite", "deleted"] {
            m.insert(key, bool_field(&old, key))?;
        }
        for field in TEXT_FIELDS {
            let register = m.ensure_mergeable_map(field)?;
            if let Some(old_register) = child_map(&old, field) {
                for (edit_id, edit) in current(&old_register) {
                    register.insert(&edit_id, serde_json::to_string(&edit)?)?;
                }
            }
        }
        let topics = m.ensure_mergeable_map("topics")?;
        if let Some(old_topics) = child_map(&old, "topics") {
            for key in old_topics.keys() {
                if let Some(ValueOrContainer::Value(v)) = old_topics.get(&key) {
                    topics.insert(&key, v)?;
                }
            }
        }
        moved += 1;
    }
    Ok(moved)
}

/// Removes notes for good (emptying Trash). The caller commits.
pub fn delete_forever(doc: &LoroDoc, ids: &[String]) -> Result<usize> {
    let notes = doc.get_map(BITS);
    let mut removed = 0;
    for id in ids {
        if note_map(doc, id).is_some_and(|m| bool_field(&m, "deleted")) {
            notes.delete(id)?;
            removed += 1;
        }
    }
    Ok(removed)
}

pub fn read_devices(doc: &LoroDoc) -> Vec<(String, DeviceRecord)> {
    let devices = doc.get_map(DEVICES);
    devices
        .keys()
        .filter_map(|k| match devices.get(&k) {
            Some(ValueOrContainer::Value(LoroValue::String(s))) => {
                serde_json::from_str(&s).ok().map(|r| (k.to_string(), r))
            }
            _ => None,
        })
        .collect()
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

    fn patch_body(text: &str) -> NotePatch {
        NotePatch { body: Some(text.into()), ..Default::default() }
    }

    /// Two replicas that start from the same note, as two devices after syncing.
    fn two_replicas() -> (LoroDoc, LoroDoc, String) {
        let a = LoroDoc::new();
        let id = create_note(&a, &NotePatch { body: Some("original".into()), topics: Some(vec!["Rust".into()]), ..Default::default() }, "A").unwrap();
        a.commit();
        let b = LoroDoc::new();
        b.import(&a.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        (a, b, id)
    }

    fn sync(a: &LoroDoc, b: &LoroDoc) {
        a.commit();
        b.commit();
        b.import(&a.export(loro::ExportMode::updates(&b.oplog_vv())).unwrap()).unwrap();
        a.import(&b.export(loro::ExportMode::updates(&a.oplog_vv())).unwrap()).unwrap();
    }

    #[test]
    fn same_field_edited_apart_keeps_both_versions() {
        let (a, b, id) = two_replicas();
        apply_patch(&a, &id, &patch_body("edited on A"), "A").unwrap();
        apply_patch(&b, &id, &patch_body("edited on B"), "B").unwrap();
        sync(&a, &b);
        let (na, nb) = (read_note(&a, &id).unwrap(), read_note(&b, &id).unwrap());
        assert_eq!(na, nb, "both devices agree");
        let versions: BTreeSet<String> = na.conflicts["body"].iter().map(|v| v.value.clone()).collect();
        assert_eq!(versions, BTreeSet::from(["edited on A".into(), "edited on B".into()]));

        // Resolving replaces both versions everywhere.
        apply_patch(&a, &id, &NotePatch { body: Some("merged".into()), base: Some(na.revs.clone()), ..Default::default() }, "A").unwrap();
        sync(&a, &b);
        let resolved = read_note(&b, &id).unwrap();
        assert_eq!(resolved.body, "merged");
        assert!(resolved.conflicts.is_empty());
    }

    #[test]
    fn different_fields_combine_without_conflict() {
        let (a, b, id) = two_replicas();
        apply_patch(&a, &id, &NotePatch { title: Some("Title from A".into()), ..Default::default() }, "A").unwrap();
        apply_patch(&b, &id, &patch_body("body from B"), "B").unwrap();
        sync(&a, &b);
        let n = read_note(&a, &id).unwrap();
        assert_eq!((n.title.as_str(), n.body.as_str()), ("Title from A", "body from B"));
        assert!(n.conflicts.is_empty());
    }

    #[test]
    fn saving_from_an_old_base_keeps_the_newer_remote_version() {
        let (a, b, id) = two_replicas();
        let opened = read_note(&a, &id).unwrap(); // Editor opens on A.
        apply_patch(&b, &id, &patch_body("changed on B meanwhile"), "B").unwrap();
        sync(&a, &b); // Arrives while A is typing.
        apply_patch(&a, &id, &NotePatch { body: Some("A's draft".into()), base: Some(opened.revs), ..Default::default() }, "A").unwrap();
        let n = read_note(&a, &id).unwrap();
        assert_eq!(n.conflicts["body"].len(), 2, "the draft didn't overwrite B's change");
    }

    #[test]
    fn topics_added_apart_combine_and_removals_stick() {
        let (a, b, id) = two_replicas();
        apply_patch(&a, &id, &NotePatch { topics: Some(vec!["Rust".into(), "Sync".into()]), ..Default::default() }, "A").unwrap();
        apply_patch(&b, &id, &NotePatch { topics: Some(vec!["Rust".into(), "Android".into()]), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        let topics: BTreeSet<String> = read_note(&a, &id).unwrap().topics.into_iter().collect();
        assert_eq!(topics, BTreeSet::from(["Rust".into(), "Sync".into(), "Android".into()]));
        apply_patch(&b, &id, &NotePatch { topics: Some(vec!["Sync".into()]), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        assert_eq!(read_note(&a, &id).unwrap().topics, vec!["Sync".to_string()]);
    }

    #[test]
    fn deleted_while_edited_elsewhere_stays_in_trash_with_the_edit() {
        let (a, b, id) = two_replicas();
        apply_patch(&a, &id, &NotePatch { deleted: Some(true), ..Default::default() }, "A").unwrap();
        apply_patch(&b, &id, &patch_body("edited on B"), "B").unwrap();
        sync(&a, &b);
        let n = read_note(&a, &id).unwrap();
        assert!(n.deleted);
        assert_eq!(n.body, "edited on B");
    }

    #[test]
    fn an_old_copy_cannot_bring_a_deleted_note_back() {
        let (a, b, id) = two_replicas();
        // B goes offline holding the old copy; A deletes; B reconnects without touching it.
        apply_patch(&a, &id, &NotePatch { deleted: Some(true), ..Default::default() }, "A").unwrap();
        apply_patch(&b, &id, &NotePatch { favorite: Some(true), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        assert!(read_note(&b, &id).unwrap().deleted);
    }

    #[test]
    fn import_keeps_ids_and_dates_and_is_idempotent() {
        let doc = LoroDoc::new();
        let note = ImportedNote {
            id: "fixed-id".into(),
            title: "T".into(),
            body: "B".into(),
            source: "S".into(),
            topics: vec!["One".into(), "Two".into()],
            favorite: true,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-02-01T00:00:00.000Z".into(),
        };
        assert!(import_note(&doc, &note, "A").unwrap());
        assert!(!import_note(&doc, &note, "A").unwrap());
        let n = read_note(&doc, "fixed-id").unwrap();
        assert_eq!((n.title, n.body, n.source, n.topics, n.favorite), ("T".into(), "B".into(), "S".into(), vec!["One".into(), "Two".into()], true));
        assert_eq!((n.created_at.as_str(), n.updated_at.as_str()), ("2026-01-01T00:00:00.000Z", "2026-02-01T00:00:00.000Z"));
        assert!(n.conflicts.is_empty() && !n.deleted);
    }

    #[test]
    fn unchanged_saves_write_nothing() {
        let (a, _, id) = two_replicas();
        let n = read_note(&a, &id).unwrap();
        let same = NotePatch { body: Some("original".into()), topics: Some(vec!["Rust".into()]), base: Some(n.revs), ..Default::default() };
        assert!(!apply_patch(&a, &id, &same, "A").unwrap());
    }

    fn imported(id: &str) -> ImportedNote {
        ImportedNote {
            id: id.into(),
            title: "Old title".into(),
            body: "Old body".into(),
            source: String::new(),
            topics: vec!["Rust".into()],
            favorite: false,
            created_at: "2026-01-01T00:00:00.000Z".into(),
            updated_at: "2026-01-02T00:00:00.000Z".into(),
        }
    }

    #[test]
    fn the_same_note_imported_on_two_devices_merges_both_edits() {
        // Both devices migrated their own copy of the same old library before pairing.
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        import_note(&a, &imported("same"), "A").unwrap();
        import_note(&b, &imported("same"), "B").unwrap();
        let (ra, rb) = (read_note(&a, "same").unwrap(), read_note(&b, "same").unwrap());
        assert_eq!(ra.revs, rb.revs, "the imported versions are the same version on both");
        apply_patch(&a, "same", &NotePatch { title: Some("Title from A".into()), base: Some(ra.revs), ..Default::default() }, "A").unwrap();
        apply_patch(&b, "same", &NotePatch { body: Some("Body from B".into()), base: Some(rb.revs), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        for doc in [&a, &b] {
            let n = read_note(doc, "same").unwrap();
            assert_eq!((n.title.as_str(), n.body.as_str()), ("Title from A", "Body from B"));
            assert!(n.conflicts.is_empty(), "{:?}", n.conflicts);
            assert_eq!(n.topics, vec!["Rust".to_string()]);
        }
        assert_eq!(list_notes(&a).len(), 1);
    }

    #[test]
    fn the_same_imported_field_edited_on_both_devices_is_a_conflict() {
        let (a, b) = (LoroDoc::new(), LoroDoc::new());
        import_note(&a, &imported("same"), "A").unwrap();
        import_note(&b, &imported("same"), "B").unwrap();
        apply_patch(&a, "same", &patch_body("A's body"), "A").unwrap();
        apply_patch(&b, "same", &patch_body("B's body"), "B").unwrap();
        sync(&a, &b);
        assert_eq!(read_note(&a, "same").unwrap().conflicts["body"].len(), 2);
    }

    #[test]
    fn an_old_draft_keeps_topics_added_elsewhere() {
        let (a, b, id) = two_replicas(); // Topics: Rust.
        let opened = read_note(&a, &id).unwrap();
        apply_patch(&b, &id, &NotePatch { topics: Some(vec!["Rust".into(), "Remote".into()]), base_topics: Some(vec!["Rust".into()]), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        // A's editor was opened before Remote arrived and adds Local.
        apply_patch(&a, &id, &NotePatch { topics: Some(vec!["Rust".into(), "Local".into()]), base_topics: Some(opened.topics.clone()), ..Default::default() }, "A").unwrap();
        sync(&a, &b);
        let topics: BTreeSet<String> = read_note(&b, &id).unwrap().topics.into_iter().collect();
        assert_eq!(topics, BTreeSet::from(["Rust".into(), "Remote".into(), "Local".into()]));
        // Removing a topic from an old draft removes only that topic.
        apply_patch(&a, &id, &NotePatch { topics: Some(vec!["Local".into()]), base_topics: Some(vec!["Rust".into(), "Local".into()]), ..Default::default() }, "A").unwrap();
        let topics: BTreeSet<String> = read_note(&a, &id).unwrap().topics.into_iter().collect();
        assert_eq!(topics, BTreeSet::from(["Remote".into(), "Local".into()]));
    }

    /// Writes a note the way schema 2 did: plain child containers under `notes`.
    pub(crate) fn legacy_note(doc: &LoroDoc, id: &str, versions: &[(&str, &str)]) {
        let m = doc.get_map(LEGACY_NOTES).insert_container(id, LoroMap::new()).unwrap();
        m.insert("createdAt", "2026-01-01T00:00:00.000Z").unwrap();
        m.insert("updatedAt", "2026-01-02T00:00:00.000Z").unwrap();
        m.insert("favorite", true).unwrap();
        m.insert("deleted", false).unwrap();
        for field in TEXT_FIELDS {
            let register = m.insert_container(field, LoroMap::new()).unwrap();
            let values: Vec<(&str, &str)> = if field == "body" { versions.to_vec() } else { vec![("t-1", "Title")] };
            for (key, value) in values {
                let edit = Edit { v: value.into(), s: vec![], at: "2026-01-02T00:00:00.000Z".into(), by: "X".into() };
                register.insert(&format!("{field}-{key}"), serde_json::to_string(&edit).unwrap()).unwrap();
            }
        }
        let topics = m.insert_container("topics", LoroMap::new()).unwrap();
        let t = TopicEntry { name: "Rust".into(), on: true, at: "2026-01-02T00:00:00.000Z".into(), i: 0 };
        topics.insert("rust", serde_json::to_string(&t).unwrap()).unwrap();
    }

    #[test]
    fn schema_2_libraries_upgrade_identically_on_every_device() {
        let a = LoroDoc::new();
        legacy_note(&a, "plain", &[("1", "Body")]);
        legacy_note(&a, "conflicted", &[("1", "Version one"), ("2", "Version two")]);
        a.commit();
        let b = LoroDoc::new();
        b.import(&a.export(loro::ExportMode::Snapshot).unwrap()).unwrap();
        // Each device upgrades on its own, then they sync.
        assert_eq!(upgrade_from_v2(&a).unwrap(), 2);
        assert_eq!(upgrade_from_v2(&b).unwrap(), 2);
        assert_eq!(upgrade_from_v2(&a).unwrap(), 0, "a resumed upgrade skips moved notes");
        sync(&a, &b);
        assert_eq!(list_notes(&a), list_notes(&b));
        let plain = read_note(&a, "plain").unwrap();
        assert_eq!((plain.title.as_str(), plain.body.as_str(), plain.favorite), ("Title", "Body", true));
        assert_eq!(plain.topics, vec!["Rust".to_string()]);
        assert!(plain.conflicts.is_empty());
        assert_eq!(read_note(&a, "conflicted").unwrap().conflicts["body"].len(), 2, "existing conflicts are kept");
        // Edits after the upgrade merge normally.
        apply_patch(&a, "plain", &NotePatch { title: Some("New".into()), base: Some(plain.revs.clone()), ..Default::default() }, "A").unwrap();
        apply_patch(&b, "plain", &NotePatch { body: Some("New body".into()), base: Some(plain.revs), ..Default::default() }, "B").unwrap();
        sync(&a, &b);
        let n = read_note(&b, "plain").unwrap();
        assert_eq!((n.title.as_str(), n.body.as_str()), ("New", "New body"));
        assert!(n.conflicts.is_empty());
    }
}
