// @pocketjs/service: the service side of Pocket Hub's local service protocol
// (docs/SERVICE-PROTOCOL.md). A service package's entry is a module whose
// default export is the service; the host loads it and calls it with the
// host (§2):
//
//   import type { ServiceFactory } from "@pocketjs/service";
//   const service: ServiceFactory = (host) => ({
//     attach(channel, info) {
//       channel.on("visits", async () => {
//         const visits = ((await host.store.getJson<number>("visits")) ?? 0) + 1;
//         await host.store.setJson("visits", visits);
//         return { visits };
//       });
//       channel.onClose((reason) => host.log(`generation ${info.generation} closed: ${reason}`));
//     },
//   });
//   export default service;
//
// The channel's names mirror the reference service API of PocketJS
// (tools/pocket-link/v2/peer.ts), with ServiceError for its LinkError.

export {
  AppletChannel,
  type AppletCall,
  type AppletEventHandler,
  type AppletHandler,
  type Call,
  type ChannelInfo,
  type ProgressHandler,
} from "./channel.ts";
export type { Glyphs, Images, Media, Native, Net } from "./capabilities.ts";
export { ServiceError, type Json } from "./errors.ts";
export type { ByteSource, Host, Store } from "./host.ts";
export type { Service, ServiceFactory } from "./serve.ts";
export { StreamReader, StreamWriter } from "./streams.ts";
export type { ResetName } from "./frame.ts";
