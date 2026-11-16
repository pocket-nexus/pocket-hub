<script setup lang="ts">
// A sentence of the language's own order with pieces of the page in it: each
// {name} in `text` is the slot of that name, so a link or a button stands
// wherever the language puts it. Text outside the names is drawn as text.
import { computed } from "vue";

const props = defineProps<{ text: string }>();

/** The sentence cut at its names: text, name, text, name, … text. */
const parts = computed(() => props.text.split(/\{(\w+)\}/));
</script>

<template>
  <template v-for="(part, index) in parts" :key="index">
    <slot v-if="index % 2 === 1" :name="part" />
    <template v-else>{{ part }}</template>
  </template>
</template>
