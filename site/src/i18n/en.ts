// The page's words in English. Each language's file has exactly these keys
// (index.ts's Wording holds it to that).
//
// A {name} is filled in where the message is drawn: with a value (fill() in
// format.ts) or with a piece of the page (Sentence.vue).
//
// A platform (src/release.ts PLATFORMS) has its words under `platforms`. A
// device is named by its maker's name for it, in every language (src/devices.ts).
const en = {
  title: "Pocket Hub",
  language: "Language",
  header: {
    home: "Pocket Hub",
    store: "Applets",
  },
  lede: "Install Pocket applets on your devices and keep them up to date.",
  release: {
    facts: "v{version} · {size}",
    notes: "Release notes",
    checksum: "SHA-256",
  },
  platforms: {
    macos: {
      download: "Download for macOS",
      requires: "Apple silicon · macOS 15 or later",
    },
  },
  devices: {
    title: "Supported devices",
  },
  footer: {
    credit: "© 2026 {nexus}. Free software under the GPL-3.0.",
    source: "Source code",
  },
};

export default en;
