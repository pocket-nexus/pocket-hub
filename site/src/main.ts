// hub.pocket.nexus: the page that hands a visitor Pocket Hub. Most arrive from
// the applet store's website, whose Install button opens Pocket Hub
// (docs/DEEP-LINKS.md) and does nothing on a computer without it, so the page
// leads with the download and ends with the way back to the store.
import { createApp } from "vue";
import "@fontsource-variable/inter";
import "./styles/base.css";
import App from "./App.vue";

createApp(App).mount("#app");
