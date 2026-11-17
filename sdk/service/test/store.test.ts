// `host.store` and the members of capabilities this host does not provide
// (SERVICE-PROTOCOL.md §6, §7), against the mock host: each call's frames,
// credit both ways, the local refusals before a frame leaves, and CANCEL.

import { afterEach, describe, expect, test } from "bun:test";
import { type Host, ServiceError } from "../src/index.ts";
import { inProcess, pattern, until, type InProcess } from "./helpers.ts";

let run: InProcess | null = null;
afterEach(async () => {
  if (run) {
    expect(run.hub.violations).toEqual([]);
    await run.cleanup();
  }
  run = null;
});

/** A service in this process with `requires`, and the host its factory got. */
async function withHost(requires: string[] = ["store"]): Promise<Host> {
  let given: Host | null = null;
  run = await inProcess((host) => {
    given = host;
    return { attach() {} };
  }, { requires, app: "dev.pocket-hub.store" });
  await until(() => given !== null, 3000, "the factory");
  return given!;
}

const rejects = async (promise: Promise<unknown>): Promise<ServiceError> => {
  try {
    await promise;
  } catch (error) {
    expect(error).toBeInstanceOf(ServiceError);
    return error as ServiceError;
  }
  throw new Error("the call resolved");
};

describe("host.store", () => {
  test("get, set, getText, getJson, setJson, delete, list, usage", async () => {
    const host = await withHost();
    expect(host.id).toBe("dev.pocket-hub.store");
    expect(await host.store.get("absent")).toBeNull();
    await host.store.set("bytes", new Uint8Array([1, 2, 3]));
    expect([...(await host.store.get("bytes"))!]).toEqual([1, 2, 3]);
    await host.store.set("text", "héllo");
    expect(await host.store.getText("text")).toBe("héllo");
    await host.store.setJson("json", { a: [1, "two"] });
    expect(await host.store.getJson<unknown>("json")).toEqual({ a: [1, "two"] });
    await host.store.set("empty", "");
    expect((await host.store.get("empty"))!.length).toBe(0);
    expect(await host.store.delete("text")).toBe(true);
    expect(await host.store.delete("text")).toBe(false);
    expect(await host.store.list()).toEqual({ keys: ["bytes", "empty", "json"], more: false });
    expect(await host.store.list({ limit: 1, after: "bytes" })).toEqual({ keys: ["empty"], more: true });
    expect(await host.store.usage()).toEqual({ bytes: (5 + 3) + (5 + 0) + (4 + 15), keys: 3, maxBytes: 16 * 1024 * 1024 });
    const ts = run!.hub.host!.requests.map((request) => request.t);
    expect(ts.slice(0, 3)).toEqual(["store.get", "store.set", "store.get"]);
  });

  test("a 3 MiB value goes out under the host's credit and comes back under the service's", async () => {
    const host = await withHost();
    const value = pattern(3 * 1024 * 1024, 7);
    await host.store.set("big", value);
    const back = await host.store.get("big");
    expect(Bun.hash.crc32(back!)).toBe(Bun.hash.crc32(value));
  });

  test("an iterable value needs its length, and must give exactly that many bytes", async () => {
    const host = await withHost();
    async function* chunks(n: number) {
      for (let i = 0; i < n; i++) yield pattern(100_000, i);
    }
    await host.store.set("streamed", chunks(5), { bytes: 500_000 });
    expect((await host.store.get("streamed"))!.length).toBe(500_000);
    expect((await rejects(host.store.set("x", chunks(1)))).code).toBe("invalid");
    // A short body is abandoned with RESET `length`; the host answers the call.
    const short = await rejects(host.store.set("short", chunks(1), { bytes: 200_000 }));
    expect(short.code).toBe("cancelled");
  });

  test("a key with a lone surrogate is refused before a frame leaves", async () => {
    const host = await withHost();
    const before = run!.hub.host!.requests.length;
    expect((await rejects(host.store.get("a\ud800"))).code).toBe("invalid");
    expect((await rejects(host.store.set("\udc00", "x"))).code).toBe("invalid");
    expect((await rejects(host.store.list({ prefix: "p\ud800" }))).code).toBe("invalid");
    expect(run!.hub.host!.requests.length).toBe(before);
    // A well-formed astral character is a key like any other.
    await host.store.set("😀", "ok");
    expect(await host.store.getText("😀")).toBe("ok");
  });

  test("a service whose manifest does not require store is refused locally, `not_required`", async () => {
    const host = await withHost([]);
    for (const call of [host.store.get("k"), host.store.set("k", "v"), host.store.delete("k"), host.store.list(), host.store.usage()]) {
      expect((await rejects(call)).code).toBe("not_required");
    }
    expect(run!.hub.host!.requests).toEqual([]);
  });

  test("an aborted call: before it is sent nothing leaves; during a body CANCEL ends it `cancelled`", async () => {
    const host = await withHost();
    const aborted = new AbortController();
    aborted.abort();
    expect((await rejects(host.store.get("k", { signal: aborted.signal }))).code).toBe("cancelled");
    expect(run!.hub.host!.requests).toEqual([]);

    let release!: () => void;
    const gate = new Promise<void>((resolve) => (release = resolve));
    async function* slow() {
      yield pattern(10);
      await gate;
      yield pattern(10);
    }
    const controller = new AbortController();
    const pending = host.store.set("slow", slow(), { bytes: 20, signal: controller.signal });
    await Bun.sleep(20);
    controller.abort();
    expect((await rejects(pending)).code).toBe("cancelled");
    release();
    expect(await host.store.get("slow")).toBeNull();
    expect(run!.hub.host!.cancels.length).toBe(1);
  });

  test("a CANCEL after the body's last byte: the write completes and the set resolves", async () => {
    const host = await withHost();
    run!.hub.host!.holdSets = true;
    const controller = new AbortController();
    const pending = host.store.set("kept", "value", { signal: controller.signal });
    await Bun.sleep(20);
    controller.abort();
    await Bun.sleep(20);
    run!.hub.host!.releaseSets();
    await pending;
    expect(await host.store.getText("kept")).toBe("value");
  });

  test("the host's error comes back as a ServiceError with its code", async () => {
    const host = await withHost();
    run!.hub.host!.maxBytes = 10;
    expect((await rejects(host.store.set("toolong", "1234567890"))).code).toBe("quota");
  });
});

describe("what this host does not provide", () => {
  test("host.net, host.native, channel.image, .text and .media exist and fail `not_required`", async () => {
    let given: Host | null = null;
    let channelSeen: import("../src/index.ts").AppletChannel | null = null;
    run = await inProcess((host) => {
      given = host;
      return { attach: (channel) => void (channelSeen = channel) };
    }, { requires: ["store"] });
    await until(() => given !== null, 3000, "the factory");
    const host = given! as Host;
    expect((await rejects(host.net.fetch("https://example.com/"))).code).toBe("not_required");
    expect((await rejects(host.net.websocket("wss://example.com/"))).code).toBe("not_required");
    expect(() => host.native.files).toThrow(ServiceError);
    await run.hub.offer();
    while (channelSeen === null) await Bun.sleep(2);
    const channel = channelSeen as import("../src/index.ts").AppletChannel;
    expect((await rejects(channel.image.render({ url: "https://example.com/a.png" }, { width: 10, height: 10 }))).code).toBe("not_required");
    expect((await rejects(channel.text.atlas({ px: 12, text: "字" }))).code).toBe("not_required");
    expect((await rejects(channel.media.open({ url: "https://example.com/a.mp4" }))).code).toBe("not_required");
    expect(run.hub.host!.requests).toEqual([]);
  });
});
