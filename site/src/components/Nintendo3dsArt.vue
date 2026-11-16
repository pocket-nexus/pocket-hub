<script setup lang="ts">
// A Nintendo 3DS, open, as the page draws it among the devices: the two
// screens (5:3 above, 4:3 below, with the hinge between), the circle pad and
// d-pad on the left, the four buttons on the right. Applets from the store sit
// on its top screen: a picture of what Hub puts on it, not of its software.
import { useId } from "vue";
import { APPLETS } from "../applets.ts";

const id = useId();
const shell = `ds-shell-${id}`;
const low = `ds-low-${id}`;
const sky = `ds-sky-${id}`;
/** Three applets on the top screen, a fourth slot empty. */
const shown = APPLETS.slice(0, 3);
</script>

<template>
  <svg viewBox="0 0 200 262" class="art" aria-hidden="true">
    <defs>
      <linearGradient :id="shell" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#3a3b52" /><stop offset="1" stop-color="#1e1f2b" /></linearGradient>
      <linearGradient :id="low" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="#2d2e40" /><stop offset="1" stop-color="#191a24" /></linearGradient>
      <radialGradient :id="sky" cx=".15" cy="0" r="1.2"><stop offset="0" stop-color="#5a3aa6" /><stop offset=".6" stop-color="#1a1530" /><stop offset="1" stop-color="#120f1f" /></radialGradient>
    </defs>
    <!-- the lid -->
    <rect x="6" y="2" width="188" height="122" rx="15" :fill="`url(#${shell})`" />
    <rect x="6.5" y="2.5" width="187" height="121" rx="14.5" fill="none" stroke="rgba(255,255,255,.14)" />
    <rect x="20" y="14" width="160" height="98" rx="5" fill="#07070b" />
    <rect x="26" y="19" width="148" height="88.8" rx="2" :fill="`url(#${sky})`" />
    <g>
      <image v-for="(applet, index) in shown" :key="applet.id" :href="applet.icon" :x="38 + index * 33" y="47" width="27" height="27" />
      <rect x="137" y="47" width="27" height="27" rx="6" fill="rgba(255,255,255,.07)" />
    </g>
    <circle cx="100" cy="8" r="1.6" fill="#0d0d14" />
    <!-- the hinge -->
    <rect x="12" y="122" width="176" height="12" rx="6" fill="#121219" />
    <!-- the base -->
    <rect x="6" y="132" width="188" height="128" rx="15" :fill="`url(#${low})`" />
    <rect x="6.5" y="132.5" width="187" height="127" rx="14.5" fill="none" stroke="rgba(255,255,255,.1)" />
    <rect x="50" y="142" width="100" height="80" rx="4" fill="#07070b" />
    <rect x="54" y="146" width="92" height="69" rx="2" fill="#16151f" />
    <rect x="66" y="170" width="68" height="6" rx="3" fill="rgba(255,255,255,.12)" />
    <rect x="76" y="182" width="48" height="6" rx="3" fill="rgba(255,255,255,.08)" />
    <circle cx="28" cy="160" r="12" fill="#101017" />
    <circle cx="28" cy="159" r="7.5" fill="#4a4b60" />
    <rect x="19" y="194" width="18" height="6" rx="1.5" fill="#101017" />
    <rect x="25" y="188" width="6" height="18" rx="1.5" fill="#101017" />
    <circle cx="172" cy="152" r="4.6" fill="#8b7cf6" />
    <circle cx="181" cy="161" r="4.6" fill="#ff6f91" />
    <circle cx="172" cy="170" r="4.6" fill="#ffc46b" />
    <circle cx="163" cy="161" r="4.6" fill="#5fd3a0" />
    <rect x="160" y="196" width="10" height="3.4" rx="1.7" fill="#101017" />
    <rect x="174" y="196" width="10" height="3.4" rx="1.7" fill="#101017" />
    <rect x="80" y="232" width="40" height="6" rx="3" fill="#101017" />
  </svg>
</template>

<style scoped>
.art {
  height: 100%;
  width: auto;
  filter: drop-shadow(0 14px 18px rgba(18, 18, 40, 0.35)) drop-shadow(0 3px 5px rgba(18, 18, 40, 0.25));
}
</style>
