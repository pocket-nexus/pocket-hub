<script setup lang="ts">
// The top of the page: the name, one line on what it does, and a download for
// each platform the release carries. A visitor sent here by the store's
// Install button came for the download, so it is the first thing to press.
// Under it, the store's applets in a slow row: what Hub is for.
import release from "virtual:release";
import { APPLETS } from "../applets.ts";
import { useLanguage } from "../i18n/index.ts";
import DownloadButton from "./DownloadButton.vue";
import HubMark from "./HubMark.vue";
import SiteHeader from "./SiteHeader.vue";

const { t } = useLanguage();
/** The icons twice over, so the row runs on without a seam. */
const reel = [...APPLETS, ...APPLETS];
</script>

<template>
  <section class="hero">
    <div class="sky" aria-hidden="true"><i /><i /><i /></div>
    <SiteHeader />
    <div class="intro page">
      <HubMark :size="84" class="mark" />
      <h1>Pocket Hub</h1>
      <p class="lede">{{ t.lede }}</p>

      <div class="downloads">
        <div v-for="download in release.downloads" :key="download.platform" class="platform">
          <DownloadButton :download="download" />
          <p class="requires">{{ t.platforms[download.platform].requires }}</p>
          <details class="checksum">
            <summary>{{ t.release.checksum }}</summary>
            <code>{{ download.sha256 }}</code>
          </details>
        </div>
      </div>
      <a class="notes" :href="release.notes">{{ t.release.notes }} ›</a>
    </div>

    <div class="reel" aria-hidden="true">
      <ul>
        <li v-for="(applet, index) in reel" :key="index"><img :src="applet.icon" alt="" width="128" height="128" decoding="async"></li>
      </ul>
    </div>
  </section>
</template>

<style scoped>
.hero {
  position: relative;
  isolation: isolate;
  overflow: hidden;
  padding-bottom: 44px;
  background: var(--night);
  color: #fff;
  /* the hero is night in both schemes: its accent is the dark one */
  --accent: #655bf5;
  --accent-hover: #776efa;
}

/* the store's sky, low behind the icons, and a faint grid of dots */
.sky {
  position: absolute;
  inset: 0;
  z-index: -1;
  background-image: radial-gradient(rgba(255, 255, 255, 0.07) 1px, transparent 1px);
  background-size: 22px 22px;
  mask-image: radial-gradient(90% 80% at 50% 30%, #000 30%, transparent 85%);
}

.sky i {
  position: absolute;
  border-radius: 50%;
  filter: blur(90px);
}

.sky i:nth-child(1) {
  left: 6%;
  top: 48%;
  width: 46vw;
  height: 30vw;
  background: var(--sky-1);
  opacity: 0.5;
}

.sky i:nth-child(2) {
  right: 4%;
  top: 56%;
  width: 40vw;
  height: 26vw;
  background: var(--sky-2);
  opacity: 0.32;
}

.sky i:nth-child(3) {
  left: 36%;
  top: 80%;
  width: 30vw;
  height: 20vw;
  background: var(--sky-3);
  opacity: 0.24;
}

.intro {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding-top: clamp(44px, 8vw, 96px);
  text-align: center;
}

.mark {
  filter: drop-shadow(0 18px 30px rgba(122, 75, 214, 0.45)) drop-shadow(0 4px 8px rgba(0, 0, 0, 0.5));
}

h1 {
  margin-top: 26px;
  font-size: clamp(48px, 8vw, 84px);
  font-weight: 750;
  line-height: 1;
  letter-spacing: -0.045em;
  background: linear-gradient(180deg, #fff 35%, rgba(255, 255, 255, 0.7));
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}

.lede {
  max-width: 30em;
  margin-top: 20px;
  font-size: clamp(17px, 2vw, 20px);
  line-height: 1.5;
  color: rgba(255, 255, 255, 0.68);
  text-wrap: balance;
}

.downloads {
  display: flex;
  flex-wrap: wrap;
  justify-content: center;
  gap: 24px 40px;
  margin-top: 38px;
}

.platform {
  display: flex;
  flex-direction: column;
  align-items: center;
}

.requires {
  margin-top: 14px;
  font-size: 13px;
  color: rgba(255, 255, 255, 0.55);
}

.checksum {
  margin-top: 4px;
  font-size: 12.5px;
  color: rgba(255, 255, 255, 0.4);
}

.checksum summary {
  cursor: pointer;
  list-style: none;
}

.checksum summary::-webkit-details-marker {
  display: none;
}

.checksum summary:hover {
  color: rgba(255, 255, 255, 0.7);
}

.checksum code {
  display: block;
  max-width: 34em;
  margin-top: 8px;
  padding: 8px 12px;
  border-radius: 10px;
  background: rgba(255, 255, 255, 0.06);
  color: rgba(255, 255, 255, 0.7);
  font-size: 11.5px;
  word-break: break-all;
  user-select: all;
}

.notes {
  margin-top: 18px;
  font-size: 14px;
  font-weight: 550;
  color: rgba(255, 255, 255, 0.75);
}

.notes:hover {
  color: #fff;
}

/* the store's icons in a slow row, faded at both ends */
.reel {
  margin-top: clamp(48px, 7vw, 80px);
  mask-image: linear-gradient(90deg, transparent, #000 15%, #000 85%, transparent);
}

.reel ul {
  display: flex;
  gap: 24px;
  width: max-content;
  padding: 10px 0 24px;
  animation: reel 50s linear infinite;
}

.reel img {
  width: 80px;
  height: 80px;
  filter: drop-shadow(0 10px 18px rgba(0, 0, 0, 0.5));
}

@keyframes reel {
  to {
    transform: translateX(calc(-50% - 12px));
  }
}
</style>
