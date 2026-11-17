// Starts a service as the host does (SERVICE-PROTOCOL.md §2): `bun
// --no-install run <sdk>/src/boot.ts <entry>` in the package folder with the
// three variables and the locale and nothing else, standard output and error
// read line by line.

import { join } from "node:path";
import type { Subprocess } from "bun";

/** The bootstrap the host runs. */
export const BOOT = join(import.meta.dir, "..", "src", "boot.ts");

/** `TZ`, `LANG` and `LC_*` of this environment: what the host passes on. */
export function locale(): Record<string, string> {
  const out: Record<string, string> = {};
  for (const [name, value] of Object.entries(process.env)) if (value !== undefined && (name === "TZ" || name === "LANG" || name.startsWith("LC_"))) out[name] = value;
  return out;
}

export interface Child {
  proc: Subprocess<"ignore", "pipe", "pipe">;
  out: string[];
  err: string[];
  /** The first output line (stdout or stderr) that matches, already printed or later. */
  line(pred: (line: string) => boolean, timeoutMs?: number): Promise<string>;
  exited: Promise<number>;
}

async function pump(stream: ReadableStream<Uint8Array>, into: string[], wake: () => void): Promise<void> {
  const decoder = new TextDecoder();
  let rest = "";
  for await (const chunk of stream) {
    rest += decoder.decode(chunk, { stream: true });
    const parts = rest.split("\n");
    rest = parts.pop() ?? "";
    into.push(...parts);
    wake();
  }
  if (rest) into.push(rest);
  wake();
}

export function spawnService(entry: string, env: Record<string, string>, cwd?: string): Child {
  const proc = Bun.spawn([process.execPath, "--no-install", "run", BOOT, entry], {
    cwd,
    env: { ...locale(), ...env },
    stdin: "ignore",
    stdout: "pipe",
    stderr: "pipe",
  });
  const out: string[] = [];
  const err: string[] = [];
  let waiters: (() => void)[] = [];
  const wake = () => {
    const list = waiters;
    waiters = [];
    for (const w of list) w();
  };
  void pump(proc.stdout, out, wake);
  void pump(proc.stderr, err, wake);
  return {
    proc,
    out,
    err,
    exited: proc.exited,
    async line(pred, timeoutMs = 5000) {
      const deadline = Date.now() + timeoutMs;
      for (;;) {
        const found = [...out, ...err].find(pred);
        if (found !== undefined) return found;
        if (Date.now() > deadline) throw new Error(`no such line in ${timeoutMs} ms; stdout: ${JSON.stringify(out)} stderr: ${JSON.stringify(err)}`);
        await Promise.race([new Promise<void>((resolve) => waiters.push(resolve)), Bun.sleep(50)]);
      }
    },
  };
}
