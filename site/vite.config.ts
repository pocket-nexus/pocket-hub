// hub.pocket.nexus: a Vue app built into site/dist, which wrangler.jsonc
// serves as static assets.
//
//   bun run dev          the page with hot reload
//   bun run build        site/dist
//
// Both read the latest release first (src/release.ts) and hand it to the page
// as the module virtual:release, so each Download button is an archive itself.
// A release the page cannot offer stops them with one line. GITHUB_TOKEN, when
// set, is sent to GitHub.
import { defineConfig, type Plugin } from "vite";
import vue from "@vitejs/plugin-vue";
import { fetchLatestRelease, type Release } from "./src/release.ts";

const MODULE = "virtual:release";

function latestRelease(): Plugin {
  let release: Release | undefined;
  return {
    name: "latest-release",
    async buildStart() {
      release = await fetchLatestRelease(process.env.GITHUB_TOKEN || undefined);
      this.info(`Pocket Hub ${release.version}: ${release.downloads.map((each) => each.url).join(", ")}`);
    },
    resolveId: (id) => (id === MODULE ? `\0${MODULE}` : undefined),
    load(id) {
      if (id !== `\0${MODULE}`) return undefined;
      if (!release) throw new Error("the latest release was not read before the page asked for it");
      return `export default ${JSON.stringify(release)};`;
    },
  };
}

export default defineConfig({
  plugins: [vue(), latestRelease()],
  build: { target: "es2022" },
});
