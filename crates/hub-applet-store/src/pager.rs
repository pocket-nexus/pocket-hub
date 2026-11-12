//! The link's catalog rows and pages (link specification §6.4), as the
//! reference does them (`tools/pocket-link/v2/paging.ts` `catalogRows`,
//! `CatalogPager`), on an injected clock. Pure: no store, no connection.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use pocket_link::consts::{APP_ID_BYTES, APP_OUTPUT_BYTES, APP_VERSION_BYTES, CATALOG_MAX_APPS, CATALOG_SNAPSHOT_MS, CATALOG_SUMMARY_BYTES, CATALOG_TITLE_BYTES};
use pocket_link::text::{encoded_bytes, encoded_prefix, json_bytes};
use serde::Serialize;
use serde_json::{Map, Value};

/// One row of the catalog (`LinkCatalogApp`), in the order of its members on the wire.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub id: String,
    pub title: String,
    pub version: String,
    pub summary: String,
    pub output: String,
    /// The file's footer hash, 16 hex digits.
    pub hash: String,
    pub bytes: u64,
}

/// The offer as rows (§6.4): one per id (the first wins), `title` and
/// `summary` cut to their limits on a code point, an app whose `id`,
/// `output` or `version` is over its limit left out (the device could not
/// install or show it under that name), ordered by `id` in byte order, the
/// first 64.
pub fn rows(offer: impl IntoIterator<Item = Row>) -> Vec<Row> {
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    for app in offer {
        if !seen.insert(app.id.clone()) {
            continue;
        }
        if app.id.is_empty() || encoded_bytes(&app.id) > APP_ID_BYTES || encoded_bytes(&app.output) > APP_OUTPUT_BYTES || encoded_bytes(&app.version) > APP_VERSION_BYTES {
            continue;
        }
        rows.push(Row {
            title: encoded_prefix(&app.title, CATALOG_TITLE_BYTES).to_owned(),
            summary: encoded_prefix(&app.summary, CATALOG_SUMMARY_BYTES).to_owned(),
            ..app
        });
    }
    // `str` orders by its UTF-8 bytes.
    rows.sort_by(|a, b| a.id.cmp(&b.id));
    rows.truncate(CATALOG_MAX_APPS);
    rows
}

/// A catalog reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Page {
    pub apps: Vec<Row>,
    pub rev: u32,
    pub more: bool,
}

/// The rows after `from` that fit one reply of `payload_limit` bytes, at
/// least one, sized as if `more` were false (the longer form) so a page that
/// turns out to be the last never grows past the limit.
pub fn page(rows: &[Row], from: usize, rev: u32, payload_limit: u32) -> Page {
    let fits = |count: usize| json_bytes(&Page { apps: rows[from..from + count].to_vec(), rev, more: false }) <= payload_limit as usize;
    let mut count = 0;
    while from + count < rows.len() && (count == 0 || fits(count + 1)) {
        count += 1;
    }
    Page { apps: rows[from..from + count].to_vec(), rev, more: from + count < rows.len() }
}

/// A page request: the first page, or the page after a cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cursor {
    First,
    After { after: String, rev: u32 },
}

/// Reads `{after?, rev?}`: a cursor needs both, `after` at most 128 bytes
/// and `rev` a u32; anything else is `malformed`.
pub fn cursor(object: &Map<String, Value>) -> Result<Cursor, &'static str> {
    match (object.get("after"), object.get("rev")) {
        (None, None) => Ok(Cursor::First),
        (Some(Value::String(after)), Some(rev)) if encoded_bytes(after) <= APP_ID_BYTES => {
            let rev = rev.as_u64().and_then(|rev| u32::try_from(rev).ok()).ok_or("rev is not a u32")?;
            Ok(Cursor::After { after: after.clone(), rev })
        }
        (Some(Value::String(_)), Some(_)) => Err("after is longer than 128 bytes"),
        (Some(_), None) | (None, Some(_)) => Err("after and rev go together"),
        (Some(_), Some(_)) => Err("after is not a string"),
    }
}

/// Why a page request is not answered with a page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageError {
    /// No snapshot at that revision: start again from the first page.
    Changed,
    /// The cursor names no row of the snapshot.
    Malformed,
}

impl PageError {
    pub fn code(self) -> &'static str {
        match self {
            Self::Changed => "changed",
            Self::Malformed => "malformed",
        }
    }
}

struct Snapshot {
    rev: u32,
    rows: Vec<Row>,
    until: Instant,
}

/// The snapshots of one Hub, one per connection (§6.4): a first page keeps
/// the offer's rows at that revision; a later page with that `rev` is
/// answered from them whether or not the offer changed since, and extends
/// their life to 30 s from its arrival. A newer first page on the
/// connection replaces the snapshot; an expired one answers `changed`.
pub struct Snapshots {
    life: Duration,
    by_conn: HashMap<u64, Snapshot>,
}

impl Default for Snapshots {
    fn default() -> Self {
        Self { life: Duration::from_millis(CATALOG_SNAPSHOT_MS), by_conn: HashMap::new() }
    }
}

impl Snapshots {
    /// A first page: `rows` (already `rows()`) become the connection's snapshot.
    pub fn first(&mut self, conn: u64, rows: Vec<Row>, rev: u32, payload_limit: u32, now: Instant) -> Page {
        self.prune(now);
        let page = page(&rows, 0, rev, payload_limit);
        self.by_conn.insert(conn, Snapshot { rev, rows, until: now + self.life });
        page
    }

    /// A later page.
    pub fn after(&mut self, conn: u64, after: &str, rev: u32, payload_limit: u32, now: Instant) -> Result<Page, PageError> {
        self.prune(now);
        let snapshot = self.by_conn.get_mut(&conn).filter(|snapshot| snapshot.rev == rev).ok_or(PageError::Changed)?;
        let index = snapshot.rows.iter().position(|row| row.id == after).ok_or(PageError::Malformed)?;
        snapshot.until = now + self.life;
        Ok(page(&snapshot.rows, index + 1, rev, payload_limit))
    }

    /// Drops the snapshots whose life ended: a connection that went away
    /// leaves nothing behind for longer than 30 s.
    fn prune(&mut self, now: Instant) {
        self.by_conn.retain(|_, snapshot| now < snapshot.until);
    }

    pub fn len(&self) -> usize {
        self.by_conn.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_conn.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pocket_link::consts::{MIN_PAYLOAD, TCP_PAYLOAD};
    use serde_json::json;

    fn vectors() -> Value {
        serde_json::from_str(include_str!("../../../vendor/pocketjs/tests/vectors/pocket-link-2/messages.json")).unwrap()
    }

    /// `vectors.ts`'s offer: 70 apps in reverse order of their ids.
    fn offer() -> Vec<Row> {
        (0..70)
            .map(|i| Row {
                id: format!("dev.example.app-{:02}", 69 - i),
                title: format!("App {}", 69 - i),
                version: "1.2.3".into(),
                summary: "A summary of the applet.".into(),
                output: format!("app-{}", 69 - i),
                hash: format!("{i:016x}"),
                bytes: 400_000 + i as u64,
            })
            .collect()
    }

    #[test]
    fn the_catalog_vectors() {
        let vectors = vectors();
        let expected = &vectors["catalog"];
        let rows = rows(offer());
        assert_eq!(rows.len() as u64, expected["served"].as_u64().unwrap());
        assert_eq!(rows[0].id, expected["firstServed"]);
        assert_eq!(rows[63].id, expected["lastServed"]);

        // A row at every limit fits the smallest payload limit.
        let at_limits: Row = serde_json::from_value::<serde_json::Map<String, Value>>(expected["rowAtLimits"]["json"].clone())
            .map(|m| Row {
                id: m["id"].as_str().unwrap().into(),
                title: m["title"].as_str().unwrap().into(),
                version: m["version"].as_str().unwrap().into(),
                summary: m["summary"].as_str().unwrap().into(),
                output: m["output"].as_str().unwrap().into(),
                hash: m["hash"].as_str().unwrap().into(),
                bytes: m["bytes"].as_u64().unwrap(),
            })
            .unwrap();
        assert_eq!(json_bytes(&at_limits) as u64, expected["rowAtLimits"]["bytes"].as_u64().unwrap());
        assert_eq!(json_bytes(&Page { apps: vec![at_limits.clone()], rev: u32::MAX, more: false }) as u64, expected["rowAtLimits"]["pageBytes"].as_u64().unwrap());
        assert_eq!(rows_of(vec![at_limits.clone()]).len(), 1, "a row at every limit is kept");

        // The fetch at the smallest payload limit, the offer changing after page 2.
        let fetch = &expected["fetch"][0];
        let mut snapshots = Snapshots::default();
        let start = Instant::now();
        let mut now = start;
        let mut answer = snapshots.first(1, rows.clone(), 1, MIN_PAYLOAD, now);
        let (mut pages, mut count, mut last) = (1, answer.apps.len(), answer.apps.last().unwrap().id.clone());
        let first = answer.apps[0].id.clone();
        // The offer changes after page 2 (a newer revision exists): the
        // snapshot still answers its own `rev`.
        while answer.more {
            now += Duration::from_secs(1);
            answer = snapshots.after(1, &last, answer.rev, MIN_PAYLOAD, now).unwrap();
            pages += 1;
            count += answer.apps.len();
            last = answer.apps.last().unwrap().id.clone();
            assert!(json_bytes(&answer) <= MIN_PAYLOAD as usize);
        }
        assert_eq!(json!({"pages": pages, "rows": count, "firstId": first, "lastId": last}), json!({"pages": fetch["pages"], "rows": fetch["rows"], "firstId": fetch["firstId"], "lastId": fetch["lastId"]}));

        // 31 s later the snapshot is gone: `changed`.
        let mut late = Snapshots::default();
        let page = late.first(1, rows.clone(), 1, MIN_PAYLOAD, start);
        let gone = late.after(1, &page.apps.last().unwrap().id, 1, MIN_PAYLOAD, start + Duration::from_secs(31));
        assert_eq!(gone.unwrap_err().code(), expected["after31s"]["error"]);
        // An `after` naming no row: `malformed`.
        let mut unknown = Snapshots::default();
        unknown.first(1, rows.clone(), 1, TCP_PAYLOAD, start);
        assert_eq!(unknown.after(1, "dev.example.none", 1, TCP_PAYLOAD, start).unwrap_err().code(), expected["unknownAfter"]["error"]);
    }

    fn rows_of(offer: Vec<Row>) -> Vec<Row> {
        rows(offer)
    }

    #[test]
    fn cursors_and_snapshot_life() {
        let parse = |value: Value| cursor(value.as_object().unwrap());
        assert_eq!(parse(json!({"t": "catalog"})), Ok(Cursor::First));
        assert_eq!(parse(json!({"after": "a", "rev": 3})), Ok(Cursor::After { after: "a".into(), rev: 3 }));
        assert!(parse(json!({"after": "a"})).is_err());
        assert!(parse(json!({"rev": 3})).is_err());
        assert!(parse(json!({"after": 1, "rev": 3})).is_err());
        assert!(parse(json!({"after": "a", "rev": -1})).is_err());
        assert!(parse(json!({"after": "a", "rev": 4_294_967_296u64})).is_err());
        assert!(parse(json!({"after": "x".repeat(129), "rev": 1})).is_err());

        let rows = rows(offer());
        let start = Instant::now();
        let mut snapshots = Snapshots::default();
        let first = snapshots.first(7, rows.clone(), 5, MIN_PAYLOAD, start);
        // Each page request extends the life to 30 s from its arrival.
        let second = snapshots.after(7, &first.apps.last().unwrap().id, 5, MIN_PAYLOAD, start + Duration::from_secs(20)).unwrap();
        assert!(snapshots.after(7, &second.apps.last().unwrap().id, 5, MIN_PAYLOAD, start + Duration::from_secs(45)).is_ok());
        // Another revision, another connection: `changed`.
        assert_eq!(snapshots.after(7, &first.apps[0].id, 6, MIN_PAYLOAD, start + Duration::from_secs(46)), Err(PageError::Changed));
        assert_eq!(snapshots.after(8, &first.apps[0].id, 5, MIN_PAYLOAD, start + Duration::from_secs(46)), Err(PageError::Changed));
        // A newer first page replaces the snapshot.
        snapshots.first(7, rows.clone(), 6, MIN_PAYLOAD, start + Duration::from_secs(47));
        assert_eq!(snapshots.after(7, &first.apps[0].id, 5, MIN_PAYLOAD, start + Duration::from_secs(47)), Err(PageError::Changed));
        // Expired snapshots are dropped.
        snapshots.first(9, rows, 6, MIN_PAYLOAD, start + Duration::from_secs(48));
        assert_eq!(snapshots.len(), 2);
        let _ = snapshots.after(9, "x", 6, MIN_PAYLOAD, start + Duration::from_secs(200));
        assert!(snapshots.is_empty());
    }

    #[test]
    fn limits_cut_and_leave_out() {
        let row = |id: &str, output: &str, version: &str| Row { id: id.into(), title: "é".repeat(40), version: version.into(), summary: "\u{1}".repeat(30), output: output.into(), hash: "0".repeat(16), bytes: 1 };
        let kept = rows(vec![row("b", "o", "1.0.0"), row("a", &"o".repeat(65), "1.0.0"), row("c", "o", &"1".repeat(33)), row("b", "second", "1"), row(&"i".repeat(129), "o", "1")]);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].output, "o", "the first row of an id wins");
        assert_eq!(encoded_bytes(&kept[0].title), 64);
        assert_eq!(kept[0].title.chars().count(), 32, "cut on a code point");
        // \u0001 is 6 encoded bytes: 20 of them fit 120.
        assert_eq!(kept[0].summary.chars().count(), 20);
    }
}
