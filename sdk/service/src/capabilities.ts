// The capabilities this host does not provide yet (docs/SERVICE-PROTOCOL.md
// §7): `host.net`, `host.native`, and the channel's `image`, `text` and
// `media`. Every member a service can name exists whatever its manifest
// says, so code that reads `host.net` on this host does not crash: it fails
// its call with ServiceError("not_required"), as a call of a capability the
// manifest does not require does (no manifest can require these here: the
// install refuses a capability the host does not provide). Their full
// surfaces come with the steps that build them.

import { ServiceError } from "./errors.ts";

/** HTTP and WebSocket to the manifest's hosts. Not provided by this host: every call rejects with `not_required`. */
export interface Net {
  fetch(url: string, init?: { signal?: AbortSignal }): Promise<never>;
  websocket(url: string, init?: { signal?: AbortSignal }): Promise<never>;
}

/** Arbitrary code. Not provided by this host: reading `files` throws `not_required`. */
export interface Native {
  readonly files: string;
}

/** A network image in the device's texture format. Not provided by this host: `render` rejects with `not_required`. */
export interface Images {
  render(source: { url: string; headers?: Record<string, string> }, box: { width: number; height: number }, options?: { signal?: AbortSignal }): Promise<never>;
}

/** Glyphs the device lacks. Not provided by this host: `atlas` rejects with `not_required`. */
export interface Glyphs {
  atlas(request: { px: number; text: string; bold?: boolean; signal?: AbortSignal }): Promise<never>;
}

/** A stream the device can play. Not provided by this host: `open` rejects with `not_required`. */
export interface Media {
  open(source: { url: string; headers?: Record<string, string> }, options?: { signal?: AbortSignal }): Promise<never>;
}

const notProvided = (name: string) => new ServiceError("not_required", `this host provides no \`${name}\` capability, so the manifest cannot require it`);
const refuse = (name: string) => () => Promise.reject(notProvided(name));

export const net: Net = Object.freeze({ fetch: refuse("net"), websocket: refuse("net") });
export const native: Native = Object.freeze({
  get files(): string {
    throw notProvided("native");
  },
});
export const images: Images = Object.freeze({ render: refuse("image") });
export const glyphs: Glyphs = Object.freeze({ atlas: refuse("text") });
export const media: Media = Object.freeze({ open: refuse("media") });
