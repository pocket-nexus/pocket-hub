// @pocket-hub/daemon: the daemon side of Pocket Hub's local daemon protocol
// (docs/DAEMON-PROTOCOL.md). A daemon package's entry:
//
//   import { serve, type DaemonFactory } from "@pocket-hub/daemon";
//   const daemon: DaemonFactory = ({ log }) => ({
//     attach(channel, info) {
//       channel.on("echo", (args) => ({ echo: args }));
//       channel.onClose((reason) => log(`generation ${info.generation} closed: ${reason}`));
//     },
//   });
//   export default daemon;
//   if (import.meta.main) await serve(daemon);
//
// The names mirror the reference daemon API of PocketJS
// (tools/pocket-link/v2/peer.ts, hub.ts, daemon.ts), so a daemon written for
// the reference Hub ports by changing its imports.

export {
  AppletChannel,
  type AppletCall,
  type AppletEventHandler,
  type AppletHandler,
  type Call,
  type DaemonInfo,
  type ProgressHandler,
} from "./channel.ts";
export { LinkError, type Json } from "./errors.ts";
export { StreamReader, StreamWriter } from "./streams.ts";
export {
  DAEMON_PROTOCOL,
  type Daemon,
  type DaemonContext,
  type DaemonFactory,
  type DaemonRun,
  type RunOptions,
  runDaemon,
  serve,
} from "./serve.ts";
export type { ResetName } from "./frame.ts";

/** The reference's name for `Daemon` (`hub.ts` exports it as `LinkDaemon`). */
export type { Daemon as LinkDaemon } from "./serve.ts";
