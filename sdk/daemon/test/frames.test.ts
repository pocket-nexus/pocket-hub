// Byte-exact frames: one example of each frame type the daemon sends,
// against bytes written out by hand from DAEMON-PROTOCOL.md §3–§5 and the
// layout of `crates/pocket-link/src/frame.rs` (`put_header`: u8 type, u8
// flags, u16 0, u32 length, u32 id, all little-endian; `encode_credit`,
// `encode_reset`, `encode_cancel`, `encode_data`), and the decoder's checks
// of `check_frame_in(Dialect::Local, …)`.

import { afterEach, describe, expect, test } from "bun:test";
import {
  FLAG_BODY,
  FLAG_ERROR,
  FRAME,
  FrameDecoder,
  FrameError,
  encodeCancel,
  encodeCredit,
  encodeData,
  encodeHandshake,
  encodeJson,
  encodeReset,
} from "../src/frame.ts";
import { LinkError } from "../src/index.ts";
import { fromHex, hex, inProcess, utf8, type InProcess } from "./helpers.ts";
import { T } from "./mock-hub.ts";

describe("encoder: one frame of each type, by hand", () => {
  test("HANDSHAKE daemon.hello", () => {
    const json = '{"t":"daemon.hello","protocol":1,"token":"00112233445566778899aabbccddeeff","app":"dev.x"}';
    expect(json.length).toBe(90);
    // type 0x04, flags 0, reserved 0, length 90 = 0x5a, id 0
    const expected = `04 00 00 00 5a 00 00 00 00 00 00 00 ${utf8(json)}`;
    expect(hex(encodeHandshake({ t: "daemon.hello", protocol: 1, token: "00112233445566778899aabbccddeeff", app: "dev.x" }))).toBe(expected);
  });

  test("REQUEST with BODY alone (local dialect) on the first Hub id", () => {
    const json = '{"t":"sink"}';
    // type 0x10, flags 0x04 BODY, length 12, id 0x80000001 little-endian = 01 00 00 80
    expect(hex(encodeJson(FRAME.request, 0x8000_0001, { t: "sink" }, 16_384, FLAG_BODY))).toBe(`10 04 00 00 0c 00 00 00 01 00 00 80 ${utf8(json)}`);
  });

  test("REPLY and REPLY ERROR", () => {
    expect(hex(encodeJson(FRAME.reply, 5, { ok: true }, 16_384))).toBe(`11 00 00 00 0b 00 00 00 05 00 00 00 ${utf8('{"ok":true}')}`);
    const error = '{"code":"busy","message":"x"}';
    expect(error.length).toBe(29);
    expect(hex(encodeJson(FRAME.reply, 5, { code: "busy", message: "x" }, 16_384, FLAG_ERROR))).toBe(`11 01 00 00 1d 00 00 00 05 00 00 00 ${utf8(error)}`);
  });

  test("EVENT: standalone, progress, stream-opening, channel.close", () => {
    expect(hex(encodeJson(FRAME.event, 0, { t: "tick", n: 1 }, 16_384))).toBe(`12 00 00 00 12 00 00 00 00 00 00 00 ${utf8('{"t":"tick","n":1}')}`);
    expect(hex(encodeJson(FRAME.event, 0x65, { t: "blob.head" }, 16_384))).toBe(`12 00 00 00 11 00 00 00 65 00 00 00 ${utf8('{"t":"blob.head"}')}`);
    expect(hex(encodeJson(FRAME.event, 0x8000_0002, { t: "up" }, 16_384))).toBe(`12 00 00 00 0a 00 00 00 02 00 00 80 ${utf8('{"t":"up"}')}`);
    const close = '{"t":"channel.close","reason":"daemon"}';
    expect(hex(encodeJson(FRAME.event, 7, { t: "channel.close", reason: "daemon" }, 16_384))).toBe(`12 00 00 00 ${close.length.toString(16)} 00 00 00 07 00 00 00 ${utf8(close)}`);
  });

  test("CANCEL, DATA, DATA END, CREDIT, RESET", () => {
    expect(hex(encodeCancel(0x8000_0002))).toBe("13 00 00 00 00 00 00 00 02 00 00 80");
    expect(hex(encodeData(0x8000_0003, new Uint8Array([1, 2, 3]), false, 16_384))).toBe("20 00 00 00 03 00 00 00 03 00 00 80 01 02 03");
    expect(hex(encodeData(0x65, new Uint8Array(0), true, 16_384))).toBe("20 01 00 00 00 00 00 00 65 00 00 00");
    // 256 units on channel 7: u32 256 = 00 01 00 00
    expect(hex(encodeCredit(7, 256))).toBe("21 00 00 00 04 00 00 00 07 00 00 00 00 01 00 00");
    // RESET refused (2): u16 code, u16 0
    expect(hex(encodeReset(9, "refused"))).toBe("22 00 00 00 04 00 00 00 09 00 00 00 02 00 00 00");
    expect(hex(encodeReset(0x8000_0004, "cancelled"))).toBe("22 00 00 00 04 00 00 00 04 00 00 80 01 00 00 00");
  });

  test("the encoder refuses what Hub's decoder would refuse", () => {
    expect(() => encodeCredit(7, 0)).toThrow(FrameError);
    expect(() => encodeData(5, new Uint8Array(0), false, 16_384)).toThrow(FrameError);
    expect(() => encodeData(5, new Uint8Array(1025), false, 1024)).toThrow(FrameError);
    expect(() => encodeCancel(0)).toThrow(FrameError);
    expect(() => encodeCancel(0x8000_0000)).toThrow(FrameError);
    expect(() => encodeJson(FRAME.event, 0x8000_0000, { t: "x" }, 16_384)).toThrow(FrameError);
    expect(() => encodeHandshake({ t: "x".repeat(1100) })).toThrow(FrameError);
    // APP (0x02) is no flag of the local dialect.
    expect(() => encodeJson(FRAME.event, 0, { t: "x" }, 16_384, 0x02)).toThrow(FrameError);
  });
});

describe("decoder: the local dialect's checks", () => {
  test("BODY alone is valid; APP on EVENT is not", () => {
    const decoder = new FrameDecoder(16_384);
    const frames = decoder.push(fromHex(`10 04 00 00 0c 00 00 00 05 00 00 00 ${utf8('{"t":"sink"}')}`));
    expect(frames.map((f) => [f.type, f.flags, f.id])).toEqual([[0x10, 0x04, 5]]);
    const app = new FrameDecoder(16_384);
    expect(() => app.push(fromHex("12 02 00 00 06 00 00 00 00 00 00 00 03 00 00 00 7b 7d"))).toThrow(FrameError);
  });
  test("a frame split across reads, an unknown type skipped, a HANDSHAKE at any time", () => {
    const decoder = new FrameDecoder(16_384);
    const bytes = fromHex(`7f 00 00 00 02 00 00 00 00 00 00 00 aa bb  04 00 00 00 02 00 00 00 00 00 00 00 7b 7d  21 00 00 00 04 00 00 00 07 00 00 00 30 00 00 00`);
    const out = [...decoder.push(bytes.subarray(0, 5)), ...decoder.push(bytes.subarray(5, 29)), ...decoder.push(bytes.subarray(29))];
    expect(out.map((f) => f.type)).toEqual([0x04, 0x21]);
  });
  test("over the limit, non-zero reserved, CREDIT 0, RESET padding: refused", () => {
    expect(() => new FrameDecoder(1024).push(fromHex("20 00 00 00 01 04 00 00 05 00 00 00"))).toThrow(FrameError);
    expect(() => new FrameDecoder(1024).push(fromHex("11 00 01 00 00 00 00 00 05 00 00 00"))).toThrow(FrameError);
    expect(() => new FrameDecoder(1024).push(fromHex("21 00 00 00 04 00 00 00 05 00 00 00 00 00 00 00"))).toThrow(FrameError);
    expect(() => new FrameDecoder(1024).push(fromHex("22 00 00 00 04 00 00 00 05 00 00 00 02 00 01 00"))).toThrow(FrameError);
  });
});

describe("on the socket: the bytes the daemon writes", () => {
  let run: InProcess | null = null;
  afterEach(async () => {
    if (run) {
      expect(run.hub.violations).toEqual([]);
      await run.cleanup();
    }
    run = null;
  });

  test("hello, attach, the first grant, a reply, an error, an event, a request", async () => {
    run = await inProcess({
      attach(channel) {
        channel.on("echo", (args) => ({ echo: args.text }));
        channel.on("fail", () => {
          throw new LinkError("nope", "no");
        });
        channel.onEvent("ping", () => void channel.emit("pong", {}));
        channel.on("ask", () => {
          void channel.request("q", { n: 1 });
          return {};
        });
      },
    }, { token: "00112233445566778899aabbccddeeff", app: "dev.x" });
    const hello = `{"t":"daemon.hello","protocol":1,"token":"00112233445566778899aabbccddeeff","app":"dev.x"}`;
    expect(run.hub.hellos).toEqual([JSON.parse(hello)]);
    const ch = await run.hub.offer({}, { channel: 7 });
    const credit = await ch.waitFor((f) => f.type === T.credit);
    expect(hex(credit.raw)).toBe("21 00 00 00 04 00 00 00 07 00 00 00 00 01 00 00");
    const attach = ch.frames[0]!;
    expect(attach.type).toBe(T.credit); // the handshake frame itself went before `channel.ready`

    ch.request("echo", { text: "hi" }, { id: 0x65 });
    expect(hex((await ch.reply(0x65)).raw)).toBe(`11 00 00 00 0d 00 00 00 65 00 00 00 ${utf8('{"echo":"hi"}')}`);
    ch.request("fail", {}, { id: 0x66 });
    const fail = '{"code":"nope","message":"no"}';
    expect(hex((await ch.reply(0x66)).raw)).toBe(`11 01 00 00 ${fail.length.toString(16)} 00 00 00 66 00 00 00 ${utf8(fail)}`);
    ch.grantUnits(48);
    ch.event("ping");
    const pong = await ch.waitFor((f) => f.type === T.event && f.json?.t === "pong");
    expect(hex(pong.raw)).toBe(`12 00 00 00 0c 00 00 00 00 00 00 00 ${utf8('{"t":"pong"}')}`);
    ch.request("ask", {}, { id: 0x67 });
    const q = await ch.waitFor((f) => f.type === T.request);
    expect(hex(q.raw)).toBe(`10 00 00 00 0f 00 00 00 01 00 00 80 ${utf8('{"t":"q","n":1}')}`);
    // the regrant of the one unit `ping` cost
    const regrant = await ch.waitFor((f) => f.type === T.credit && f.id === 7 && f.value === 1);
    expect(hex(regrant.raw)).toBe("21 00 00 00 04 00 00 00 07 00 00 00 01 00 00 00");
  });
});
