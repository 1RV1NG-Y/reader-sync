//! Which reading position to open a book at (PLAN.md §4), decided without trusting clocks.
//!
//! Every position a device saves gets the next number from that device's revision counter.
//! A position's `seen` records, for each device, the highest revision this reading has
//! continued from (its own included), so it survives the device overwriting its record with
//! newer positions. Position A *descends from* B when `A.seen[B.device] >= B.rev`. The *heads*
//! are the positions no other device's position descends from:
//!
//! - one head, mine → open there;
//! - one head, another device's → a continuation: open there automatically;
//! - several heads → divergent: ask.
//!
//! Heads that point at the same place count as one. `at` is the writing device's clock and is
//! used only for display.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub type Seen = BTreeMap<String, u64>;

const MAX_LOCATOR_JSON: usize = 4096;
const MAX_SNIPPET: usize = 200;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PositionRecord {
    pub rev: u64,
    pub seen: Seen,
    /// Format-specific and opaque to the core: `{cfi, section}` for EPUB, `{page, offset}`
    /// for PDF.
    pub locator: Value,
    /// 0..1 through the book, for progress display.
    pub fraction: f64,
    pub snippet: String,
    pub at: String,
}

impl PositionRecord {
    pub fn validate(&self) -> Result<()> {
        if !(0.0..=1.0).contains(&self.fraction) {
            bail!("A reading position must be between 0 and 1.");
        }
        if serde_json::to_string(&self.locator)?.len() > MAX_LOCATOR_JSON || self.snippet.chars().count() > MAX_SNIPPET {
            bail!("A reading position is too large.");
        }
        Ok(())
    }

    pub fn descends_from(&self, device: &str, other: &PositionRecord) -> bool {
        self.seen.get(device).is_some_and(|rev| *rev >= other.rev)
    }
}

/// Where a reader is, as reported by the front end.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub locator: Value,
    pub fraction: f64,
    #[serde(default)]
    pub snippet: String,
}

/// A position together with the device it belongs to.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Placed {
    pub device: String,
    #[serde(flatten)]
    pub record: PositionRecord,
}

/// Identifies one revision of one device's position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevRef {
    pub device: String,
    pub rev: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Decision {
    Mine,
    Continued,
    Divergent,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub decision: Decision,
    /// Where to open: my position (`mine`), the head continued from (`continued`), or `None`
    /// (`divergent`: the reader chooses, or there is no position yet).
    pub position: Option<Placed>,
    /// This device's own latest position, for "Back to where I was here".
    pub mine: Option<Placed>,
    /// The competing positions, newest first by their devices' clocks (display only).
    pub heads: Vec<Placed>,
}

/// Positions that no other device's position descends from.
pub fn heads(records: &BTreeMap<String, PositionRecord>) -> Vec<Placed> {
    records
        .iter()
        .filter(|(device, record)| {
            !records.iter().any(|(other, r)| other != *device && r.descends_from(device, record))
        })
        .map(|(device, record)| Placed { device: device.clone(), record: record.clone() })
        .collect()
}

pub fn select(records: &BTreeMap<String, PositionRecord>, me: &str) -> Selection {
    let mine = records.get(me).map(|r| Placed { device: me.to_owned(), record: r.clone() });
    let mut heads = heads(records);
    heads.sort_by(|a, b| b.record.at.cmp(&a.record.at).then_with(|| a.device.cmp(&b.device)));
    let one_place = heads.windows(2).all(|w| w[0].record.locator == w[1].record.locator);
    let (decision, position) = if heads.is_empty() {
        (Decision::Mine, None)
    } else if one_place {
        match heads.iter().find(|h| h.device == me) {
            Some(own) => (Decision::Mine, Some(own.clone())),
            None => (Decision::Continued, heads.iter().min_by(|a, b| a.device.cmp(&b.device)).cloned()),
        }
    } else {
        (Decision::Divergent, None)
    };
    Selection { decision, position, mine, heads }
}

/// `seen` for a new revision `rev` by `me` that continues from `mine` and has taken
/// `acknowledged` into account.
pub fn next_seen<'a>(mine: Option<&PositionRecord>, acknowledged: impl IntoIterator<Item = (&'a str, &'a PositionRecord)>, me: &str, rev: u64) -> Seen {
    let mut seen = mine.map(|r| r.seen.clone()).unwrap_or_default();
    for (device, record) in acknowledged {
        merge(&mut seen, device, record.rev);
        for (d, r) in &record.seen {
            merge(&mut seen, d, *r);
        }
    }
    seen.insert(me.to_owned(), rev);
    seen
}

fn merge(seen: &mut Seen, device: &str, rev: u64) {
    let entry = seen.entry(device.to_owned()).or_insert(0);
    *entry = (*entry).max(rev);
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Simulates devices reading: each keeps its latest record, and `share` copies records
    /// between devices the way sync would. Timestamps are deliberately wrong to show that
    /// decisions never depend on them.
    #[derive(Default, Clone)]
    struct World {
        /// What each device's replica holds: device → (device → record).
        replicas: BTreeMap<String, BTreeMap<String, PositionRecord>>,
        counters: BTreeMap<String, u64>,
    }

    impl World {
        fn records(&self, device: &str) -> BTreeMap<String, PositionRecord> {
            self.replicas.get(device).cloned().unwrap_or_default()
        }

        /// `device` saves a position at `place`, acknowledging `ack` (the heads it was shown).
        fn save(&mut self, device: &str, place: &str, ack: &[&str], clock: &str) {
            let records = self.records(device);
            let rev = { let c = self.counters.entry(device.into()).or_insert(0); *c += 1; *c };
            let acknowledged: Vec<(&str, &PositionRecord)> = ack.iter().filter_map(|d| records.get(*d).map(|r| (*d, r))).collect();
            let seen = next_seen(records.get(device), acknowledged, device, rev);
            let record = PositionRecord { rev, seen, locator: json!({ "cfi": place }), fraction: 0.1, snippet: place.into(), at: clock.into() };
            self.replicas.entry(device.into()).or_default().insert(device.into(), record);
        }

        /// Opens the book on `device` the way the app does: continuations and choices are
        /// saved at once with the heads acknowledged.
        fn open(&mut self, device: &str, choose: Option<&str>) -> (Decision, String) {
            let records = self.records(device);
            let sel = select(&records, device);
            let heads: Vec<String> = sel.heads.iter().map(|h| h.device.clone()).collect();
            let ack: Vec<&str> = heads.iter().map(String::as_str).collect();
            let place = match sel.decision {
                Decision::Mine => return (Decision::Mine, sel.position.map(|p| cfi(&p.record)).unwrap_or_default()),
                Decision::Continued => cfi(&sel.position.unwrap().record),
                Decision::Divergent => cfi(&records[choose.expect("divergent needs a choice")]),
            };
            self.save(device, &place, &ack, "1999-01-01T00:00:00Z");
            (sel.decision, place)
        }

        fn share(&mut self, from: &str, to: &str) {
            for (device, record) in self.records(from) {
                let target = self.replicas.entry(to.into()).or_default();
                if target.get(&device).is_none_or(|r| r.rev < record.rev) {
                    target.insert(device, record);
                }
            }
        }
    }

    fn cfi(r: &PositionRecord) -> String {
        r.locator["cfi"].as_str().unwrap().to_owned()
    }

    #[test]
    fn linux_phone_windows_linux_each_continue_automatically() {
        let mut w = World::default();
        w.save("linux", "ch1", &[], "2030-01-01T00:00:00Z");
        w.save("linux", "ch2", &[], "2030-01-01T00:01:00Z");
        w.share("linux", "phone");
        assert_eq!(w.open("phone", None), (Decision::Continued, "ch2".into()));
        w.save("phone", "ch3", &[], "2001-01-01T00:00:00Z"); // phone's clock is way behind
        w.share("phone", "windows");
        assert_eq!(w.open("windows", None), (Decision::Continued, "ch3".into()));
        w.save("windows", "ch4", &[], "1970-01-01T00:00:00Z");
        w.share("windows", "phone");
        w.share("phone", "linux");
        // Linux never talked to Windows directly, and its own record is "newer" by the clock.
        assert_eq!(w.open("linux", None), (Decision::Continued, "ch4".into()));
        w.save("linux", "ch5", &[], "2030-01-01T00:02:00Z");
        assert_eq!(w.open("linux", None), (Decision::Mine, "ch5".into()));
    }

    #[test]
    fn reading_apart_from_a_shared_start_asks_and_one_choice_settles_it_everywhere() {
        let mut w = World::default();
        w.save("linux", "ch1", &[], "2030-01-01T00:00:00Z");
        w.share("linux", "phone");
        w.open("phone", None);
        // Both read offline from ch1.
        w.save("linux", "reread-ch1", &[], "2030-01-02T00:00:00Z");
        w.save("phone", "ch7", &[], "2030-01-01T00:00:00Z");
        w.share("phone", "linux");
        w.share("linux", "phone");
        let sel = select(&w.records("linux"), "linux");
        assert_eq!(sel.decision, Decision::Divergent);
        assert_eq!(sel.heads.len(), 2);
        // Choose the phone's place on Linux, then close without moving.
        assert_eq!(w.open("linux", Some("phone")), (Decision::Divergent, "ch7".into()));
        w.share("linux", "phone");
        w.share("linux", "windows");
        w.share("phone", "windows");
        for device in ["linux", "phone", "windows"] {
            assert_ne!(select(&w.records(device), device).decision, Decision::Divergent, "{device} asks again");
        }
        // The phone sees Linux's choice as a continuation (same place, so no jump).
        assert_eq!(w.open("phone", None).1, "ch7");
    }

    #[test]
    fn choosing_to_stay_also_settles_it() {
        let mut w = World::default();
        w.save("linux", "a", &[], "x");
        w.share("linux", "phone");
        w.open("phone", None);
        w.save("linux", "b", &[], "x");
        w.save("phone", "c", &[], "x");
        w.share("phone", "linux");
        assert_eq!(w.open("linux", Some("linux")), (Decision::Divergent, "b".into()));
        w.share("linux", "phone");
        assert_eq!(w.open("phone", None), (Decision::Continued, "b".into()));
    }

    #[test]
    fn heads_at_the_same_place_are_not_a_divergence() {
        let mut w = World::default();
        w.save("linux", "same", &[], "x");
        w.save("phone", "same", &[], "y");
        w.share("phone", "linux");
        assert_eq!(select(&w.records("linux"), "linux").decision, Decision::Mine);
        w.share("linux", "windows");
        w.share("phone", "windows");
        assert_eq!(select(&w.records("windows"), "windows").decision, Decision::Continued);
    }

    #[test]
    fn a_device_reading_without_seeing_a_newer_head_is_a_real_divergence() {
        let mut w = World::default();
        w.save("linux", "ch1", &[], "x");
        w.share("linux", "phone");
        w.open("phone", None);
        w.save("phone", "ch2", &[], "x");
        w.share("phone", "linux");
        // Linux was already reading; the banner was ignored and it kept going.
        w.save("linux", "ch1-later", &[], "x");
        assert_eq!(select(&w.records("linux"), "linux").decision, Decision::Divergent);
    }

    #[test]
    fn no_positions_opens_at_the_start() {
        let sel = select(&BTreeMap::new(), "me");
        assert_eq!((sel.decision, sel.position), (Decision::Mine, None));
    }

    #[test]
    fn invalid_records_are_refused() {
        let base = PositionRecord { rev: 1, seen: Seen::new(), locator: json!({}), fraction: 0.5, snippet: String::new(), at: String::new() };
        assert!(base.validate().is_ok());
        assert!(PositionRecord { fraction: 1.5, ..base.clone() }.validate().is_err());
        assert!(PositionRecord { fraction: f64::NAN, ..base.clone() }.validate().is_err());
        assert!(PositionRecord { locator: json!("x".repeat(5000)), ..base }.validate().is_err());
    }
}
