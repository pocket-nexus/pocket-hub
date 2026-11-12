//! What Hub reads out of a `.pocket` (PocketJS `contracts/spec/pocket-package.ts`)
//! through the `pocket-package` crate, the format's reader in Rust
//! (link specification §12.3 item 5): its variants per target with each
//! variant's identity (id, title, output name), the manifest's version, and
//! the footer hash a device checks an install against (§6.4).

use pocket_package::Package;

/// One variant: the build for one device target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariantFacts {
    pub target: String,
    pub host_abi: u32,
    /// The identity section (kind 1): the launcher's key once installed.
    pub id: String,
    pub title: String,
    pub output: String,
}

/// A package's facts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageFacts {
    /// The file's size.
    pub bytes: u64,
    /// The footer hash (FNV-1a64 of every byte before it), 16 hex digits.
    pub hash: String,
    /// The manifest's `id`, `title` and `version` (`pocket.app.json`).
    pub id: String,
    pub title: String,
    pub version: String,
    pub variants: Vec<VariantFacts>,
}

impl PackageFacts {
    pub fn variant(&self, target: &str) -> Option<&VariantFacts> {
        self.variants.iter().find(|variant| variant.target == target)
    }
}

/// Reads a package whole: the layout, the footer hash and every variant's
/// section hash are checked (`Package::parse` without `skip_hash`), and
/// every variant must carry an identity.
pub fn read(bytes: &[u8]) -> Result<PackageFacts, String> {
    let package = Package::parse(bytes, false).map_err(|error| format!("not a valid .pocket ({error:?})"))?;
    let manifest: serde_json::Value = serde_json::from_slice(package.manifest()).map_err(|_| "the manifest is not JSON".to_owned())?;
    let text = |key: &str| manifest.get(key).and_then(serde_json::Value::as_str).unwrap_or_default().to_owned();
    let mut variants = Vec::with_capacity(package.variant_count());
    for index in 0..package.variant_count() {
        let variant = package.variant(index).map_err(|error| format!("variant {index} cannot be read ({error:?})"))?;
        let identity = variant
            .identity()
            .map_err(|error| format!("the {} variant's identity cannot be read ({error:?})", variant.target))?
            .ok_or_else(|| format!("the {} variant has no identity", variant.target))?;
        variants.push(VariantFacts {
            target: variant.target.to_owned(),
            host_abi: variant.host_abi,
            id: identity.id.to_owned(),
            title: identity.title.to_owned(),
            output: identity.output.to_owned(),
        });
    }
    let hash = package.package_hash().map_err(|error| format!("the footer cannot be read ({error:?})"))?;
    Ok(PackageFacts { bytes: bytes.len() as u64, hash: format!("{hash:016x}"), id: text("id"), title: text("title"), version: text("version"), variants })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
    }

    /// The real packages from the store: three variants each, the 3DS's
    /// identity, and the footer that `pocket_hub_core::ops::package_hash`
    /// (what `devices.push` sends) computes from the bytes.
    #[test]
    fn real_packages_read() {
        let hn = fixture("hn.pocket");
        let facts = read(&hn).unwrap();
        assert_eq!((facts.id.as_str(), facts.title.as_str(), facts.version.as_str()), ("dev.pocket-nexus.hn", "Hacker News", "0.3.0"));
        let targets: Vec<&str> = facts.variants.iter().map(|v| v.target.as_str()).collect();
        assert_eq!(targets, ["3ds-dev", "rg-ds-plus", "tab5-home"]);
        let three = facts.variant("3ds-dev").unwrap();
        assert_eq!((three.id.as_str(), three.output.as_str(), three.title.as_str(), three.host_abi), ("dev.pocket-nexus.hn", "hn-main", "Hacker News", 12));
        assert_eq!(facts.hash, pocket_hub_core::ops::package_hash(&hn).unwrap());
        assert_eq!(facts.bytes, 2_210_904);

        let youtube = read(&fixture("youtube.pocket")).unwrap();
        assert_eq!(youtube.id, "dev.pocket-nexus.youtube");
        assert!(youtube.variant("3ds-dev").is_some());

        let mut broken = hn.clone();
        broken[1000] ^= 1;
        assert!(read(&broken).is_err(), "a changed byte breaks the footer");
        assert!(read(b"not a package at all, not even close to it").is_err());
    }
}
