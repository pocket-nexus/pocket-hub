// @pocketjs/service: the service side of Pocket Hub's local service protocol
// (docs/SERVICE-PROTOCOL.md). A service package's entry:
//
//   import { serve, type ServiceFactory } from "@pocketjs/service";
//   const service: ServiceFactory = ({ log }) => ({
//     attach(channel, info) {
//       channel.on("echo", (args) => ({ echo: args }));
//       channel.onClose((reason) => log(`generation ${info.generation} closed: ${reason}`));
//     },
//   });
//   export default service;
//   if (import.meta.main) await serve(service);
//
// The names mirror the reference service API of PocketJS
// (tools/pocket-link/v2/peer.ts, hub.ts, daemon.ts), with service where the
// reference says daemon and ServiceError for its LinkError, so a service
// written for the reference Hub ports by changing its imports and those names.

export {
  AppletChannel,
  type AppletCall,
  type AppletEventHandler,
  type AppletHandler,
  type Call,
  type ServiceInfo,
  type ProgressHandler,
} from "./channel.ts";
export { ServiceError, type Json } from "./errors.ts";
export { StreamReader, StreamWriter } from "./streams.ts";
export {
  SERVICE_PROTOCOL,
  type Service,
  type ServiceContext,
  type ServiceFactory,
  type ServiceRun,
  type RunOptions,
  runService,
  serve,
} from "./serve.ts";
export type { ResetName } from "./frame.ts";
