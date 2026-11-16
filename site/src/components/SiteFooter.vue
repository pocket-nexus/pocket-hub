<script setup lang="ts">
// The foot of the page, as the store's: who makes it, and the source.
import { REPOSITORY_URL } from "../applets.ts";
import { useLanguage } from "../i18n/index.ts";
import Sentence from "./Sentence.vue";
import UiIcon from "./UiIcon.vue";

const { t } = useLanguage();
</script>

<template>
  <footer>
    <div class="page inner">
      <p>
        <Sentence :text="t.footer.credit">
          <template #nexus><a href="https://pocket.nexus/">Pocket Nexus</a></template>
        </Sentence>
      </p>
      <a class="source" :href="REPOSITORY_URL"><UiIcon name="github" :size="16" />{{ t.footer.source }}</a>
    </div>
  </footer>
</template>

<style scoped>
footer {
  border-top: 1px solid var(--line-soft);
  font-size: 13px;
  color: var(--text-3);
}

.inner {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px 24px;
  padding-top: 22px;
  padding-bottom: 28px;
}

a {
  color: var(--text-2);
}

a:hover {
  color: var(--text);
}

p a:hover {
  text-decoration: underline;
  text-underline-offset: 3px;
}

.source {
  display: inline-flex;
  align-items: center;
  gap: 6px;
}
</style>
