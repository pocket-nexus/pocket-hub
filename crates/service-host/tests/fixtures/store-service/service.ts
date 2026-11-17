// A service on the host's own SDK, for the methods' tests: each start counts
// itself in the store before the service exists, so a test sees
// `Service.store` move, and a store the host refuses fails the start.
import type { ServiceFactory } from "@pocketjs/service";

const service: ServiceFactory = async (host) => {
  const starts = ((await host.store.getJson<number>("starts")) ?? 0) + 1;
  await host.store.setJson("starts", starts);
  host.log(`started ${host.id}: start ${starts}`);
  return {
    attach(channel, info) {
      host.log(`channel ${info.generation}`);
      channel.on("starts", async () => ({ starts: await host.store.getJson<number>("starts") }));
    },
  };
};

export default service;
