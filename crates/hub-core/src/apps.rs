//! §6.2 `runtime.apps`: the installed applets a device reports, assembled from
//! the pages of one report (`set`, `at`, `more`).

use pocket_link::LinkClose;
use pocket_link::consts::APPS_MAX_ROWS;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One row of a report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstalledApp {
    pub id: String,
    pub output: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<bool>,
}

/// One page as it arrived. A page whose fields are missing reads as a page
/// with a wrong `at`, which drops its set: the link does not close for a
/// device bug that an ordered carrier makes harmless (§6.2).
#[derive(Debug, Clone)]
pub struct AppsPage {
    pub set: u64,
    pub at: u64,
    pub apps: Vec<InstalledApp>,
    pub more: bool,
}

impl AppsPage {
    pub fn from_event(object: &serde_json::Map<String, Value>) -> Self {
        let number = |key: &str| object.get(key).and_then(Value::as_u64);
        let apps = object
            .get("apps")
            .and_then(Value::as_array)
            .map(|rows| rows.iter().filter_map(|row| serde_json::from_value::<InstalledApp>(row.clone()).ok()).collect())
            .unwrap_or_default();
        Self { set: number("set").unwrap_or(0), at: number("at").unwrap_or(u64::MAX), apps, more: object.get("more").and_then(Value::as_bool).unwrap_or(false) }
    }
}

/// Hub's side of the paging rules. It keeps the highest `set` seen on the
/// connection and replaces the set it holds only when the page with `more:
/// false` arrives after every page of that set from `at` 0, in order.
#[derive(Debug, Default)]
pub struct AppsAssembler {
    highest: u64,
    partial: Option<Vec<InstalledApp>>,
    /// The highest set was completed or dropped: its later pages are ignored.
    settled: bool,
}

impl AppsAssembler {
    /// The new complete set, or None while the earlier one stands. A report
    /// over 64 rows closes the link with `protocol`.
    pub fn page(&mut self, page: AppsPage) -> Result<Option<Vec<InstalledApp>>, LinkClose> {
        if page.set < self.highest || (page.set == self.highest && (self.settled || self.partial.is_none())) {
            return Ok(None);
        }
        if page.set > self.highest {
            self.highest = page.set;
            self.partial = Some(Vec::new());
            self.settled = false;
        }
        let partial = self.partial.as_mut().expect("a partial set is held");
        if page.at != partial.len() as u64 {
            self.partial = None;
            self.settled = true;
            return Ok(None);
        }
        partial.extend(page.apps);
        if partial.len() > APPS_MAX_ROWS {
            return Err(LinkClose::protocol("runtime.apps: more than 64 rows"));
        }
        if page.more {
            return Ok(None);
        }
        self.settled = true;
        Ok(self.partial.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(n: usize) -> Vec<InstalledApp> {
        (0..n).map(|i| InstalledApp { id: format!("app.{i}"), output: format!("a{i}"), version: "1".into(), hash: "0".repeat(16), home: None }).collect()
    }

    /// `messages.json` `runtimeApps.assembler`: each page and the size of the
    /// set it completes, or null.
    #[test]
    fn assembler_vectors() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../vendor/pocketjs/tests/vectors/pocket-link-2/messages.json");
        let vectors: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        let cases = vectors["runtimeApps"]["assembler"].as_array().unwrap();
        assert!(!cases.is_empty());
        for case in cases {
            let mut assembler = AppsAssembler::default();
            let pages = case["pages"].as_array().unwrap();
            let complete = case["complete"].as_array().unwrap();
            for (page, expected) in pages.iter().zip(complete) {
                let got = assembler
                    .page(AppsPage { set: page["set"].as_u64().unwrap(), at: page["at"].as_u64().unwrap(), apps: rows(page["rows"].as_u64().unwrap() as usize), more: page["more"].as_bool().unwrap() })
                    .unwrap();
                assert_eq!(got.map(|set| set.len() as u64), expected.as_u64(), "{}", case["name"]);
            }
        }
    }

    #[test]
    fn more_than_64_rows_closes_with_protocol() {
        let mut assembler = AppsAssembler::default();
        assert!(assembler.page(AppsPage { set: 1, at: 0, apps: rows(40), more: true }).unwrap().is_none());
        assert!(assembler.page(AppsPage { set: 1, at: 40, apps: rows(25), more: false }).is_err());
    }
}
