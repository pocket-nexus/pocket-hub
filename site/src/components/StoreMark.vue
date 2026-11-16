<script setup lang="ts">
// The applet store's mark, as its website draws it (pocket-nexus/applets,
// store/site/src/components/SiteHeader.vue): where this page points back to.
import { useId } from "vue";

withDefaults(defineProps<{ size?: number }>(), { size: 20 });
const sky = `store-sky-${useId()}`;
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 64 64" aria-hidden="true">
    <defs>
      <linearGradient :id="sky" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#7a4bd6" /><stop offset=".55" stop-color="#ff6f91" /><stop offset="1" stop-color="#ffc46b" />
      </linearGradient>
    </defs>
    <rect x="10" y="3" width="44" height="28" rx="6" fill="#23242c" /><rect x="15" y="7" width="34" height="20" rx="2" :fill="`url(#${sky})`" />
    <rect x="10" y="33" width="44" height="28" rx="6" fill="#23242c" /><rect x="15" y="37" width="34" height="20" rx="2" fill="#0d0e14" />
    <g fill="#8e99c9"><rect x="18" y="44" width="28" height="3" rx="1" /><rect x="21" y="49" width="22" height="3" rx="1" /></g>
  </svg>
</template>
