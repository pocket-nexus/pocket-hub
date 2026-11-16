<script setup lang="ts">
// The page's own glyphs, drawn on the store's 24 px grid in the text's colour
// (pocket-nexus/applets, store/site/src/components/UiIcon.vue; download and
// globe are its drawings).
const GLYPHS = {
  download: '<path d="M12 4v11m-4.5-4.5L12 15l4.5-4.5M5 19.5h14"/>',
  globe: '<circle cx="12" cy="12" r="8.5"/><path d="M3.5 12h17M12 3.5c2.7 2.3 2.7 14.7 0 17M12 3.5c-2.7 2.3-2.7 14.7 0 17"/>',
  github: '<path d="M12 3.2a8.8 8.8 0 0 0-2.8 17.1c.4.1.6-.2.6-.4v-1.6c-2.5.5-3-1.1-3-1.1-.4-1-1-1.3-1-1.3-.8-.6.1-.6.1-.6.9.1 1.4.9 1.4.9.8 1.4 2.1 1 2.6.8.1-.6.3-1 .6-1.2-2-.2-4-1-4-4.4 0-1 .3-1.8.9-2.4-.1-.2-.4-1.1.1-2.4 0 0 .7-.2 2.4.9a8.4 8.4 0 0 1 4.4 0c1.7-1.1 2.4-.9 2.4-.9.5 1.3.2 2.2.1 2.4.6.6.9 1.4.9 2.4 0 3.4-2.1 4.2-4 4.4.3.3.6.8.6 1.6v2.4c0 .2.2.5.6.4A8.8 8.8 0 0 0 12 3.2Z" fill="currentColor" stroke="none"/>',
} as const;

export type GlyphName = keyof typeof GLYPHS;

withDefaults(defineProps<{ name: GlyphName; size?: number }>(), { size: 20 });
</script>

<template>
  <!-- eslint-disable-next-line vue/no-v-html: the glyphs above, written here -->
  <svg class="glyph" :width="size" :height="size" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.9" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true" v-html="GLYPHS[name]" />
</template>

<style scoped>
.glyph {
  flex: none;
}
</style>
