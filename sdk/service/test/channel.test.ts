// The channel's behaviour against the mock Hub, in this process. The mock
// records every rule of SERVICE-PROTOCOL.md §5 the service's frames break;
// each test ends by asserting there is none.

import { afterEach, describe, expect, test } from "bun:test";
import { type AppletCall, type AppletChannel, type ChannelInfo, ServiceError, type StreamReader, type StreamWriter } from "../src/index.ts";
import { inProcess, pattern, until, type InProcess } from "./helpers.ts";
import { type MockChannel, T, frameBytes, units } from "./mock-hub.ts";

let run: InProcess | null = null;
afterEach(async () => {
  if (run) {
    expect(run.hub.violations).toEqual([]);
    await run.cleanup();
  }
  run = null;
});

/** Starts a service whose attach() hands the test its channel and info. */
async function open(setup: (channel: AppletChannel, info: ChannelInfo) => void = () => {}, offer: Parameters<InProcess["hub"]["offer"]>[0] = {}) {
  let channel: AppletChannel | null = null;
  let info: ChannelInfo | null = null;
  run = await inProcess({
    attach(c, i) {
      channel = c;
      info = i;
      setup(c, i);
    },
  });
  const ch = await run.hub.offer(offer);
  await until(() => channel !== null);
  return { ch, channel: channel! as AppletChannel, info: info! as ChannelInfo, hub: run.hub };
}

const crc = (bytes: Uint8Array) => Bun.hash.crc32(bytes) >>> 0;

describe("opening a channel", () => {
  test("offer → channel.attach → channel.ready → 256 units → attach(channel, info)", async () => {
    const { ch, channel, info } = await open(() => {}, { generation: 77, deviceName: "Lab 3DS", payloadLimit: 4096 });
    expect(info).toEqual({ device: "0123456789abcdef", deviceName: "Lab 3DS", model: "new-3ds-xl", target: "3ds", app: run!.hub.app, hash: "fedcba9876543210", generation: 77 });
    // The first frame after `channel.ready` is the service's grant of 256 units on the channel id.
    await ch.waitFor((f) => f.type === T.credit);
    expect(ch.frames[0]).toMatchObject({ type: T.credit, id: ch.channel, value: 256 });
    expect(channel.id).toBe(ch.channel);
    expect(channel.payloadLimit).toBe(4096);
    expect(channel.messageLimit).toBe(4092);
    expect(channel.state()).toEqual({ state: "open", reason: null });
    expect(channel.generation).toBe(77);
  });

  test("frames right behind channel.ready reach handlers an async attach() registers", async () => {
    let seen: unknown = null;
    run = await inProcess({
      async attach(channel) {
        await Bun.sleep(20);
        channel.on("early", (args) => {
          seen = args;
          return { ok: 1 };
        });
      },
    });
    const ch = await run.hub.offer();
    ch.request("early", { n: 1 }, { id: 0x65 });
    expect((await ch.reply(0x65)).json).toEqual({ ok: 1 });
    expect(seen).toEqual({ n: 1 });
  });

  test("a refused ticket leaves the service running", async () => {
    let attached = 0;
    run = await inProcess({ attach: () => void (attached += 1) });
    await expect(run.hub.offer({}, { refuse: "expired" })).rejects.toThrow("refused");
    const ch = await run.hub.offer();
    await ch.waitFor((f) => f.type === T.credit);
    expect(attached).toBe(1);
    expect(run.diags.some((line) => line.includes("refused: expired"))).toBe(true);
  });
});

describe("requests from the applet", () => {
  test("a reply, an error reply with extra fields, a thrown Error, an unknown t, a malformed request", async () => {
    const { ch } = await open((channel) => {
      channel.on("echo", (args) => ({ echo: args }));
      channel.on("later", async (args) => {
        await Bun.sleep(5);
        return { n: Number(args.n) + 1 };
      });
      channel.on("deny", () => {
        throw new ServiceError("not_yours", "that file is not yours", { path: "/x" });
      });
      channel.on("boom", () => {
        throw new Error("kaboom");
      });
      channel.on("void", () => {});
    });
    ch.request("echo", { a: [1, 2], s: "é" }, { id: 0x65 });
    ch.request("later", { n: 41 }, { id: 0x66 });
    ch.request("deny", {}, { id: 0x67 });
    ch.request("boom", {}, { id: 0x68 });
    ch.request("missing", {}, { id: 0x69 });
    ch.request("void", {}, { id: 0x6a });
    ch.appletRequests.add(0x6b);
    ch.send(new Uint8Array([0x10, 0, 0, 0, 2, 0, 0, 0, 0x6b, 0, 0, 0, 0x5b, 0x5d])); // `[]`: not an object
    expect((await ch.reply(0x65)).json).toEqual({ echo: { a: [1, 2], s: "é" } });
    expect((await ch.reply(0x66)).json).toEqual({ n: 42 });
    const deny = await ch.reply(0x67);
    expect(deny.flags).toBe(1);
    expect(deny.json).toEqual({ path: "/x", code: "not_yours", message: "that file is not yours" });
    expect((await ch.reply(0x68)).json).toEqual({ code: "internal", message: "kaboom" });
    expect((await ch.reply(0x69)).json).toEqual({ code: "unknown_message", message: "no handler for missing" });
    expect((await ch.reply(0x6a)).json).toEqual({});
    expect((await ch.reply(0x6b)).json).toMatchObject({ code: "malformed" });
    expect(run!.diags.some((line) => line.includes("kaboom"))).toBe(true);
  });

  test("an error message is cut to 240 encoded bytes; an invalid code becomes internal", async () => {
    const { ch } = await open((channel) => {
      channel.on("long", () => {
        throw new ServiceError("long", "é".repeat(300));
      });
      channel.on("bad", () => {
        throw new ServiceError("Bad Code", "x");
      });
    });
    ch.request("long", {}, { id: 0x65 });
    ch.request("bad", {}, { id: 0x66 });
    const long = (await ch.reply(0x65)).json!;
    expect(new TextEncoder().encode(long.message).length).toBe(240);
    expect((await ch.reply(0x66)).json).toEqual({ code: "internal", message: "Bad Code: x" });
  });

  test("a ninth open request of the applet is answered busy", async () => {
    const holds: (() => void)[] = [];
    const { ch } = await open((channel) => channel.on("hold", () => new Promise<void>((resolve) => holds.push(resolve))));
    for (let i = 0; i < 9; i++) ch.request("hold", {}, { id: 0x65 + i });
    expect((await ch.reply(0x65 + 8)).json).toMatchObject({ code: "busy" });
    for (const release of holds) release();
    for (let i = 0; i < 8; i++) expect((await ch.reply(0x65 + i)).json).toEqual({});
  });
});

describe("requests from the service", () => {
  test("a reply, an error, and the outcomes Hub makes: timeout, busy, too_large, cancelled", async () => {
    const { ch, channel } = await open();
    const calls = ["a", "b", "c", "d", "e", "f"].map((t) => channel.request(t, { x: t }));
    expect(calls.map((c) => c.id)).toEqual([0x8000_0001, 0x8000_0002, 0x8000_0003, 0x8000_0004, 0x8000_0005, 0x8000_0006]);
    await ch.waitFor((f) => f.type === T.request && f.id === 0x8000_0006);
    expect(ch.frames.filter((f) => f.type === T.request).map((f) => f.json)).toEqual(["a", "b", "c", "d", "e", "f"].map((t) => ({ t, x: t })));
    ch.replyTo(0x8000_0001, { sum: 42 });
    ch.replyError(0x8000_0002, "nope", "the applet says no");
    ch.replyError(0x8000_0003, "timeout", "30 s without a frame");
    ch.replyError(0x8000_0004, "busy");
    ch.replyError(0x8000_0005, "too_large");
    ch.replyError(0x8000_0006, "cancelled");
    expect(await calls[0]!.reply).toEqual({ sum: 42 });
    for (const [i, code] of [[1, "nope"], [2, "timeout"], [3, "busy"], [4, "too_large"], [5, "cancelled"]] as const) {
      const error = await calls[i]!.reply.catch((e: unknown) => e);
      expect(error).toBeInstanceOf(ServiceError);
      expect((error as ServiceError).code).toBe(code);
    }
    expect(((await calls[1]!.reply.catch((e: unknown) => e)) as ServiceError).message).toBe("the applet says no");
  });

  test("a second REPLY and a REPLY for an unknown id are ignored", async () => {
    const { ch, channel } = await open();
    const call = channel.request("q");
    await ch.waitFor((f) => f.type === T.request);
    ch.replyTo(call.id, { first: true });
    ch.replyTo(call.id, { second: true });
    ch.replyTo(0x8000_0099, {});
    expect(await call.reply).toEqual({ first: true });
    await ch.settle();
    expect(channel.state().state).toBe("open");
  });

  test("a ninth request fails locally with busy, and nothing reaches Hub", async () => {
    const { ch, channel } = await open();
    const calls = Array.from({ length: 8 }, () => channel.request("hold"));
    const ninth = channel.request("hold");
    expect(ninth.id).toBe(0);
    expect(((await ninth.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("busy");
    await ch.settle();
    expect(ch.frames.filter((f) => f.type === T.request).length).toBe(8);
    // A reply frees a place.
    ch.replyTo(calls[0]!.id, {});
    await calls[0]!.reply;
    const next = channel.request("hold");
    expect(next.id).toBe(0x8000_0009);
  });

  test("names and sizes refused locally: malformed, too_large; no_channel after close", async () => {
    const { ch, channel } = await open(() => {}, { payloadLimit: 1024 });
    expect(channel.messageLimit).toBe(1020);
    const bad = channel.request("Not A Name");
    expect(((await bad.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("malformed");
    // {"t":"big","s":"…"}: 18 bytes around the string.
    const fits = channel.request("big", { s: "x".repeat(1020 - 18) });
    const over = channel.request("big", { s: "x".repeat(1020 - 17) });
    expect(fits.id).not.toBe(0);
    expect(((await over.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("too_large");
    await ch.waitFor((f) => f.type === T.request);
    expect(ch.frames.find((f) => f.type === T.request)!.payload.length).toBe(1020);
    channel.close();
    expect(((await channel.request("q").reply.catch((e: unknown) => e)) as ServiceError).code).toBe("no_channel");
    expect(((await fits.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("closed");
  });
});

describe("events and their units", () => {
  test("emit is false at zero units and true after a grant; a unit per started 512 bytes", async () => {
    const { ch, channel } = await open();
    expect(channel.units).toBe(0);
    expect(channel.emit("tick", { n: 0 })).toBe(false);
    ch.grantUnits(2);
    await until(() => channel.units === 2);
    expect(channel.emit("tick", { n: 1 })).toBe(true);
    expect(channel.emit("tick", { n: 2 })).toBe(true);
    expect(channel.emit("tick", { n: 3 })).toBe(false);
    expect(channel.units).toBe(0);
    // {"t":"big","s":"…"} of 513 bytes costs 2 units.
    const big = { s: "x".repeat(513 - 18) };
    ch.grantUnits(1);
    await until(() => channel.units === 1);
    expect(channel.emit("big", big)).toBe(false);
    ch.grantUnits(1);
    await until(() => channel.units === 2);
    expect(channel.emit("big", big)).toBe(true);
    expect(channel.units).toBe(0);
    await ch.waitFor((f) => f.type === T.event && f.json?.t === "big");
    const sent = ch.frames.filter((f) => f.type === T.event);
    expect(sent.map((f) => f.json?.n ?? f.payload.length)).toEqual([1, 2, 513]);
    expect(ch.unitsForService).toBe(0);
  });

  test("emit throws for an event that can never be sent", async () => {
    const { channel } = await open(() => {}, { payloadLimit: 1024 });
    expect(() => channel.emit("Bad!", {})).toThrow(ServiceError);
    try {
      channel.emit("big", { s: "x".repeat(1100) });
      throw new Error("no throw");
    } catch (error) {
      expect((error as ServiceError).code).toBe("too_large");
    }
    expect(() => channel.emitStream("big", { s: "x".repeat(1100) })).toThrow(ServiceError);
  });

  test("progress events count against the applet's units", async () => {
    let call: AppletCall | null = null;
    let release!: () => void;
    const { ch } = await open((channel) =>
      channel.on("work", (_args, c) => {
        call = c;
        return new Promise<void>((resolve) => (release = resolve));
      }),
    );
    ch.request("work", {}, { id: 0x65 });
    await until(() => call !== null);
    expect(call!.progress("step", { n: 0 })).toBe(false);
    ch.grantUnits(3);
    await until(() => call!.progress("step", { n: 1 }));
    expect(call!.progress("step", { n: 2 })).toBe(true);
    expect(call!.progress("step", { n: 3, pad: "x".repeat(600) })).toBe(false); // 2 units, 1 left
    expect(call!.progress("step", { n: 4 })).toBe(true);
    expect(call!.progress("step", { n: 5 })).toBe(false);
    release();
    await ch.reply(0x65);
    expect(ch.frames.filter((f) => f.type === T.event && f.id === 0x65).map((f) => f.json?.n)).toEqual([1, 2, 4]);
    // Progress after the reply: false, nothing sent.
    ch.grantUnits(5);
    await ch.settle();
    expect(call!.progress("late")).toBe(false);
  });

  test("the service regrants exactly the units of the events it takes, progress included", async () => {
    const got: string[] = [];
    const { ch, channel } = await open((c) => {
      c.onEvent("a", (args) => void got.push(`a${String(args.n)}`));
    });
    await ch.waitFor((f) => f.type === T.credit);
    expect(ch.unitsFromService).toBe(256);
    ch.event("a", { n: 1 });
    ch.event("a", { n: 2 });
    ch.event("a", { n: 3, pad: "y".repeat(1200) }); // 3 units
    ch.event("unknown", {}); // no handler: still leaves the queue, still regranted
    await until(() => ch.unitGrants.slice(1).reduce((a, b) => a + b, 0) === 6, 3000, "a regrant of 6 units");
    expect(got).toEqual(["a1", "a2", "a3"]);
    expect(ch.unitsFromService).toBe(256);
    // Progress on the service's request: counted and regranted.
    const progress: string[] = [];
    const call = channel.request("q", {}, { onProgress: (t) => void progress.push(t) });
    await ch.waitFor((f) => f.type === T.request);
    ch.progress(call.id, "step", { pad: "z".repeat(700) }); // 2 units
    ch.replyTo(call.id, {});
    await call.reply;
    await until(() => ch.unitsFromService === 256);
    expect(progress).toEqual(["step"]);
    expect(channel.stats.eventsIn).toBe(5);
    expect(channel.stats.unitsRegranted).toBe(8);
  });

  test("an event past the units the service granted closes the channel with overflow", async () => {
    let reason: string | null = null;
    const { ch, channel } = await open((c) => c.onClose((r) => (reason = r)));
    await ch.waitFor((f) => f.type === T.credit);
    // 257 one-unit events in one write: the service has regranted nothing yet when the 257th arrives.
    const one = frameBytes(T.event, 0, 0, new TextEncoder().encode('{"t":"e"}'));
    const flood = new Uint8Array(one.length * 257);
    for (let i = 0; i < 257; i++) flood.set(one, i * one.length);
    ch.send(flood);
    const close = await ch.waitFor((f) => f.type === T.event && f.id === ch.channel);
    expect(close.json).toEqual({ t: "channel.close", reason: "overflow" });
    await ch.closed;
    expect(reason as string | null).toBe("overflow");
    expect(channel.state()).toEqual({ state: "closed", reason: "overflow" });
  });
});

describe("streams", () => {
  test("1 MiB toward the applet: within credit, no frame over the payload limit", async () => {
    const size = 1024 * 1024;
    const { ch } = await open(
      (channel) =>
        channel.on("blob", async (args, call) => {
          const bytes = pattern(Number(args.bytes), 7);
          while (!call.progress("blob.head", { bytes: bytes.length })) await Bun.sleep(1);
          const out = call.respond();
          await out.writeAll(bytes);
          out.end();
          await out.finished;
          return { bytes: bytes.length, crc32: crc(bytes) };
        }),
      { payloadLimit: 4096 },
    );
    ch.grantUnits(48);
    const id = ch.request("blob", { bytes: size });
    await ch.waitFor((f) => f.type === T.event && f.id === id);
    const got = await ch.readStream(id, 32 * 1024);
    const reply = await ch.reply(id);
    expect(got.length).toBe(size);
    expect(reply.json).toEqual({ bytes: size, crc32: crc(pattern(size, 7)) });
    expect(crc(got)).toBe(crc(pattern(size, 7)));
    expect(ch.maxData).toBe(4096);
    // The REPLY came after the stream's END (§5.2).
    const end = ch.frames.findIndex((f) => f.type === T.data && (f.flags & 1) !== 0);
    expect(ch.frames.findIndex((f) => f.type === T.reply)).toBeGreaterThan(end);
  });

  test("1 MiB from the applet: the service's outstanding credit never passes 256 KiB", async () => {
    const size = 1024 * 1024;
    let result: { bytes: number; crc32: number } | null = null;
    const { ch } = await open((channel) =>
      channel.onEvent("upload", async (_args, stream) => {
        let bytes = 0;
        let sum = 0;
        for (let chunk = await stream!.read(); chunk !== null; chunk = await stream!.read()) {
          bytes += chunk.length;
          sum = Bun.hash.crc32(chunk, sum);
        }
        result = { bytes, crc32: sum >>> 0 };
      }),
    );
    const id = ch.event("upload", { bytes: size }, { stream: true });
    const data = pattern(size, 3);
    await ch.sendStream(id, data);
    await until(() => result !== null);
    expect(result!).toEqual({ bytes: size, crc32: crc(data) });
    // Credit granted and not yet used by the applet, at its highest: the 256 KiB window.
    expect(ch.mostCreditFrom.get(id)).toBe(256 * 1024);
    const grants = ch.frames.filter((f) => f.type === T.credit && f.id === id);
    expect(grants[0]!.value).toBe(256 * 1024);
    // Refills come in quarters of the window or more, not one per frame.
    expect(grants.every((g) => g.value >= 64 * 1024)).toBe(true);
    expect(grants.length).toBeLessThan(size / (64 * 1024) + 2);
  });

  test("credit is granted as the service's code reads, not before", async () => {
    let stream: StreamReader | null = null;
    const { ch } = await open((channel) => channel.onEvent("up", (_args, s) => void (stream = s!)));
    const id = ch.event("up", {}, { stream: true });
    await until(() => stream !== null);
    await ch.settle();
    expect(ch.creditFrom(id)).toBe(0);
    const first = stream!.read();
    await ch.waitCredit(id, 256 * 1024);
    ch.data(id, pattern(100));
    expect((await first)!.length).toBe(100);
    // 100 bytes of a 256 KiB window read: no new grant for so little.
    void stream!.read();
    await ch.settle();
    expect(ch.frames.filter((f) => f.type === T.credit && f.id === id).length).toBe(1);
    ch.data(id, pattern(0), true);
  });

  test("the head step: respond() before a progress event is refused before a byte goes out", async () => {
    let refused: unknown = null;
    const { ch } = await open((channel) =>
      channel.on("eager", (_args, call) => {
        try {
          call.respond();
        } catch (error) {
          refused = error;
        }
        return { refused: refused !== null };
      }),
    );
    const id = ch.request("eager");
    expect((await ch.reply(id)).json).toEqual({ refused: true });
    expect(refused).toBeInstanceOf(ServiceError);
    expect((refused as ServiceError).message).toContain("head step");
    expect(ch.frames.some((f) => f.type === T.data)).toBe(false);
  });

  test("a request with a body, each way", async () => {
    let bodyNull = false;
    const { ch, channel } = await open((c) => {
      c.on("sink", async (_args, call) => {
        if (call.body === null) return { none: true };
        let bytes = 0;
        for (let chunk = await call.body.read(); chunk !== null; chunk = await call.body.read()) bytes += chunk.length;
        return { bytes };
      });
      c.on("nobody", (_args, call) => {
        bodyNull = call.body === null;
        return {};
      });
    });
    const id = ch.request("sink", {}, { body: true });
    await ch.sendStream(id, pattern(70_000));
    expect((await ch.reply(id)).json).toEqual({ bytes: 70_000 });
    const empty = ch.request("sink", {}, { body: true });
    ch.data(empty, new Uint8Array(0), true);
    expect((await ch.reply(empty)).json).toEqual({ bytes: 0 });
    await ch.reply(ch.request("nobody"));
    expect(bodyNull).toBe(true);

    // The service's request with a body: bytes only within the applet's credit, END last, then the applet replies.
    const call = channel.request("store", { name: "x" }, { body: true });
    expect(call.body).not.toBeNull();
    const writing = (async () => {
      await call.body!.writeAll(pattern(100_000, 9));
      call.body!.end();
      await call.body!.finished;
    })();
    const req = await ch.waitFor((f) => f.type === T.request && f.id === call.id);
    expect(req.flags).toBe(0x04);
    await ch.settle();
    expect(ch.frames.some((f) => f.type === T.data)).toBe(false); // no credit yet
    const got = await ch.readStream(call.id, 16 * 1024);
    await writing;
    expect(crc(got)).toBe(crc(pattern(100_000, 9)));
    ch.replyTo(call.id, { stored: got.length });
    expect(await call.reply).toEqual({ stored: 100_000 });
  });

  test("a response to the service's request: handed over at the head event, the reply after its bytes", async () => {
    const { ch, channel } = await open();
    ch.grantUnits(4);
    let reading: Promise<number> | null = null;
    let readSoFar = 0;
    const heads: unknown[] = [];
    const call = channel.request("fetch", {}, {
      onProgress(t, args, stream) {
        heads.push({ t, args, stream: stream !== undefined });
        if (stream && reading === null) {
          reading = (async () => {
            for (let c = await stream.read(); c !== null; c = await stream.read()) readSoFar += c.length;
            return readSoFar;
          })();
        }
      },
    });
    await ch.waitFor((f) => f.type === T.request);
    ch.progress(call.id, "fetch.head", { bytes: 300_000 });
    await ch.waitCredit(call.id, 1);
    expect(call.response).not.toBeNull();
    await ch.sendStream(call.id, pattern(300_000, 1));
    ch.replyTo(call.id, { done: true });
    // When the reply settles, the service's code has every byte of the response (§7.3: the REPLY is the trailer).
    const atReply = await call.reply.then(() => readSoFar);
    expect(atReply).toBe(300_000);
    expect(await reading!).toBe(300_000);
    expect(heads).toEqual([{ t: "fetch.head", args: { bytes: 300_000 }, stream: true }]);
  });

  test("a stream-opening event with no handler, or with a handler that takes no stream, is answered RESET refused", async () => {
    const plain: unknown[] = [];
    const { ch } = await open((c) => c.onEvent("note", (args) => void plain.push(args)));
    const none = ch.event("nobody", {}, { stream: true });
    const one = ch.event("note", { k: 1 }, { stream: true });
    const r1 = await ch.waitFor((f) => f.type === T.reset && f.id === none);
    const r2 = await ch.waitFor((f) => f.type === T.reset && f.id === one);
    expect([r1.value, r2.value]).toEqual([2, 2]);
    expect(plain).toEqual([{ k: 1 }]);
  });

  test("the applet resets a stream: the service's writer and reader fail with transfer", async () => {
    let writer: StreamWriter | null = null;
    let reader: StreamReader | null = null;
    const { ch, channel } = await open((c) => c.onEvent("up", (_a, s) => void (reader = s!)));
    ch.grantUnits(4);
    await until(() => channel.units === 4);
    writer = channel.emitStream("down", {});
    const down = await ch.waitFor((f) => f.type === T.event && f.json?.t === "down");
    const up = ch.event("up", {}, { stream: true });
    await until(() => reader !== null);
    const reading = reader!.read();
    ch.reset(down.id, "refused");
    ch.reset(up, "cancelled");
    expect(((await writer!.finished.catch((e: unknown) => e)) as ServiceError).code).toBe("transfer");
    expect(((await reading.catch((e: unknown) => e)) as ServiceError).extra).toEqual({ reset: "cancelled" });
    expect(writer!.write(new Uint8Array(1))).toBe(false);
  });

  test("a fifth stream toward the applet fails locally: emitStream null, a body busy, respond() busy", async () => {
    let call: AppletCall | null = null;
    let release!: () => void;
    const { ch, channel } = await open((c) =>
      c.on("dl", (_a, cl) => {
        call = cl;
        return new Promise<void>((resolve) => (release = resolve));
      }),
    );
    ch.grantUnits(48);
    await until(() => channel.units === 48);
    const writers = [channel.emitStream("s1"), channel.emitStream("s2"), channel.emitStream("s3")];
    const upload = channel.request("put", {}, { body: true });
    expect(writers.every((w) => w !== null)).toBe(true);
    expect(channel.emitStream("s5")).toBeNull();
    expect(((await channel.request("put", {}, { body: true }).reply.catch((e: unknown) => e)) as ServiceError).code).toBe("busy");
    ch.request("dl");
    await until(() => call !== null);
    expect(call!.progress("dl.head")).toBe(true);
    expect(() => call!.respond()).toThrow("streams toward the applet are open");
    // One stream ends: there is room again.
    writers[0]!.end();
    const response = call!.respond();
    expect(response).not.toBeNull();
    expect(channel.emitStream("s6")).toBeNull();
    release();
    upload.body!.end();
    await ch.settle(30);
    expect(ch.outboundStreams.size).toBeLessThanOrEqual(4);
  });
});

describe("cancellation", () => {
  test("a CANCEL from the applet stops the handler; nothing more is sent for the id", async () => {
    let cancelledSeen = false;
    let wrote = 0;
    const { ch } = await open((channel) =>
      channel.on("stream", async (_args, call) => {
        call.onCancel(() => (cancelledSeen = true));
        while (!call.progress("head")) await Bun.sleep(1);
        const out = call.respond();
        try {
          while (!call.cancelled) {
            await out.writeAll(new Uint8Array(4096));
            wrote += 4096;
          }
        } catch {
          // the writer fails with `cancelled` once the request is cancelled
        }
        return { wrote };
      }),
    );
    ch.grantUnits(4);
    const id = ch.request("stream");
    await ch.waitFor((f) => f.type === T.event && f.id === id);
    ch.credit(id, 8192);
    await ch.waitFor((f) => f.type === T.data && f.id === id);
    ch.cancel(id);
    await until(() => cancelledSeen);
    ch.credit(id, 1 << 20); // late credit: must not make the service send
    await ch.settle(40);
    // The mock records a REPLY, a progress event or DATA for the cancelled id as a violation.
    expect(ch.frames.some((f) => f.type === T.reply && f.id === id)).toBe(false);
  });

  test("onCancel registered after the cancel runs at once", async () => {
    let call: AppletCall | null = null;
    const { ch } = await open((channel) => channel.on("w", (_a, c) => ((call = c), new Promise(() => {}))));
    const id = ch.request("w");
    await until(() => call !== null);
    ch.cancel(id);
    await until(() => call!.cancelled);
    let ran = false;
    call!.onCancel(() => (ran = true));
    await until(() => ran);
  });

  test("the service's cancel(): CANCEL goes out, progress after it is dropped, the reply Hub makes settles the call", async () => {
    const { ch, channel } = await open();
    const progress: string[] = [];
    const call = channel.request("slow", {}, { onProgress: (t) => void progress.push(t) });
    await ch.waitFor((f) => f.type === T.request);
    call.cancel();
    call.cancel(); // once
    const cancel = await ch.waitFor((f) => f.type === T.cancel);
    expect(cancel.id).toBe(call.id);
    ch.progress(call.id, "late");
    ch.replyError(call.id, "cancelled", "cancelled");
    expect(((await call.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("cancelled");
    expect(progress).toEqual([]);
    expect(ch.frames.filter((f) => f.type === T.cancel).length).toBe(1);
    await until(() => ch.unitsFromService === 256);
  });
});

describe("closing", () => {
  test("channel.close from Hub with calls pending: each ends with closed, one onClose", async () => {
    const closes: string[] = [];
    let handler: AppletCall | null = null;
    let reader: StreamReader | null = null;
    const { ch, channel } = await open((c) => {
      c.onClose((reason) => closes.push(reason));
      c.onClose((reason) => closes.push(`second listener: ${reason}`));
      c.on("hang", (_a, call) => ((handler = call), new Promise(() => {})));
      c.onEvent("up", (_a, s) => void (reader = s!));
    });
    const pending = [channel.request("a"), channel.request("b", {}, { body: true })];
    ch.request("hang");
    ch.event("up", {}, { stream: true });
    await until(() => handler !== null && reader !== null);
    const read = reader!.read();
    let handlerCancelled = false;
    handler!.onCancel(() => (handlerCancelled = true));
    ch.close("exit");
    for (const call of pending) expect(((await call.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("closed");
    expect(((await read.catch((e: unknown) => e)) as ServiceError).code).toBe("closed");
    expect(((await pending[1]!.body!.finished.catch((e: unknown) => e)) as ServiceError).code).toBe("closed");
    await ch.closed;
    expect(closes).toEqual(["exit", "second listener: exit"]);
    expect(handlerCancelled).toBe(true);
    expect(channel.state()).toEqual({ state: "closed", reason: "exit" });
    expect(channel.emit("x")).toBe(false);
    channel.close("again"); // no second notice, no second onClose
    await Bun.sleep(10);
    expect(closes.length).toBe(2);
  });

  test("the service closes a channel: channel.close behind its queued frames, then the socket closes", async () => {
    const closes: string[] = [];
    const { ch, channel } = await open((c) => c.onClose((r) => closes.push(r)));
    ch.grantUnits(4);
    await until(() => channel.units === 4);
    const call = channel.request("q");
    channel.emit("bye", {});
    channel.close("done");
    const notice = await ch.waitFor((f) => f.type === T.event && f.id === ch.channel);
    expect(notice.json).toEqual({ t: "channel.close", reason: "done" });
    const order = ch.frames.map((f) => (f.type === T.event ? f.json?.t : f.type));
    expect(order.slice(-3)).toEqual([T.request, "bye", "channel.close"]);
    await ch.closed;
    expect(((await call.reply.catch((e: unknown) => e)) as ServiceError).code).toBe("closed");
    expect(closes).toEqual(["done"]);
  });

  test("the socket closing without a notice closes the channel once", async () => {
    const closes: string[] = [];
    const { ch } = await open((c) => c.onClose((r) => closes.push(r)));
    ch.closeSocket();
    await until(() => closes.length === 1);
    await Bun.sleep(10);
    expect(closes).toEqual(["connection"]);
  });

  test("two channels at once are isolated from each other", async () => {
    const seen: string[] = [];
    run = await inProcess({
      attach(channel, info) {
        channel.on("who", () => ({ generation: info.generation }));
        channel.onEvent("hi", () => void seen.push(`hi on ${info.generation}`));
        channel.onClose((reason) => seen.push(`closed ${info.generation}: ${reason}`));
      },
    });
    const a = await run.hub.offer({ generation: 100 });
    const b = await run.hub.offer({ generation: 200 });
    expect(a.channel).not.toBe(b.channel);
    const ia = a.request("who", {}, { id: 0x65 });
    const ib = b.request("who", {}, { id: 0x65 }); // the same applet id on another channel connection
    expect((await a.reply(ia)).json).toEqual({ generation: 100 });
    expect((await b.reply(ib)).json).toEqual({ generation: 200 });
    b.event("hi");
    await until(() => seen.length === 1);
    a.close("exit");
    await a.closed;
    b.event("hi");
    await until(() => seen.length === 3);
    expect(seen).toEqual(["hi on 200", "closed 100: exit", "hi on 200"]);
    // b still answers, and its ids start at the Hub range's first.
    expect((await b.reply(b.request("who", {}, { id: 0x66 }))).json).toEqual({ generation: 200 });
  });
});

describe("robustness", () => {
  test("a throwing event handler, a throwing onProgress and a throwing onClose do not stop the service", async () => {
    const { ch, channel } = await open((c) => {
      c.onEvent("bad", () => {
        throw new Error("event handler broke");
      });
      c.onEvent("badAsync", async () => {
        throw new Error("async event handler broke");
      });
      c.onClose(() => {
        throw new Error("onClose broke");
      });
      c.on("ok", () => ({ ok: true }));
    });
    ch.event("bad");
    ch.event("badAsync");
    const call = channel.request("q", {}, {
      onProgress() {
        throw new Error("onProgress broke");
      },
    });
    await ch.waitFor((f) => f.type === T.request);
    ch.progress(call.id, "p");
    ch.replyTo(call.id, {});
    await call.reply;
    expect((await ch.reply(ch.request("ok"))).json).toEqual({ ok: true });
    for (const what of ["event handler broke", "async event handler broke", "onProgress broke"]) expect(run!.diags.some((d) => d.includes(what))).toBe(true);
  });

  test("a reply that is not an object or not JSON is an internal error; a reply over messageLimit is too_large", async () => {
    const { ch } = await open(
      (c) => {
        c.on("array", () => [1, 2] as unknown as Record<string, unknown>);
        c.on("bigint", () => ({ n: 1n }));
        c.on("huge", () => ({ s: "x".repeat(2000) }));
      },
      { payloadLimit: 1024 },
    );
    expect((await ch.reply(ch.request("array"))).json).toMatchObject({ code: "internal" });
    expect((await ch.reply(ch.request("bigint"))).json).toMatchObject({ code: "internal" });
    expect((await ch.reply(ch.request("huge"))).json).toMatchObject({ code: "too_large" });
  });

  test("DATA past the service's credit closes the channel with overflow", async () => {
    const { ch, channel } = await open((c) => c.onEvent("up", (_a, s) => void s!.read()));
    const id = ch.event("up", {}, { stream: true });
    await ch.waitCredit(id, 1);
    ch.data(id, new Uint8Array(256 * 1024 + 1).subarray(0, 16_384), false);
    for (let i = 0; i < 16; i++) ch.data(id, new Uint8Array(16_384), false, true);
    const notice = await ch.waitFor((f) => f.type === T.event && f.id === ch.channel);
    expect(notice.json).toEqual({ t: "channel.close", reason: "overflow" });
    expect(channel.state().reason).toBe("overflow");
  });

  test("a mock channel's units: the test helper matches the protocol's arithmetic", () => {
    expect([units(1), units(512), units(513), units(8192)]).toEqual([1, 1, 2, 16]);
  });
});

export type { MockChannel };
