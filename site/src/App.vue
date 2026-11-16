<script setup lang="ts">
// The one page: the download, then the devices Hub connects to.
import DevicesSection from "./components/DevicesSection.vue";
import HeroSection from "./components/HeroSection.vue";
import SiteFooter from "./components/SiteFooter.vue";
</script>

<template>
  <HeroSection />
  <main>
    <DevicesSection />
  </main>
  <SiteFooter />
</template>
