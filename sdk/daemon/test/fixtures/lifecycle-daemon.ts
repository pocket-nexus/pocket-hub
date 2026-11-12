// A daemon for the lifecycle tests, run as Hub runs one: `bun run` with the
// four POCKET_HUB_DAEMON_* variables. It logs what happens to it on standard
// output, one line per step, for the test to read.

import { type DaemonFactory, LinkError, serve } from "@pocket-hub/daemon";

const daemon: DaemonFactory = ({ app, data, log }) => {
  log(`started ${app} ${data}`);
  return {
    attach(channel, info) {
      log(`attach ${JSON.stringify(info)}`);
      channel.on("echo", (args) => ({ echo: args }));
      channel.on("throw", () => {
        throw new Error("sync boom");
      });
      channel.on("reject", async () => {
        await Bun.sleep(1);
        throw new TypeError("async boom");
      });
      channel.on("deny", () => {
        throw new LinkError("denied", "no");
      });
      channel.on("stray", () => {
        // A rejection nobody handles, outside any handler's promise: logged, the process stays.
        void Promise.reject(new Error("stray rejection"));
        setTimeout(() => void Promise.reject(new Error("later stray rejection")), 1);
        return { ok: true };
      });
      channel.onClose((reason) => log(`closed ${info.generation} ${reason}`));
    },
    async stop() {
      await Bun.sleep(5);
      log("stop called");
    },
  };
};

export default daemon;
if (import.meta.main) await serve(daemon);
