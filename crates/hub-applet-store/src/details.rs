//! A package's details as the store answers them at `GET /v1/details/<sha256>`
//! (pocket-nexus/applets `store/src/wire.ts` `AppDetails`): what the package's
//! manifest and directory say about the app, read out by the store
//! (`store/src/details.ts` `detailsOf`), the one place that reads them. Hub
//! asks for them and parses no manifest of its own (docs/CONTROL-API.md §2
//! StoreDetails).
//!
//! The publisher's signature covers the entry (`Entry::signed_message`), not
//! these: they are the store's reading of a package it holds.

use serde::{Deserialize, Serialize};

/// An answer over this is not the store's: the details of a package of three
/// builds take 2 KB.
pub const DETAILS_MAX_BYTES: usize = 1024 * 1024;

/// The details. Members Hub does not use (a build's `profile`, `hash` and
/// `sections`) are not kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    /// The devices the package was built for, in the order the store names
    /// devices; one it has no name for comes last.
    pub devices: Vec<DetailsDevice>,
    /// The capabilities it needs of a device, by id.
    pub needs: Vec<String>,
    /// The ones it uses where a device has them.
    pub uses: Vec<String>,
    /// Absent when the package names no surface.
    pub surface: Option<Surface>,
    pub framework: Option<String>,
    /// The applet API's version.
    pub api: Option<u32>,
}

/// A device a package was built for (`Device`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DetailsDevice {
    /// The target id its variant names: `rg-ds-plus`.
    pub target: String,
    /// The store's name for the device, or the target id of one it has no name for.
    pub name: String,
    /// Its build in the package; `/v1/details` always names one.
    pub build: Option<Build>,
}

/// A build (`PackageVariant`), of which Hub keeps the host ABI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Build {
    pub host_abi: u32,
}

/// The surface an app lays itself out for (`Surface`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Surface {
    /// A touch surface of its own, not the device's screen.
    pub touch: bool,
    /// The sizes it is laid out for, `[width, height]` in points, when it names both.
    pub size: Option<SurfaceSize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SurfaceSize {
    pub min: [u32; 2],
    pub max: [u32; 2],
}

/// Reads an answer of `/v1/details`: one that does not have the shape the
/// store states (`wire.ts`) is not the store's answer.
pub fn parse(body: &[u8]) -> Result<Details, String> {
    serde_json::from_slice(body).map_err(|error| format!("its answer is not a package's details ({error})"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The live store's answers for the two packages kept in the fixtures.
    #[test]
    fn the_live_answers_read() {
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures");
        let hn = parse(&std::fs::read(format!("{dir}/hn.details.json")).unwrap()).unwrap();
        let devices: Vec<(&str, &str, Option<u32>)> = hn.devices.iter().map(|d| (d.target.as_str(), d.name.as_str(), d.build.as_ref().map(|b| b.host_abi))).collect();
        assert_eq!(devices, [("rg-ds-plus", "Omarchy Pocket", Some(12)), ("tab5-home", "M5Stack Tab5", Some(12)), ("3ds-dev", "Nintendo 3DS", Some(12))]);
        assert_eq!(hn.needs, ["text.glyphs.baked", "input.buttons", "display.auxiliary", "input.touch.auxiliary", "net.http"]);
        assert_eq!(hn.uses, ["input.analog.left", "system.agent"]);
        assert_eq!(hn.surface, Some(Surface { touch: true, size: Some(SurfaceSize { min: [320, 240], max: [640, 720] }) }));
        assert_eq!((hn.framework.as_deref(), hn.api), (Some("solid"), Some(1)));

        let youtube = parse(&std::fs::read(format!("{dir}/youtube.details.json")).unwrap()).unwrap();
        assert!(youtube.uses.iter().any(|id| id == "media.playback"));
    }

    /// The optional members may be absent, as the store leaves them out when
    /// the manifest names none; the lists may not.
    #[test]
    fn the_least_the_store_answers() {
        let least = parse(br#"{"devices":[{"target":"3ds-dev","name":"Nintendo 3DS"}],"needs":[],"uses":[]}"#).unwrap();
        assert_eq!((least.surface, least.framework, least.api, least.devices[0].build.clone()), (None, None, None, None));
        assert!(parse(br#"{"devices":[],"needs":[]}"#).is_err());
        assert!(parse(br#"{"error":"no listed version has that file"}"#).is_err());
        assert!(parse(b"<html>").is_err());
    }
}
