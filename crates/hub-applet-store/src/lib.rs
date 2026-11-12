//! Pocket Hub as a client of the applet store (pocket-nexus/applets,
//! `docs/applets.md`; served at applets.pocket.nexus), and the link's
//! `catalog` and `install` served from it (link specification §6.4).
//! Not to be confused with `pocket-hub-store`, Hub's own records of paired
//! devices.
//!
//! - `entry`: one app of the store's catalog, its publisher's signature and
//!   the store's version order.
//! - `package`: what Hub reads out of a `.pocket` (variants, identity,
//!   footer hash) through the `pocket-package` crate.
//! - `client`: `AppletStore`, the catalog with its disk cache, verified
//!   packages and icons in a content-addressed cache, and the link's offer.
//! - `pager`: catalog rows, pages and per-connection snapshots (§6.4).
//! - `link`: `LinkCatalog`, the `DeviceRequests` provider of `catalog` and
//!   `install`.
//! - `test_support` (feature `test-support`): a fake store on loopback with
//!   fault injection, a package builder and test keys.

pub mod client;
pub mod entry;
pub mod link;
pub mod package;
pub mod pager;
#[cfg(feature = "test-support")]
pub mod test_support;

pub use client::{AppletStore, Catalog, DEFAULT_URL, StoreConfig, StoreError, StoreEvent};
pub use entry::{Entry, OFFICIAL_KEY, is_update, version_order};
pub use link::{LinkCatalog, LinkConfig};
pub use package::{PackageFacts, VariantFacts};
