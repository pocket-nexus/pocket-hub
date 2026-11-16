/// <reference types="vite/client" />

/** The latest release, read when the page is built (vite.config.ts). */
declare module "virtual:release" {
  const release: import("./release.ts").Release;
  export default release;
}

declare module "*.vue" {
  import type { DefineComponent } from "vue";
  const component: DefineComponent<object, object, unknown>;
  export default component;
}
