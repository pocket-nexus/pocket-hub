<script setup lang="ts">
// The devices Hub connects to (src/devices.ts): each drawn as it looks, under
// its name.
import { DEVICES } from "../devices.ts";
import { useLanguage } from "../i18n/index.ts";

const { t } = useLanguage();
</script>

<template>
  <section class="devices page">
    <h2 class="section-title">{{ t.devices.title }}</h2>
    <ul>
      <li v-for="device in DEVICES" :key="device.id">
        <div class="stage">
          <component :is="device.art" />
        </div>
        <h3>{{ device.name }}</h3>
      </li>
    </ul>
  </section>
</template>

<style scoped>
.devices {
  padding-top: clamp(56px, 8vw, 88px);
  padding-bottom: clamp(64px, 9vw, 104px);
  text-align: center;
}

ul {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(min(100%, 300px), 380px));
  justify-content: center;
  gap: 20px;
  margin-top: 32px;
}

li {
  padding: 12px 12px 20px;
  border-radius: 24px;
  background: var(--surface);
  box-shadow: var(--shadow-card);
}

/* the device, standing in its own light */
.stage {
  display: flex;
  align-items: center;
  justify-content: center;
  height: 220px;
  padding: 28px 0;
  margin-bottom: 18px;
  border-radius: 16px;
  background:
    radial-gradient(70% 70% at 50% 45%, var(--accent-soft), transparent 75%),
    var(--sunken);
}

h3 {
  font-size: 17px;
  font-weight: 650;
  letter-spacing: -0.015em;
}
</style>
