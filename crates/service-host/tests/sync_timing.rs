//! The time of one `set` under each way of syncing a commit
//! (SERVICE-PROTOCOL.md §7, "Durable"): today's `synchronous = FULL` alone,
//! with `checkpoint_fullfsync`, and with `fullfsync` as well. A measurement,
//! not a check: run with `--ignored --nocapture`; `PH_SYNC_RUNS` sets the
//! number of sets per case (500 by default).

mod common;

use std::time::Instant;

/// Median and 99th percentile, in milliseconds.
fn summary(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let at = |q: f64| samples[((samples.len() - 1) as f64 * q).round() as usize];
    (at(0.5), at(0.99))
}

#[test]
#[ignore = "a measurement; run with --ignored --nocapture"]
fn the_time_of_one_set_by_sync_setting() {
    let runs: usize = std::env::var("PH_SYNC_RUNS").ok().and_then(|runs| runs.parse().ok()).unwrap_or(500);
    for (name, pragmas) in [
        ("(a) synchronous = FULL", ""),
        ("(b) + checkpoint_fullfsync", "PRAGMA checkpoint_fullfsync = ON;"),
        ("(c) + fullfsync", "PRAGMA checkpoint_fullfsync = ON; PRAGMA fullfsync = ON;"),
    ] {
        for size in [16usize, 64 * 1024] {
            let home = common::temp_home();
            let folder = home.path().join("service-store/dev.x@dev");
            drop(service_host::store::Store::open(&folder).unwrap());
            let store = service_host::store::Store::open_with(&folder, pragmas).unwrap();
            // Each value differs from the one it replaces: SQLite writes no
            // page, and syncs nothing, for a row replaced by the same bytes.
            let mut value = vec![7u8; size];
            let flags = (store.pragma("synchronous").unwrap(), store.pragma("fullfsync").unwrap(), store.pragma("checkpoint_fullfsync").unwrap());
            let mut samples = Vec::with_capacity(runs);
            for i in 0..runs {
                let key = format!("k{}", i % 64);
                value[..8].copy_from_slice(&(i as u64).to_le_bytes());
                let started = Instant::now();
                store.set(key.as_bytes(), &value, 1 << 28).unwrap();
                samples.push(started.elapsed().as_secs_f64() * 1000.0);
            }
            let (median, p99) = summary(samples);
            eprintln!("{name} {flags:?}, {size} B: median {median:.2} ms, p99 {p99:.2} ms ({runs} sets)");
        }
    }
}
