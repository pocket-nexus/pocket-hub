// The release the page offers is read from GitHub's answer, and an answer
// that is not a release of docs/CONTRIBUTING.md "Releasing"'s shape stops the
// build with one line naming what is wrong.
import { describe, expect, test } from "bun:test";
import { readRelease } from "../src/release.ts";

/** GitHub's answer for v0.1.0, the parts the page reads (and one it does not). */
function answer() {
  return {
    tag_name: "v0.1.0",
    html_url: "https://github.com/pocket-nexus/pocket-hub/releases/tag/v0.1.0",
    published_at: "2026-11-12T13:48:18Z",
    draft: false,
    assets: [
      {
        name: "Pocket-Hub-0.1.0-arm64.zip",
        size: 7438342,
        digest: "sha256:8ce9a904bd9fde7e6e5a7a7adbf7af66b9af1a9f266e486faceae355a5488f58",
        browser_download_url:
          "https://github.com/pocket-nexus/pocket-hub/releases/download/v0.1.0/Pocket-Hub-0.1.0-arm64.zip",
      },
    ],
  };
}

describe("readRelease", () => {
  test("a release as the release steps make it", () => {
    expect(readRelease(answer())).toEqual({
      version: "0.1.0",
      notes: "https://github.com/pocket-nexus/pocket-hub/releases/tag/v0.1.0",
      downloads: [{
        platform: "macos",
        archive: "Pocket-Hub-0.1.0-arm64.zip",
        url: "https://github.com/pocket-nexus/pocket-hub/releases/download/v0.1.0/Pocket-Hub-0.1.0-arm64.zip",
        size: 7438342,
        sha256: "8ce9a904bd9fde7e6e5a7a7adbf7af66b9af1a9f266e486faceae355a5488f58",
      }],
    });
  });

  test("other assets beside the archive are passed over", () => {
    const release = answer();
    release.assets.unshift({ ...release.assets[0]!, name: "notes.txt" });
    expect(readRelease(release).downloads.map((each) => each.archive)).toEqual(["Pocket-Hub-0.1.0-arm64.zip"]);
  });

  test("a tag that is not v<major>.<minor>.<patch>", () => {
    for (const tag of ["0.1.0", "v0.1", "v0.1.0-beta", "v0.1.0<b>", undefined]) {
      expect(() => readRelease({ ...answer(), tag_name: tag })).toThrow("the latest release's tag is");
    }
  });

  test("a release without the archive", () => {
    const release = answer();
    release.assets[0]!.name = "Pocket-Hub-0.1.0-x86_64.zip";
    expect(() => readRelease(release)).toThrow("release v0.1.0 has no Pocket-Hub-0.1.0-arm64.zip");
    expect(() => readRelease({ ...answer(), assets: undefined })).toThrow("has no Pocket-Hub-0.1.0-arm64.zip");
  });

  test("an archive anywhere but where GitHub keeps this release's", () => {
    const release = answer();
    release.assets[0]!.browser_download_url = "https://example.com/Pocket-Hub-0.1.0-arm64.zip";
    expect(() => readRelease(release)).toThrow("Pocket-Hub-0.1.0-arm64.zip of v0.1.0 is at");
  });

  test("an archive without a size or a SHA-256", () => {
    const sized = answer();
    sized.assets[0]!.size = 0;
    expect(() => readRelease(sized)).toThrow("has the size 0");
    for (const digest of [undefined, "md5:abc", "sha256:XYZ"]) {
      const release = answer();
      (release.assets[0] as { digest?: string }).digest = digest;
      expect(() => readRelease(release)).toThrow("not sha256:<hex>");
    }
  });

  test("a release without a page of its own", () => {
    expect(() => readRelease({ ...answer(), html_url: "https://example.com/" })).toThrow("release v0.1.0's page is");
  });
});
