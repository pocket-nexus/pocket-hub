<script setup lang="ts">
// Pocket Hub's mark on this page: a Mac with the store's sky on its screen,
// drawn as the store draws its own mark (a 3DS with that sky on the top
// screen), so the two read as one family. The app has no icon of its own yet;
// this is the page's, and public/favicon.svg is the same drawing.
import { useId } from "vue";

withDefaults(defineProps<{ size?: number }>(), { size: 26 });
// one gradient per mark drawn: url(#id) takes the first element with that id
const sky = `hub-sky-${useId()}`;
</script>

<template>
  <svg :width="size" :height="size" viewBox="0 0 64 64" aria-hidden="true">
    <defs>
      <linearGradient :id="sky" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0" stop-color="#7a4bd6" /><stop offset=".55" stop-color="#ff6f91" /><stop offset="1" stop-color="#ffc46b" />
      </linearGradient>
    </defs>
    <rect x="7" y="10" width="50" height="34" rx="5" fill="#23242c" />
    <rect x="11" y="14" width="42" height="26" rx="2" :fill="`url(#${sky})`" />
    <path d="M4 47h56a1 1 0 0 1 .8 1.6l-2.4 3.2a3 3 0 0 1-2.4 1.2H8a3 3 0 0 1-2.4-1.2l-2.4-3.2A1 1 0 0 1 4 47Z" fill="#23242c" />
  </svg>
</template>
