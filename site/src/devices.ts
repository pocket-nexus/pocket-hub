// The devices Hub connects to, as the page lists them: each by its maker's
// name, the same in every language, with a drawing of it. Setting one up is
// the app's business, so the page says nothing more about it. A device is
// added here, with its drawing.
import type { Component } from "vue";
import Nintendo3dsArt from "./components/Nintendo3dsArt.vue";
import Tab5Art from "./components/Tab5Art.vue";

export interface Device {
  id: string;
  name: string;
  art: Component;
}

export const DEVICES: Device[] = [
  { id: "3ds", name: "Nintendo 3DS", art: Nintendo3dsArt },
  { id: "tab5", name: "M5Stack Tab5", art: Tab5Art },
];
