// The host's bootstrap (docs/SERVICE-PROTOCOL.md §2). The host runs, with the
// package folder as the working directory,
//
//   bun --no-install run <sdk>/src/boot.ts ./<entry>
//
// This reads the three variables, imports the entry, serves its default
// export (a ServiceFactory) and exits: 0 after `service.stop` or when the
// lease ends, 1 when the host refuses the process or it cannot start. A
// rejected promise nobody handled is logged and does not end the process:
// one handler's mistake must not close every device's channel.

import { resolve } from "node:path";
import { pathToFileURL } from "node:url";
import { defaultDiag, runService, type ServiceFactory } from "./serve.ts";

const { POCKET_SERVICE_SOCKET: socket, POCKET_SERVICE_TOKEN: token, POCKET_SERVICE_ID: app } = process.env;
const entry = process.argv[2];
if (!socket || !token || !app || !entry) {
  defaultDiag("POCKET_SERVICE_SOCKET, _TOKEN and _ID and an entry are not all given: this program is started by Pocket Hub");
  process.exit(1);
}
process.on("unhandledRejection", (reason: unknown) => {
  defaultDiag(`unhandled rejection: ${reason instanceof Error ? (reason.stack ?? reason.message) : String(reason)}`);
});

let factory: unknown;
try {
  factory = (await import(pathToFileURL(resolve(entry)).href)).default;
} catch (error) {
  defaultDiag(`the entry ${entry} could not be loaded: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
  process.exit(1);
}
if (typeof factory !== "function") {
  defaultDiag(`the entry ${entry} has no service: its default export must be a ServiceFactory, a function that takes the host`);
  process.exit(1);
}
const run = runService(factory as ServiceFactory, { socket, token, app });
process.exit(await run.done);
