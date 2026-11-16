<script setup lang="ts">
// One platform's download: a link to its archive on GitHub, so the press is
// the download (src/release.ts has why). It says what it fetches.
import release from "virtual:release";
import { computed } from "vue";
import { fill, megabytes } from "../format.ts";
import { useLanguage } from "../i18n/index.ts";
import type { Download } from "../release.ts";
import UiIcon from "./UiIcon.vue";

const props = defineProps<{ download: Download }>();
const { t, locale } = useLanguage();
const facts = computed(() => fill(t.value.release.facts, { version: release.version, size: megabytes(props.download.size, locale.value) }));
</script>

<template>
  <a class="download" :href="download.url">
    <span class="badge"><UiIcon name="download" :size="20" /></span>
    <span class="words">
      <strong>{{ t.platforms[download.platform].download }}</strong>
      <small>{{ facts }}</small>
    </span>
  </a>
</template>

<style scoped>
.download {
  display: inline-flex;
  align-items: center;
  gap: 12px;
  min-height: 60px;
  padding: 9px 26px 9px 10px;
  border-radius: 18px;
  background: linear-gradient(180deg, color-mix(in srgb, var(--accent) 88%, white), var(--accent));
  color: var(--on-accent);
  box-shadow:
    inset 0 1px 0 rgba(255, 255, 255, 0.28),
    0 1px 2px rgba(18, 18, 40, 0.2),
    0 14px 34px -12px color-mix(in srgb, var(--accent) 80%, transparent);
  transition: transform 0.18s var(--ease), box-shadow 0.18s var(--ease), filter 0.18s;
}

.download:hover {
  filter: brightness(1.06);
  transform: translateY(-1px);
}

.download:active {
  transform: scale(0.985);
}

.badge {
  display: grid;
  place-items: center;
  width: 42px;
  height: 42px;
  border-radius: 12px;
  background: rgba(255, 255, 255, 0.16);
}

.words {
  display: flex;
  flex-direction: column;
  align-items: flex-start;
  line-height: 1.2;
}

strong {
  font-size: 16.5px;
  font-weight: 650;
  letter-spacing: -0.012em;
  white-space: nowrap;
}

small {
  margin-top: 2px;
  font-size: 12.5px;
  font-weight: 500;
  opacity: 0.78;
  font-variant-numeric: tabular-nums;
  white-space: nowrap;
}
</style>
