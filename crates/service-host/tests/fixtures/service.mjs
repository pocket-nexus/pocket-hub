// A test service that speaks docs/SERVICE-PROTOCOL.md at the frame level with
// node:net only. What it does is set by `fixture.json` in its package folder,
// since Hub passes a service no environment of the test's.
import fs from "node:fs";
import net from "node:net";
import { spawn } from "node:child_process";

const cfg = JSON.parse(fs.readFileSync("fixture.json", "utf8"));
const env = process.env;
const socketPath = env.POCKET_SERVICE_SOCKET;
const HANDSHAKE = 0x04;
const EVENT = 0x12;

// The 12-byte header: type, flags, u16 reserved, u32 length, u32 id (LE).
function frame(type, id, payload) {
  const body = Buffer.from(payload);
  const header = Buffer.alloc(12);
  header.writeUInt8(type, 0);
  header.writeUInt8(0, 1);
  header.writeUInt16LE(0, 2);
  header.writeUInt32LE(body.length, 4);
  header.writeUInt32LE(id, 8);
  return Buffer.concat([header, body]);
}
const jsonFrame = (type, object) => frame(type, 0, JSON.stringify(object));

// Calls onFrame with each whole frame read from `socket`.
function readFrames(socket, onFrame) {
  let buffer = Buffer.alloc(0);
  socket.on("data", (chunk) => {
    buffer = Buffer.concat([buffer, chunk]);
    while (buffer.length >= 12) {
      const length = buffer.readUInt32LE(4);
      if (buffer.length < 12 + length) break;
      onFrame({ type: buffer[0], id: buffer.readUInt32LE(8), payload: buffer.subarray(12, 12 + length) });
      buffer = buffer.subarray(12 + length);
    }
  });
}

console.log(`fixture start app=${env.POCKET_SERVICE_ID} data=${env.POCKET_SERVICE_DATA}`);
console.log(`env ${Object.keys(env).filter((k) => k.startsWith("POCKET_SERVICE_")).sort().join(",")}`);
for (let i = 0; i < (cfg.logLines ?? 0); i++) console.log(`line ${i}`);
// The descriptors this process holds (the listing's own included).
if (cfg.listFds) console.log(`fds ${fs.readdirSync("/dev/fd").join(",")}`);
if (cfg.mode === "exitAtStart") {
  console.error("boom: exiting at start");
  process.exit(3);
}
if (cfg.spawnChild) {
  const child = spawn(process.execPath, ["-e", "setInterval(() => {}, 1000)"], { stdio: "ignore" });
  console.log(`child pid ${child.pid}`);
}
if (cfg.ignoreTerm) {
  process.on("SIGTERM", () => console.log("ignoring SIGTERM"));
  setInterval(() => {}, 1000); // alive with no socket left
}

if (cfg.mode === "silent") {
  console.log("never connecting");
  setInterval(() => {}, 1000);
  await new Promise(() => {});
}

const control = net.createConnection(socketPath);
control.on("connect", () => {
  control.write(jsonFrame(HANDSHAKE, {
    t: "service.hello",
    protocol: cfg.mode === "badProtocol" ? 1 : 2,
    token: cfg.mode === "badToken" ? "0".repeat(32) : env.POCKET_SERVICE_TOKEN,
    app: env.POCKET_SERVICE_ID,
  }));
});
control.on("error", () => {});
control.on("close", () => {
  console.log("control closed");
  if (!cfg.ignoreTerm) process.exit(0);
});
readFrames(control, (f) => {
  const message = JSON.parse(f.payload.toString());
  console.log(`control ${message.t}`);
  if (message.t === "service.refused") {
    console.log(`refused: ${message.message}`);
    process.exit(4);
  } else if (message.t === "channel.offer") {
    console.log(`offer device=${message.device} name=${message.deviceName} generation=${message.generation} limit=${message.payloadLimit}`);
    if (cfg.mode !== "noAttach") attach(cfg.attachTicket ?? message.ticket);
  } else if (message.t === "service.stop") {
    if (cfg.ignoreStop || cfg.ignoreTerm) {
      console.log("ignoring service.stop");
    } else {
      console.log("stopping");
      process.exit(0);
    }
  }
});

function attach(ticket) {
  const socket = net.createConnection(socketPath);
  socket.on("connect", () => {
    // The attach and a first channel frame in one write: Hub must hand the
    // second to the relay untouched.
    socket.write(Buffer.concat([
      jsonFrame(HANDSHAKE, { t: "channel.attach", ticket }),
      jsonFrame(EVENT, { t: "greeting" }),
    ]));
  });
  let first = true;
  socket.on("data", (chunk) => {
    if (first && chunk[0] === HANDSHAKE) {
      const message = JSON.parse(chunk.subarray(12, 12 + chunk.readUInt32LE(4)).toString());
      if (message.t === "channel.refused") {
        console.log(`channel refused: ${message.message}`);
        return;
      }
    }
    first = false;
    socket.write(chunk); // the echo
  });
  socket.on("error", () => {});
  socket.on("close", () => console.log("channel closed"));
  if (cfg.dieAfterAttachMs) setTimeout(() => { console.log("dying"); process.exit(5); }, cfg.dieAfterAttachMs);
  if (cfg.closeControlAfterAttachMs) setTimeout(() => { console.log("closing control"); control.destroy(); }, cfg.closeControlAfterAttachMs);
}
