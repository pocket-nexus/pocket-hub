<script setup lang="ts">
// The bar over the hero: the page's name, the way to the store, the language.
// It stands on the hero's night, so it is drawn light on dark in either scheme.
// The language is a native <select> under the store's pill: the system's own
// menu, keyboard and screen reader for two choices.
import { LOCALES, chooseLocale, useLanguage, type Locale } from "../i18n/index.ts";
import { STORE } from "../applets.ts";
import HubMark from "./HubMark.vue";
import StoreMark from "./StoreMark.vue";
import UiIcon from "./UiIcon.vue";

const { t, locale } = useLanguage();
const short = (id: Locale) => LOCALES.find((each) => each.id === id)?.short;
</script>

<template>
  <header class="bar page">
    <a class="logo" href="/" :aria-label="t.header.home">
      <HubMark :size="28" />
      <span>Pocket <em>Hub</em></span>
    </a>
    <a class="store" :href="STORE">
      <StoreMark :size="18" />
      <span>{{ t.header.store }}</span>
    </a>
    <label class="language">
      <UiIcon name="globe" :size="16" />
      <span aria-hidden="true">{{ short(locale) }}</span>
      <select :value="locale" :aria-label="t.language" @change="chooseLocale(($event.target as HTMLSelectElement).value as Locale)">
        <option v-for="each in LOCALES" :key="each.id" :value="each.id" :lang="each.id">{{ each.name }}</option>
      </select>
    </label>
  </header>
</template>

<style scoped>
.bar {
  position: relative;
  z-index: 2;
  display: flex;
  align-items: center;
  gap: 8px;
  min-height: 64px;
  color: #fff;
}

.logo {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-right: auto;
  font-size: 17px;
  font-weight: 700;
  letter-spacing: -0.02em;
  white-space: nowrap;
}

.logo svg {
  border-radius: 5px;
}

.logo em {
  font-style: normal;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.66);
}

.store,
.language {
  position: relative;
  display: inline-flex;
  align-items: center;
  gap: 7px;
  height: 34px;
  padding: 0 13px;
  border-radius: 10px;
  font-size: 14px;
  font-weight: 500;
  color: rgba(255, 255, 255, 0.82);
  transition: background 0.15s, color 0.15s;
}

.store:hover,
.language:hover {
  color: #fff;
  background: rgba(255, 255, 255, 0.1);
}

.language {
  background: rgba(255, 255, 255, 0.08);
  font-weight: 600;
}

/* the select covers the pill: a press anywhere on it opens the system's menu */
.language select {
  position: absolute;
  inset: 0;
  width: 100%;
  opacity: 0;
  cursor: pointer;
  appearance: none;
}

.language:has(select:focus-visible) {
  outline: 2px solid var(--accent);
  outline-offset: 2px;
}

@media (max-width: 520px) {
  .store span {
    display: none;
  }
}
</style>
