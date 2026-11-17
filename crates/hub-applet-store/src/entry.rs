//! One app of the store's catalog as `GET /v1/apps` answers it
//! (pocket-nexus/applets `store/src/index.ts` `entry`), the message its
//! publisher signed (`store/src/shared.ts` `signedMessage`), and the store's
//! order of versions (`versionOrder`).

use base64::Engine;
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::engine::DecodePaddingMode;
use serde::{Deserialize, Serialize};

/// The first line of every signed message (`SIGNED_PREFIX`).
pub const SIGNED_PREFIX: &str = "omarchy-pocket-store/1";

/// The store's official publisher key, base64url of the raw ed25519 public
/// key, as the store's Worker is deployed with it (`store/wrangler.jsonc`
/// `OFFICIAL_KEY`). Hub decides `official` itself by this key, not by the
/// store's own `official` member.
pub const OFFICIAL_KEY: &str = "tF8Q_hSXiYfxDdlQrpJfleDcKmkZPtQ20G_SNjtOquQ";

/// App ids only the official key may publish (`RESERVED_ID`).
pub const RESERVED_ID_PREFIX: &str = "dev.pocket-nexus.";

/// One app at its newest listed version. Members Hub does not use
/// (`profile`, `target`, `created`) are not kept.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub id: String,
    pub version: String,
    pub title: String,
    pub summary: String,
    pub author: String,
    /// The publisher's ed25519 public key, base64url.
    pub key: String,
    #[serde(default)]
    pub services: Vec<String>,
    #[serde(default)]
    pub hosts: Vec<String>,
    /// SHA-256 of the package, 64 lowercase hex digits.
    pub package: String,
    /// SHA-256 of the 128 px PNG icon.
    pub icon: String,
    /// The package's size.
    pub bytes: u64,
    /// The publisher's ed25519 signature of `signed_message`, base64url.
    pub signature: String,
    /// The target of every variant: the devices it runs on.
    pub targets: Vec<String>,
    /// SHA-256 of the version's sample data (`/v1/mocks/<sha256>`), which the
    /// store's player runs it on; absent when it has none. The store's
    /// reviewer adds it (`/v1/admin/mock`): the publisher's signature does
    /// not cover it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mock: Option<String>,
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

impl Entry {
    /// The store's own rules on the members Hub relies on: an entry that
    /// breaks one is not the store's answer.
    pub fn check(&self) -> Result<(), String> {
        if self.id.is_empty() {
            return Err("an app without an id".into());
        }
        if !is_sha256(&self.package) || !is_sha256(&self.icon) || self.mock.as_deref().is_some_and(|mock| !is_sha256(mock)) {
            return Err(format!("{}: a file is not named by a SHA-256", self.id));
        }
        if self.bytes == 0 {
            return Err(format!("{}: a package of 0 bytes", self.id));
        }
        Ok(())
    }

    /// The bytes the publisher signed: the prefix, id, version, package
    /// hash, icon hash, author and summary, one per line, without a final
    /// newline (`signedMessage`).
    pub fn signed_message(&self) -> String {
        [SIGNED_PREFIX, &self.id, &self.version, &self.package, &self.icon, &self.author, &self.summary].join("\n")
    }

    /// Checks the publisher's signature over `signed_message` with the
    /// entry's own key, and that an id reserved for the official key comes
    /// from it. Proves that the holder of `key` published these id, version,
    /// files, author and summary; which key may publish an id is the store's
    /// record (the first key to publish an id owns it), and only the
    /// official key is known to Hub.
    pub fn verify(&self, official_key: &str) -> Result<(), String> {
        if self.id.starts_with(RESERVED_ID_PREFIX) && self.key != official_key {
            return Err(format!("{} is an id of the official publisher, and another key published it", self.id));
        }
        let key: [u8; 32] = decode(&self.key).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{}: the publisher's key is not an ed25519 key", self.id))?;
        let signature: [u8; 64] = decode(&self.signature).and_then(|bytes| bytes.try_into().ok()).ok_or_else(|| format!("{}: the signature is not an ed25519 signature", self.id))?;
        let key = ed25519_dalek::VerifyingKey::from_bytes(&key).map_err(|_| format!("{}: the publisher's key is not a valid ed25519 key", self.id))?;
        key.verify_strict(self.signed_message().as_bytes(), &ed25519_dalek::Signature::from_bytes(&signature))
            .map_err(|_| format!("the publisher's signature of {} {} does not verify", self.id, self.version))
    }

    pub fn official(&self, official_key: &str) -> bool {
        self.key == official_key
    }

    pub fn runs_on(&self, target: &str) -> bool {
        !target.is_empty() && self.targets.iter().any(|t| t == target)
    }
}

/// base64url as the store writes it (`base64url`: no padding); padding is
/// accepted too, as the store's `atob` accepts it.
fn decode(text: &str) -> Option<Vec<u8>> {
    const ENGINE: GeneralPurpose = GeneralPurpose::new(&base64::alphabet::URL_SAFE, GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent));
    ENGINE.decode(text.as_bytes()).ok()
}

/// base64url without padding, as the store writes keys and signatures.
pub fn encode(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A version's order as the store sorts versions (`versionOrder`):
/// `major.minor.patch` of 1 to 4 digits each, as one number. None for a
/// version of any other form.
pub fn version_order(version: &str) -> Option<u64> {
    let mut parts = version.split('.');
    let mut order = 0u64;
    for scale in [100_000_000u64, 10_000, 1] {
        let part = parts.next()?;
        if part.is_empty() || part.len() > 4 || !part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        order += part.parse::<u64>().ok()? * scale;
    }
    parts.next().is_none().then_some(order)
}

/// Whether the store's version is an update for a device holding
/// `installed`: the store's version orders above it. A device version that
/// is not `major.minor.patch` (a build pushed by hand under another scheme)
/// orders below every store version, which always has that form; an equal
/// version is no update whatever its hash.
pub fn is_update(store: &str, installed: &str) -> bool {
    match (version_order(store), version_order(installed)) {
        (Some(store), Some(installed)) => store > installed,
        (Some(_), None) => true,
        (None, _) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_order_as_the_store_orders_them() {
        assert_eq!(version_order("1.2.3"), Some(100_020_003));
        assert_eq!(version_order("0.3.0"), Some(30_000));
        assert_eq!(version_order("1.2"), None);
        assert_eq!(version_order("1.2.3.4"), None);
        assert_eq!(version_order("12345.0.0"), None);
        assert_eq!(version_order("1.2.x"), None);
        assert!(is_update("0.3.0", "0.2.9"));
        assert!(is_update("0.10.0", "0.9.0"));
        assert!(!is_update("0.3.0", "0.3.0"));
        assert!(!is_update("0.3.0", "1.0.0"));
        assert!(is_update("0.3.0", "dev"));
    }

    /// The live store's entries kept in the fixtures carry the official
    /// key's real signatures.
    #[test]
    fn the_official_signatures_of_the_fixtures_verify() {
        let text = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/apps.json")).unwrap();
        let apps: serde_json::Value = serde_json::from_str(&text).unwrap();
        let entries: Vec<Entry> = serde_json::from_value(apps["apps"].clone()).unwrap();
        assert_eq!(entries.len(), 2);
        for entry in &entries {
            entry.check().unwrap();
            assert!(entry.official(OFFICIAL_KEY));
            entry.verify(OFFICIAL_KEY).unwrap();
            let mut changed = entry.clone();
            changed.summary.push('!');
            assert!(changed.verify(OFFICIAL_KEY).is_err(), "a changed summary is not what was signed");
            let mut other = entry.clone();
            other.package.replace_range(0..1, if other.package.starts_with('0') { "1" } else { "0" });
            assert!(other.verify(OFFICIAL_KEY).is_err());
        }
        // A reserved id from another key.
        let mut foreign = entries[0].clone();
        foreign.key = encode(&[7u8; 32]);
        assert!(foreign.verify(OFFICIAL_KEY).unwrap_err().contains("official publisher"));
    }
}
