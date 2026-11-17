// The release the page offers: GitHub's latest release of this repository,
// read when the page is built (vite.config.ts hands it to the page as the
// module virtual:release). A release is what docs/CONTRIBUTING.md
// "Releasing" makes: a tag v<version> and an archive for each platform Hub is
// built for, named as PLATFORMS says. Today that is macOS alone: the disk
// image Pocket-Hub-<version>-arm64.dmg, as app/scripts/package.sh names it
// (the zip beside it is for what unpacks the app by itself, not for a
// visitor). A platform is added here with its archive's name, and in the
// page's words.
//
// The page links each archive itself, so a visitor's click is the download.
// GitHub's /releases/latest/download/<name> would follow the latest release
// without a build, but only for an archive whose name has no version in it.
//
// A release that is not of that shape stops the build with one line saying
// what is wrong; the page is then not deployed, and the one that is live
// keeps offering the release it was built from.

export const REPOSITORY = "pocket-nexus/pocket-hub";

/** The platforms Hub is built for, each with its archive's name in a release
 *  of `version`. Every release carries every one of them. */
export const PLATFORMS = {
  macos: (version: string) => `Pocket-Hub-${version}-arm64.dmg`,
} as const;

export type Platform = keyof typeof PLATFORMS;

/** One platform's archive, every value checked. */
export interface Download {
  platform: Platform;
  /** The archive's own name: "Pocket-Hub-0.2.0-arm64.dmg". */
  archive: string;
  /** The archive itself, on GitHub. */
  url: string;
  /** In bytes. */
  size: number;
  /** The archive's SHA-256, as lowercase hex. */
  sha256: string;
}

/** What the page says about the release. */
export interface Release {
  /** "0.2.0" */
  version: string;
  /** The release's page on GitHub: its notes. */
  notes: string;
  /** One for each of PLATFORMS, in its order. */
  downloads: Download[];
}

/** The part of GitHub's answer the page reads
 *  (https://docs.github.com/en/rest/releases/releases#get-the-latest-release). */
interface GitHubRelease {
  tag_name?: unknown;
  html_url?: unknown;
  assets?: unknown;
}

interface GitHubAsset {
  name?: unknown;
  size?: unknown;
  digest?: unknown;
  browser_download_url?: unknown;
}

/** A release tag as the release steps make it. Its version goes into the
 *  page and into file names, so it is no more than digits and dots. */
const TAG = /^v(\d+\.\d+\.\d+)$/;
const DIGEST = /^sha256:([0-9a-f]{64})$/;

/** Reads GitHub's answer for the latest release; throws an Error whose
 *  message is the one line to print when the answer is not a release the page
 *  can offer. */
export function readRelease(answer: unknown): Release {
  const release = (answer ?? {}) as GitHubRelease;
  const tag = release.tag_name;
  const version = typeof tag === "string" ? TAG.exec(tag)?.[1] : undefined;
  if (version === undefined) {
    throw new Error(`the latest release's tag is ${JSON.stringify(tag)}, not v<major>.<minor>.<patch>`);
  }

  const assets = Array.isArray(release.assets) ? (release.assets as GitHubAsset[]) : [];
  const downloads = (Object.keys(PLATFORMS) as Platform[]).map((platform) =>
    readDownload(platform, PLATFORMS[platform](version), version, assets));

  const notes = `https://github.com/${REPOSITORY}/releases/tag/v${version}`;
  if (release.html_url !== notes) {
    throw new Error(`release v${version}'s page is ${JSON.stringify(release.html_url)}, not ${notes}`);
  }

  return { version, notes, downloads };
}

/** One platform's archive among a release's assets. */
function readDownload(platform: Platform, archive: string, version: string, assets: GitHubAsset[]): Download {
  const asset = assets.find((each) => each.name === archive);
  if (asset === undefined) {
    throw new Error(`release v${version} has no ${archive}`);
  }
  // GitHub's address for an asset is fixed by the repository, the tag and the
  // name; anything else would send visitors somewhere this page did not mean.
  const url = `https://github.com/${REPOSITORY}/releases/download/v${version}/${archive}`;
  if (asset.browser_download_url !== url) {
    throw new Error(`${archive} of v${version} is at ${JSON.stringify(asset.browser_download_url)}, not ${url}`);
  }
  const size = asset.size;
  if (typeof size !== "number" || !Number.isSafeInteger(size) || size <= 0) {
    throw new Error(`${archive} of v${version} has the size ${JSON.stringify(size)}`);
  }
  const sha256 = typeof asset.digest === "string" ? DIGEST.exec(asset.digest)?.[1] : undefined;
  if (sha256 === undefined) {
    throw new Error(`${archive} of v${version} has the digest ${JSON.stringify(asset.digest)}, not sha256:<hex>`);
  }
  return { platform, archive, url, size, sha256 };
}

/** Asks GitHub for the latest release. `token`, when given, is sent so the
 *  request counts against that token's limit, not the address's 60 an hour. */
export async function fetchLatestRelease(token?: string): Promise<Release> {
  const where = `https://api.github.com/repos/${REPOSITORY}/releases/latest`;
  const response = await fetch(where, {
    headers: {
      Accept: "application/vnd.github+json",
      "X-GitHub-Api-Version": "2022-11-28",
      "User-Agent": "pocket-hub-site",
      ...(token ? { Authorization: `Bearer ${token}` } : {}),
    },
  });
  if (!response.ok) {
    // 404 is GitHub's answer for a repository with no published release
    throw new Error(`GitHub answered ${response.status} for ${where}`);
  }
  return readRelease(await response.json());
}
