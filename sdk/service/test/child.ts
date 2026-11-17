// Starts a service as Hub does (SERVICE-PROTOCOL.md §2): `bun run <entry>` in
// the package folder with the four variables, standard output and error read
// line by line.

import type { Subprocess } from "bun";

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
  const proc = Bun.spawn([process.execPath, "run", entry], {
    cwd,
    env: { ...process.env, ...env },
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
