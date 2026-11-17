// The service process's life, as a real child `bun` process started as the
// host starts one, its bootstrap with the three POCKET_SERVICE_* variables
// (SERVICE-PROTOCOL.md §2, §4, §6): hello and ready, the host connection, a
// refused hello, offers, `service.stop`, the lease ending, and handlers that
// fail without ending the process.

import { afterEach, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { BOOT, type Child, locale, spawnService } from "./child.ts";
import { inProcess, until } from "./helpers.ts";
import { MockHub, T } from "./mock-hub.ts";

const ENTRY = join(import.meta.dir, "fixtures", "lifecycle-service.ts");

let hub: MockHub | null = null;
let child: Child | null = null;
afterEach(async () => {
  if (child && child.proc.exitCode === null) child.proc.kill();
  if (hub) {
    expect(hub.violations).toEqual([]);
    await hub.shutdown();
  }
  hub = null;
  child = null;
});

describe("a service process", () => {
  test("hello → ready → host.attach, the factory with the host, an offer → attach(info), service.stop → stop(), exit 0", async () => {
    hub = await MockHub.start({ app: "dev.pocket-hub.lifecycle" });
    child = spawnService("./lifecycle-service.ts", hub.env(), join(import.meta.dir, "fixtures"));
    await hub.waitControl(5000);
    expect(hub.hellos).toEqual([{ t: "service.hello", protocol: 3, token: hub.token, app: "dev.pocket-hub.lifecycle" }]);
    await hub.waitHost(5000);
    await child.line((l) => l === "started dev.pocket-hub.lifecycle");
    // The three variables and the locale, and nothing else of the parent's.
    const env = (await child.line((l) => l.startsWith("env "))).slice(4).split(",");
    expect(env).toEqual(["POCKET_SERVICE_ID", "POCKET_SERVICE_SOCKET", "POCKET_SERVICE_TOKEN", ...Object.keys(locale())].sort());
    const ch = await hub.offer({ generation: 9001 }, { timeoutMs: 5000 });
    const attach = await child.line((l) => l.startsWith("attach "));
    expect(JSON.parse(attach.slice(7))).toEqual({ device: "0123456789abcdef", deviceName: "Test 3DS", model: "new-3ds-xl", target: "3ds", app: "dev.pocket-hub.lifecycle", hash: "fedcba9876543210", generation: 9001 });
    expect((await ch.reply(ch.request("echo", { x: 1 }))).json).toEqual({ echo: { x: 1 } });
    hub.stop();
    const notice = await ch.waitFor((f) => f.type === T.event && f.id === ch.channel, 5000);
    expect(notice.json).toEqual({ t: "channel.close", reason: "stop" });
    expect(await child.exited).toBe(0);
    // stop() ran before the channels closed.
    const stopAt = child.out.indexOf("stop called");
    expect(stopAt).toBeGreaterThan(-1);
    expect(child.out.indexOf("closed 9001 stop")).toBeGreaterThan(stopAt);
  });

  test("a refused hello: exit 1, the message on standard error", async () => {
    hub = await MockHub.start({ refuse: "unknown token" });
    child = spawnService(ENTRY, hub.env());
    expect(await child.exited).toBe(1);
    expect(child.err.join("\n")).toContain("the host refused the service: unknown token");
  });

  test("the control connection closing ends the process", async () => {
    hub = await MockHub.start();
    child = spawnService(ENTRY, hub.env());
    await hub.waitControl(5000);
    const ch = await hub.offer({}, { timeoutMs: 5000 });
    hub.closeControl();
    expect(await child.exited).toBe(0);
    await ch.closed;
    expect(child.out).toContain("stop called");
  });

  test("the host connection closing ends the process too: it is part of the lease", async () => {
    hub = await MockHub.start();
    child = spawnService(ENTRY, hub.env());
    await hub.waitHost(5000);
    const ch = await hub.offer({}, { timeoutMs: 5000 });
    hub.closeHost();
    expect(await child.exited).toBe(0);
    await ch.closed;
    expect(child.out).toContain("stop called");
  });

  test("a refused host.attach: exit 1, the message on standard error", async () => {
    hub = await MockHub.start({ refuseHost: "unknown or used ticket" });
    child = spawnService(ENTRY, hub.env());
    expect(await child.exited).toBe(1);
    expect(child.err.join("\n")).toContain("the host refused the host connection: unknown or used ticket");
  });

  test("an entry without a default export, or one that is no function, says so and exits 1", async () => {
    hub = await MockHub.start();
    for (const [source, said] of [["export const service = {};", "its default export must be a ServiceFactory"], ["export default { attach() {} };", "its default export must be a ServiceFactory"], ["throw new Error('cannot load');", "could not be loaded: Error: cannot load"]] as const) {
      const dir = join(hub.dir, `entry-${said.length}-${source.length}`);
      await Bun.write(join(dir, "entry.ts"), source);
      child = spawnService("./entry.ts", hub.env(), dir);
      expect(await child.exited).toBe(1);
      expect(child.err.join("\n")).toContain(said);
    }
    expect(hub.hellos).toEqual([]);
  });

  test("handlers that throw, reject, or leave a rejection unhandled do not end the process", async () => {
    hub = await MockHub.start();
    child = spawnService(ENTRY, hub.env());
    await hub.waitControl(5000);
    const ch = await hub.offer({}, { timeoutMs: 5000 });
    expect((await ch.reply(ch.request("throw"))).json).toEqual({ code: "internal", message: "sync boom" });
    expect((await ch.reply(ch.request("reject"))).json).toEqual({ code: "internal", message: "async boom" });
    expect((await ch.reply(ch.request("deny"))).json).toEqual({ code: "denied", message: "no" });
    expect((await ch.reply(ch.request("stray"))).json).toEqual({ ok: true });
    await child.line((l) => l.includes("later stray rejection"));
    await Bun.sleep(30);
    expect(child.proc.exitCode).toBeNull();
    expect((await ch.reply(ch.request("echo", { still: "here" }))).json).toEqual({ echo: { still: "here" } });
    hub.stop();
    expect(await child.exited).toBe(0);
  });

  test("without the three variables the bootstrap says why and exits 1", async () => {
    const proc = Bun.spawn([process.execPath, "run", BOOT, ENTRY], { env: locale(), stdout: "pipe", stderr: "pipe" });
    expect(await proc.exited).toBe(1);
    expect(await new Response(proc.stderr).text()).toContain("POCKET_SERVICE_SOCKET");
  });
});

describe("in process", () => {
  test("a factory that throws, after the host connection it was given, ends with 1", async () => {
    const run = await inProcess(() => {
      throw new Error("cannot start");
    });
    hub = run.hub;
    expect(await run.done).toBe(1);
    expect(run.hub.hellos.length).toBe(1);
    expect(run.diags.join("\n")).toContain("cannot start");
  });

  test("ready: the channels offered meanwhile are attached once it resolves", async () => {
    let resolveReady!: () => void;
    const ready = new Promise<void>((resolve) => (resolveReady = resolve));
    let attached = 0;
    const run = await inProcess({ ready, attach: () => void (attached += 1) });
    hub = run.hub;
    const offered = hub.offer();
    await Bun.sleep(50);
    expect(attached).toBe(0);
    resolveReady();
    await offered;
    await until(() => attached === 1);
    expect(await run.cleanup()).toBe(0);
  });

  test("offers that come before service.ready is read, and a malformed offer, are handled", async () => {
    const run = await inProcess({ attach() {} });
    hub = run.hub;
    hub.sendControl(new Uint8Array([0x12, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0x7b, 0x7d])); // an event without t: ignored
    const { jsonFrame } = await import("./mock-hub.ts");
    hub.sendControl(jsonFrame(T.event, 0, { t: "channel.offer", ticket: "nope" }));
    const ch = await hub.offer();
    await ch.waitFor((f) => f.type === T.credit);
    expect(run.diags.join("\n")).toContain("a channel.offer without its fields");
    expect(await run.cleanup()).toBe(0);
  });
});
