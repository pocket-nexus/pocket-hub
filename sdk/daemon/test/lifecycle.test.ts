// The daemon process's life, as a real child `bun` process started with the
// four POCKET_HUB_DAEMON_* variables (DAEMON-PROTOCOL.md §2, §4): hello and
// ready, a refused hello, offers, `daemon.stop`, the control connection
// closing, and handlers that fail without ending the process.

import { afterEach, describe, expect, test } from "bun:test";
import { join } from "node:path";
import { type Child, spawnDaemon } from "./child.ts";
import { inProcess } from "./helpers.ts";
import { MockHub, T } from "./mock-hub.ts";

const ENTRY = join(import.meta.dir, "fixtures", "lifecycle-daemon.ts");

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

describe("a daemon process", () => {
  test("hello → ready, an offer → attach(info), daemon.stop → stop(), channels closed, exit 0", async () => {
    hub = await MockHub.start({ app: "dev.pocket-hub.lifecycle" });
    child = spawnDaemon(ENTRY, hub.env(), join(import.meta.dir, "fixtures"));
    await hub.waitControl(5000);
    expect(hub.hellos).toEqual([{ t: "daemon.hello", protocol: 1, token: hub.token, app: "dev.pocket-hub.lifecycle" }]);
    await child.line((l) => l === `started dev.pocket-hub.lifecycle ${hub!.data}`);
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
    child = spawnDaemon(ENTRY, hub.env());
    expect(await child.exited).toBe(1);
    expect(child.err.join("\n")).toContain("Hub refused the daemon: unknown token");
  });

  test("the control connection closing ends the process", async () => {
    hub = await MockHub.start();
    child = spawnDaemon(ENTRY, hub.env());
    await hub.waitControl(5000);
    const ch = await hub.offer({}, { timeoutMs: 5000 });
    hub.closeControl();
    expect(await child.exited).toBe(0);
    await ch.closed;
    expect(child.out).toContain("stop called");
  });

  test("handlers that throw, reject, or leave a rejection unhandled do not end the process", async () => {
    hub = await MockHub.start();
    child = spawnDaemon(ENTRY, hub.env());
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

  test("without the four variables the program says why and exits 1", async () => {
    const env: Record<string, string> = {};
    for (const [k, v] of Object.entries(process.env)) if (!k.startsWith("POCKET_HUB_") && v !== undefined) env[k] = v;
    const proc = Bun.spawn([process.execPath, "run", ENTRY], { env, stdout: "pipe", stderr: "pipe" });
    expect(await proc.exited).toBe(1);
    expect(await new Response(proc.stderr).text()).toContain("POCKET_HUB_DAEMON_SOCKET");
  });
});

describe("in process", () => {
  test("a daemon whose factory throws says no hello and ends with 1", async () => {
    const run = await inProcess(() => {
      throw new Error("cannot start");
    }, { refuse: "unused" });
    hub = run.hub;
    expect(await run.done).toBe(1);
    expect(run.hub.hellos).toEqual([]);
    expect(run.diags.join("\n")).toContain("cannot start");
  });

  test("ready: hello waits for the daemon's own ready promise", async () => {
    let resolveReady!: () => void;
    const ready = new Promise<void>((resolve) => (resolveReady = resolve));
    const started = MockHub.start();
    hub = await started;
    const { runDaemon } = await import("../src/index.ts");
    const run = runDaemon({ ready, attach() {} }, { ...hub.runOptions(), log: () => {}, diag: () => {} });
    await Bun.sleep(30);
    expect(hub.hellos).toEqual([]);
    resolveReady();
    await hub.waitControl();
    hub.stop();
    expect(await run.done).toBe(0);
  });

  test("offers that come before daemon.ready is read, and a malformed offer, are handled", async () => {
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
