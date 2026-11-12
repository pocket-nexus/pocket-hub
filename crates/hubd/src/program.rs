//! The device's own program (§2 `Device.program`): the newest release Hub
//! knows, per device family.
//!
//! **One look serves every device of a family**: all 3DSes run the same
//! Pocket Shell, so the release is looked up once for the family and each
//! Device shows it beside the version it reports itself. A look starts when
//! a device links, unless one runs or one started less than the interval
//! ago (6 hours): a device that drops and relinks every few seconds does not
//! send a request to the release source each time, and an unauthenticated
//! client of GitHub's API has 60 requests an hour. **The interval counts
//! from a look's start whatever its end**, so a source that fails is not
//! asked again at every link either.
//!
//! A look that fails leaves `latest` as it was and is logged; it is no
//! client's error. A look that finds a version other than the one held
//! changes every Device of the family, which the core publishes.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use pocket_hub_setup::DeviceFamily;

#[derive(Debug, Default)]
struct Look {
    /// The newest release's version; None until a look found one.
    latest: Option<String>,
    /// When the last look started.
    started: Option<Instant>,
    running: bool,
}

pub struct Releases {
    interval: Duration,
    looks: Mutex<HashMap<DeviceFamily, Look>>,
}

impl Releases {
    pub fn new(interval: Duration) -> Self {
        Self { interval, looks: Mutex::new(HashMap::new()) }
    }

    /// The newest release Hub knows for the family.
    pub fn latest(&self, family: DeviceFamily) -> Option<String> {
        self.looks.lock().unwrap().get(&family).and_then(|look| look.latest.clone())
    }

    /// Takes the family's look when it is due (none runs, none started
    /// within the interval); the caller then looks and calls `finish`.
    pub fn begin(&self, family: DeviceFamily) -> bool {
        let mut looks = self.looks.lock().unwrap();
        let look = looks.entry(family).or_default();
        if look.running || look.started.is_some_and(|started| started.elapsed() < self.interval) {
            return false;
        }
        look.running = true;
        look.started = Some(Instant::now());
        true
    }

    /// The look ended.
    pub fn finish(&self, family: DeviceFamily) {
        if let Some(look) = self.looks.lock().unwrap().get_mut(&family) {
            look.running = false;
        }
    }

    /// Hub found the newest release's version (a look, or an update that
    /// asked for the latest release). True when it changed.
    pub fn set(&self, family: DeviceFamily, version: Option<String>) -> bool {
        let mut looks = self.looks.lock().unwrap();
        let look = looks.entry(family).or_default();
        let changed = look.latest != version;
        look.latest = version;
        changed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn one_look_per_interval_and_family() {
        let family = DeviceFamily::Nintendo3ds;
        let releases = Releases::new(Duration::from_secs(3600));
        assert_eq!(releases.latest(family), None);
        assert!(releases.begin(family));
        assert!(!releases.begin(family), "one runs");
        releases.finish(family);
        assert!(!releases.begin(family), "one started within the interval, failed or not");
        assert!(releases.set(family, Some("2026.11.11".into())));
        assert!(!releases.set(family, Some("2026.11.11".into())));
        assert_eq!(releases.latest(family).as_deref(), Some("2026.11.11"));

        let eager = Releases::new(Duration::ZERO);
        assert!(eager.begin(family));
        assert!(!eager.begin(family), "never two at once");
        eager.finish(family);
        assert!(eager.begin(family));
    }
}
