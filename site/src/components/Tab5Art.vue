<script setup lang="ts">
// An M5Stack Tab5 from the front, as M5Stack's drawing gives it (128 × 80 mm,
// corners R4; drawn here at 2 units a millimetre): a white shell round a black
// glass front, the 5" 16:9 screen toward the left, the camera in the middle of
// the right-hand border, two microphone holes along the top. Applets from the
// store sit on its screen: a picture of what Hub offers it, not its software.
import { useId } from "vue";
import { APPLETS } from "../applets.ts";

const id = useId();
const shell = `tab5-shell-${id}`;
const sky = `tab5-sky-${id}`;
/** Two rows of applets on the screen. */
const rows = [APPLETS.slice(0, 5), APPLETS.slice(5, 10)];
</script>

<template>
  <svg viewBox="0 0 256 160" class="art" aria-hidden="true">
    <defs>
      <linearGradient :id="shell" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#fbfbfc" /><stop offset="1" stop-color="#dcdce2" /></linearGradient>
      <radialGradient :id="sky" cx=".1" cy="0" r="1.3"><stop offset="0" stop-color="#3a2f7a" /><stop offset=".55" stop-color="#17142b" /><stop offset="1" stop-color="#0f0d1a" /></radialGradient>
    </defs>
    <!-- the white shell, and the black glass over the front -->
    <rect x="0.5" y="0.5" width="255" height="159" rx="8" :fill="`url(#${shell})`" stroke="rgba(18,18,40,.18)" />
    <rect x="3.5" y="3.5" width="249" height="153" rx="6" fill="#0a0a0e" />
    <!-- microphones along the top, the camera in the right border -->
    <circle cx="48" cy="8.5" r="1.1" fill="#24242c" />
    <circle cx="116" cy="8.5" r="1.1" fill="#24242c" />
    <circle cx="242" cy="80" r="5" fill="#16161d" />
    <circle cx="242" cy="80" r="2.6" fill="#2b3557" />
    <circle cx="241" cy="79" r=".9" fill="rgba(255,255,255,.5)" />
    <!-- the screen -->
    <rect x="13" y="18" width="216" height="121.5" rx="1.5" :fill="`url(#${sky})`" />
    <rect x="13" y="18" width="216" height="9" fill="rgba(255,255,255,.05)" />
    <circle cx="21" cy="22.5" r="1.6" fill="#5fd3a0" />
    <rect x="198" y="21" width="24" height="3" rx="1.5" fill="rgba(255,255,255,.25)" />
    <g v-for="(row, line) in rows" :key="line">
      <image v-for="(applet, index) in row" :key="applet.id" :href="applet.icon" :x="31 + index * 38" :y="40 + line * 46" width="30" height="30" />
    </g>
    <!-- a sheen across the glass -->
    <path d="M3.5 9.5a6 6 0 0 1 6-6H140L60 156.5H9.5a6 6 0 0 1-6-6Z" fill="rgba(255,255,255,.035)" />
  </svg>
</template>

<style scoped>
.art {
  height: 100%;
  width: auto;
  filter: drop-shadow(0 14px 18px rgba(18, 18, 40, 0.3)) drop-shadow(0 3px 5px rgba(18, 18, 40, 0.2));
}
</style>
