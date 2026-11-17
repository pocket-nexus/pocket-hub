// The escape probe (tests/enclosure.rs): a service that reaches past the SDK
// the way hostile code would, through `Function("return import(…)")`,
// `bun:ffi` and raw sockets, and logs one line per attempt:
//
//   probe <name> allowed <detail>
//   probe <name> refused <detail>
//
// The test reads the lines from the service's log and asserts each. What to
// try is in `probe.json`, which the test writes into the package before the
// install (a service gets no environment of the test's).
import type { ServiceFactory } from "@pocketjs/service";

interface Plan {
  home: string;
  secret: string;
  outside: string;
  ownStore: string;
  otherPackage: string;
  otherStore: string;
  controlSocket: string;
  otherSocket: string;
  tcpPort: number;
  stranger: number;
  ancestor: number;
}

const load = (name: string) => Function(`return import('${name}')`)() as Promise<any>;

const service: ServiceFactory = async (host) => {
  const fs = await load("node:fs");
  const net = await load("node:net");
  const dns = await load("node:dns");
  const os = await load("node:os");
  const childProcess = await load("node:child_process");
  const ffi = await load("bun:ffi");
  const plan: Plan = JSON.parse(fs.readFileSync("probe.json", "utf8"));
  const say = (name: string, allowed: boolean, detail: unknown = "") => host.log(`probe ${name} ${allowed ? "allowed" : "refused"} ${String(detail).replace(/\s+/g, " ").slice(0, 160)}`);
  const attempt = async (name: string, run: () => unknown) => {
    try {
      say(name, true, await run());
    } catch (error: any) {
      say(name, false, error?.code ?? error?.message ?? error);
    }
  };
  const connect = (options: object) =>
    new Promise<string>((resolve, reject) => {
      const socket = net.connect(options);
      socket.on("connect", () => {
        socket.destroy();
        resolve("connected");
      });
      socket.on("error", reject);
    });

  // What works.
  await host.store.set("probe", "kept");
  await attempt("own-package", () => fs.readFileSync("pocket.service.json", "utf8").length);
  await attempt("store", async () => {
    const value = await host.store.getText("probe");
    if (value !== "kept") throw new Error(`read ${value}`);
    return value;
  });
  await attempt("intl", () => new Intl.DateTimeFormat("en-GB", { timeZone: "Asia/Shanghai", hour: "2-digit", hourCycle: "h23" }).format(new Date(0)));
  await attempt("crypto", async () => Buffer.from(await crypto.subtle.digest("SHA-256", new TextEncoder().encode("x"))).toString("hex").slice(0, 8));
  await attempt("timer", () => new Promise((resolve) => setTimeout(() => resolve("fired"), 10)));

  // Files.
  await attempt("read-home-file", () => fs.readFileSync(plan.secret, "utf8"));
  await attempt("list-home", () => fs.readdirSync(plan.home).join(","));
  await attempt("stat-outside", () => fs.statSync(plan.outside).size);
  await attempt("stat-home-file", () => fs.statSync(plan.secret).size);
  await attempt("read-own-store-file", () => fs.readFileSync(plan.ownStore).length);
  await attempt("read-other-package", () => fs.readFileSync(`${plan.otherPackage}/pocket.service.json`, "utf8").length);
  await attempt("read-other-store-file", () => fs.readFileSync(plan.otherStore).length);
  await attempt("write-own-package", () => fs.writeFileSync("written.txt", "x"));
  await attempt("write-home", () => fs.writeFileSync(`${plan.home}/written.txt`, "x"));
  await attempt("write-tmp", () => fs.writeFileSync("/private/tmp/ph-probe-written.txt", "x"));

  // Sockets and names.
  await attempt("connect-control-socket", () => connect({ path: plan.controlSocket }));
  await attempt("connect-other-socket", () => connect({ path: plan.otherSocket }));
  await attempt("connect-tcp", () => connect({ host: "127.0.0.1", port: plan.tcpPort }));
  await attempt("dns-lookup", () => dns.promises.lookup("example.com").then((found: any) => found.address));

  // Programs.
  await attempt("spawn", () => {
    const result = Bun.spawnSync(["/bin/echo", "escaped"]);
    if (!result.success) throw new Error(`exit ${result.exitCode}`);
    return result.stdout.toString().trim();
  });
  await attempt("exec-file", () => childProcess.execFileSync("/bin/echo", ["escaped"]).toString().trim());

  // Native calls (`bun:ffi`), an accepted leak: they run, and meet the profile.
  const { dlopen, FFIType, ptr } = ffi;
  let lib: any = null;
  await attempt("ffi-libsystem", () => {
    lib = dlopen("/usr/lib/libSystem.B.dylib", {
      getpid: { args: [], returns: FFIType.i32 },
      fork: { args: [], returns: FFIType.i32 },
      _exit: { args: [FFIType.i32], returns: FFIType.void },
      sysctl: { args: [FFIType.ptr, FFIType.u32, FFIType.ptr, FFIType.ptr, FFIType.ptr, FFIType.u64], returns: FFIType.i32 },
    });
    return `pid ${lib.symbols.getpid()}`;
  });
  if (lib !== null) {
    const sysctl = (mib: number[], buffer: Uint8Array | null) => {
      const names = new Int32Array(mib);
      const size = new BigUint64Array([BigInt(buffer?.length ?? 0)]);
      const rc = lib.symbols.sysctl(ptr(names), mib.length, buffer === null ? null : ptr(buffer), ptr(size), null, 0);
      if (rc !== 0) throw new Error(`sysctl ${mib.join(".")} refused`);
      return size[0]!;
    };
    // The command line of a process: KERN_PROCARGS2 (CTL_KERN 1, 49, pid).
    const commandLine = (pid: number) => {
      const buffer = new Uint8Array(1 << 20);
      const length = Number(sysctl([1, 49, pid], buffer));
      const text = new TextDecoder().decode(buffer.subarray(4, length));
      return text.split("\0").filter(Boolean)[0] ?? "";
    };
    await attempt("fork", () => {
      const pid = lib.symbols.fork();
      if (pid === 0) lib.symbols._exit(0);
      if (pid < 0) throw new Error("fork refused");
      return `child ${pid}`;
    });
    await attempt("list-processes", () => `${sysctl([1, 14, 0], null)} bytes`);
    await attempt("args-stranger", () => commandLine(plan.stranger));
    await attempt("args-launchd", () => commandLine(1));
    await attempt("args-ancestor", () => commandLine(plan.ancestor).split("/").pop());
  }
  // The machine's name, and the metadata of a folder above the package:
  // accepted leaks.
  await attempt("hostname", () => os.hostname().length);
  await attempt("stat-ancestor", () => (fs.statSync(plan.home).isDirectory() ? "a folder" : "?"));

  host.log("probe done");
  return {
    attach(channel) {
      channel.on("stored", async () => ({ value: await host.store.getText("probe") }));
    },
  };
};

export default service;
