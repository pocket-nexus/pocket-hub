// The two ported services, each run as Hub runs a service package: its
// manifest's entry, in its folder, with the four variables, against the mock
// Hub playing the applet with the message sequences the originals expect
// (PocketJS tools/pocket-link/v2/daemon-sample.ts's header;
// apps/link-channel/app.tsx's script and `linkBench`).

import { afterEach, describe, expect, test } from "bun:test";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { fnv, patternBytes } from "../examples/link-channel/http-fixture.ts";
import { type Child, spawnService } from "./child.ts";
import { pattern } from "./helpers.ts";
import { type MFrame, MockHub, T } from "./mock-hub.ts";

const EXAMPLES = join(import.meta.dir, "..", "examples");
const crc = (bytes: Uint8Array) => Bun.hash.crc32(bytes) >>> 0;

interface Manifest {
  service: number;
  id: string;
  version: string;
  entry: string;
  idleSeconds?: number;
}
const manifest = (name: string): Manifest => JSON.parse(readFileSync(join(EXAMPLES, name, "pocket.service.json"), "utf8"));

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

async function start(name: string, env: Record<string, string> = {}) {
  const m = manifest(name);
  expect(m.service).toBe(1);
  hub = await MockHub.start({ app: m.id });
  child = spawnService(m.entry, { ...hub.env(), ...env }, join(EXAMPLES, name));
  await hub.waitControl(5000);
  return { hub, child, manifest: m };
}

const events = (frames: MFrame[], t: string) => frames.filter((f) => f.type === T.event && f.json?.t === t);

describe("examples/sample (daemon-sample.ts)", () => {
  test("echo, time, blob, sink, slow and its cancel, ticks, note, upload", async () => {
    const { hub, child } = await start("sample");
    const ch = await hub.offer({ generation: 5, hash: "0011223344556677" }, { timeoutMs: 5000 });
    await child.line((l) => l === "device 0123456789abcdef opened generation 5 (package 0011223344556677)");
    ch.grantUnits(48);
    ch.autoUnits = true;

    expect((await ch.reply(ch.request("echo", { a: 1, b: "two" }))).json).toEqual({ echo: { a: 1, b: "two" } });
    const time = (await ch.reply(ch.request("time"))).json!;
    expect(Math.abs(time.utc - Date.now())).toBeLessThan(5000);

    // blob: the head event, then the bytes on the request's stream, then {bytes, crc32}.
    const blob = ch.request("blob", { bytes: 100_000, seed: 9 });
    const head = await ch.waitFor((f) => f.type === T.event && f.id === blob);
    expect(head.json).toEqual({ t: "blob.head", bytes: 100_000 });
    const bytes = await ch.readStream(blob, 32 * 1024);
    expect(crc(bytes)).toBe(crc(pattern(100_000, 9)));
    expect((await ch.reply(blob)).json).toEqual({ bytes: 100_000, crc32: crc(pattern(100_000, 9)) });
    const empty = ch.request("blob", { bytes: 0 });
    expect((await ch.readStream(empty)).length).toBe(0);
    expect((await ch.reply(empty)).json).toEqual({ bytes: 0, crc32: 0 });
    expect((await ch.reply(ch.request("blob", { bytes: -1 }))).json).toEqual({ code: "invalid", message: `bytes is 0 to ${4 * 1024 * 1024}` });

    // sink: a body in, {bytes, crc32} out.
    const sink = ch.request("sink", {}, { body: true });
    const body = pattern(50_000, 4);
    await ch.sendStream(sink, body);
    expect((await ch.reply(sink)).json).toEqual({ bytes: 50_000, crc32: crc(body) });
    expect((await ch.reply(ch.request("sink"))).json).toEqual({ code: "invalid", message: "sink takes a body" });

    // slow: answered after `ms`, or nothing at all once the applet cancels (Hub answered it).
    const cancelled = ch.request("slow", { ms: 20_000 });
    await Bun.sleep(20);
    ch.cancel(cancelled);
    await child.line((l) => l === "slow: cancelled by the applet");
    expect((await ch.reply(ch.request("slow", { ms: 20 }))).json).toEqual({ waited: 20 });

    // ticks: {started}, then `count` events `tick` {n, of} (the full-window case is the next test).
    expect((await ch.reply(ch.request("ticks", { count: 5, everyMs: 10 }))).json).toEqual({ started: 5 });
    await ch.waitFor((f) => f.type === T.event && f.json?.t === "tick" && f.json?.n === 4, 3000);
    expect(events(ch.frames, "tick").map((f) => f.json)).toEqual([0, 1, 2, 3, 4].map((n) => ({ t: "tick", n, of: 5 })));

    // note → note.seen
    ch.event("note", { text: "hello service" });
    expect((await ch.waitFor((f) => f.type === T.event && f.json?.t === "note.seen")).json).toEqual({ t: "note.seen", text: "hello service" });
    await child.line((l) => l === "note: hello service");

    // upload: a stream event in, upload.done out.
    const up = ch.event("upload", { name: "u" }, { stream: true });
    const upBytes = pattern(300_000, 2);
    await ch.sendStream(up, upBytes);
    expect((await ch.waitFor((f) => f.type === T.event && f.json?.t === "upload.done")).json).toEqual({ t: "upload.done", bytes: 300_000, crc32: crc(upBytes) });

    ch.close("exit");
    await child.line((l) => l === "generation 5 closed: exit");
    hub.stop();
    expect(await child.exited).toBe(0);
  }, 20_000);

  test("ticks with a full window: nothing dropped, nothing past the units", async () => {
    const { hub, child } = await start("sample");
    const ch = await hub.offer({}, { timeoutMs: 5000 });
    await child.line((l) => l.includes("opened generation"));
    ch.grantUnits(2);
    expect((await ch.reply(ch.request("ticks", { count: 6, everyMs: 10 }))).json).toEqual({ started: 6 });
    await Bun.sleep(150);
    expect(events(ch.frames, "tick").map((f) => f.json?.n)).toEqual([0, 1]);
    ch.grantUnits(4);
    await ch.waitFor((f) => f.type === T.event && f.json?.t === "tick" && f.json?.n === 5, 3000);
    expect(events(ch.frames, "tick").map((f) => f.json?.n)).toEqual([0, 1, 2, 3, 4, 5]);
    hub.stop();
    expect(await child.exited).toBe(0);
  }, 10_000);
});

describe("examples/link-channel (apps/link-channel/daemon.ts)", () => {
  test("the applet's script, its bench, and the close report", async () => {
    const { hub, child, manifest: m } = await start("link-channel", { LINK_CHANNEL_HTTP_PORT: "0" });
    expect(m.id).toBe("dev.pocket-nexus.link-channel");
    const fixture = await child.line((l) => l.startsWith("http fixture on 127.0.0.1:"));
    const port = Number(/127\.0\.0\.1:(\d+)/.exec(fixture)![1]);
    // The worker's HTTP origin the module starts answers as the original's does.
    expect(await (await fetch(`http://127.0.0.1:${port}/hello?from=test`)).text()).toBe("hello from Hub");

    const ch = await hub.offer({ generation: 12 }, { timeoutMs: 5000 });
    await child.line((l) => l === "channel open: generation 12");
    ch.grantUnits(48);
    ch.autoUnits = true;
    // The service's `tick` events, every 100 ms while the channel is open.
    await ch.waitFor((f) => f.type === T.event && f.json?.t === "tick" && f.json?.n === 1, 3000);

    // 1. hello → the service's own request `applet.sum`, which the applet answers.
    ch.event("hello", { generation: 12 });
    const sum = await ch.waitFor((f) => f.type === T.request && f.json?.t === "applet.sum");
    expect(sum.json).toEqual({ t: "applet.sum", a: 19, b: 23 });
    expect(sum.id).toBeGreaterThan(0x8000_0000);
    ch.replyTo(sum.id, { sum: 42 });

    // 2. echo
    const echo = (await ch.reply(ch.request("echo", { text: "hello" }))).json;
    expect(echo).toEqual({ echo: "hello" });

    // 3. blob: 48,000 bytes behind the head event, the trailer with their FNV-1a.
    const blob = ch.request("blob", { bytes: 48_000 });
    expect((await ch.waitFor((f) => f.type === T.event && f.id === blob)).json).toEqual({ t: "blob.head", bytes: 48_000 });
    const received = await ch.readStream(blob, 32 * 1024);
    const trailer = (await ch.reply(blob)).json;
    expect(received.length).toBe(48_000);
    expect(trailer).toEqual({ bytes: 48_000, hash: fnv(patternBytes(48_000)) });
    expect(fnv(received)).toBe(trailer!.hash);

    // 4. note: an event with a stream toward the service.
    const note = new Uint8Array(3000).map((_, i) => (i * 7) & 0xff);
    const noteId = ch.event("note", { bytes: note.length }, { stream: true });
    await ch.sendStream(noteId, note);

    // 5. slow, cancelled: Hub answers the applet `cancelled`; the service sends nothing for it.
    const slow = ch.request("slow", {});
    await Bun.sleep(10);
    ch.cancel(slow);

    // 6. the report
    const report = { echo, blob: { received: 48_000, hash: fnv(received), trailer }, note: { sent: 3000, hash: fnv(note) }, cancelled: "cancelled", ticks: 3, sums: 1, http: "" };
    expect((await ch.reply(ch.request("report", report))).json).toEqual({ ok: true });
    ch.event("http", { result: "200 14 hello from Hub" });

    // The bench: a response stream down, an upload, events each way, a report line.
    const down = ch.request("bench.down", { bytes: 1024 * 1024 });
    expect((await ch.waitFor((f) => f.type === T.event && f.id === down)).json).toEqual({ t: "bench.head", bytes: 1024 * 1024 });
    const downBytes = await ch.readStream(down, 32 * 1024);
    expect(fnv(downBytes)).toBe(fnv(patternBytes(1024 * 1024)));
    expect((await ch.reply(down)).json).toMatchObject({ bytes: 1024 * 1024 });

    const upId = ch.event("bench.up", { bytes: 500_000 }, { stream: true });
    await ch.sendStream(upId, patternBytes(500_000), 16_384);
    expect((await ch.reply(ch.request("bench.up.result"))).json).toMatchObject({ bytes: 500_000, ok: true });

    expect((await ch.reply(ch.request("bench.events.result"))).json).toMatchObject({ count: 0 });
    for (let n = 0; n < 100; n++) ch.event("bench.event", { n });
    expect((await ch.reply(ch.request("bench.events.result"))).json).toMatchObject({ count: 100 });

    const eventsDown = ch.request("bench.events.down", { count: 300 });
    expect((await ch.reply(eventsDown, 10_000)).json).toMatchObject({ sent: 300 });
    expect(events(ch.frames, "bench.tick").map((f) => f.json?.n)).toEqual(Array.from({ length: 300 }, (_, n) => n));

    expect((await ch.reply(ch.request("bench.report", { k: "down", n: 1, ms: 2 }))).json).toEqual({ ok: true });
    await child.line((l) => l === 'bench {"k":"down","n":1,"ms":2}');

    // The applet exits: the service logs what it saw, as the original does.
    ch.close("exit");
    const closed = await child.line((l) => l.startsWith("closed (exit): report "));
    expect(closed).toBe(`closed (exit): report ${JSON.stringify(report)}, note ${JSON.stringify({ bytes: 3000, hash: fnv(note) })}, http 200 14 hello from Hub`);
    // Ticks stop with the channel: nothing after the close (the mock records a frame after it as a violation only for the service's own close).
    const ticksAtClose = events(ch.frames, "tick").length;
    await Bun.sleep(250);
    expect(events(ch.frames, "tick").length).toBe(ticksAtClose);

    hub.stop();
    expect(await child.exited).toBe(0);
  }, 30_000);
});
