// Runs a service in this process against a MockHub.

import type { Service, ServiceFactory } from "../src/index.ts";
import { runService } from "../src/serve.ts";
import { MockHub, type MockHubOptions } from "./mock-hub.ts";

export interface InProcess {
  hub: MockHub;
  logs: string[];
  diags: string[];
  done: Promise<number>;
  /** Stops the service (`service.stop`), waits for it, removes the socket folder. Returns the exit code. */
  cleanup(): Promise<number>;
}

export async function inProcess(service: Service | ServiceFactory, options: MockHubOptions = {}): Promise<InProcess> {
  const hub = await MockHub.start(options);
  const logs: string[] = [];
  const diags: string[] = [];
  const factory: ServiceFactory = typeof service === "function" ? service : () => service;
  const run = runService(factory, { ...hub.runOptions(), log: (line: string) => logs.push(line), diag: (line: string) => diags.push(line) });
  if (options.refuse === undefined) await hub.waitControl();
  if (options.refuse === undefined && options.refuseHost === undefined) await hub.waitHost();
  return {
    hub,
    logs,
    diags,
    done: run.done,
    async cleanup() {
      hub.stop();
      const code = await Promise.race([run.done, Bun.sleep(3000).then(() => -1)]);
      await hub.shutdown();
      return code;
    },
  };
}

export const hex = (bytes: Uint8Array) => [...bytes].map((b) => b.toString(16).padStart(2, "0")).join(" ");
export const fromHex = (text: string) => new Uint8Array(text.trim().split(/\s+/).filter(Boolean).map((b) => Number.parseInt(b, 16)));
export const utf8 = (text: string) => [...new TextEncoder().encode(text)].map((b) => b.toString(16).padStart(2, "0")).join(" ");

/** Byte i of a pattern both ends compute (as the reference's daemon-sample.ts). */
export const pattern = (length: number, seed = 0, from = 0) => new Uint8Array(length).map((_, i) => ((from + i) * 31 + seed) & 255);

export async function until(test: () => boolean, timeoutMs = 3000, what = "the condition"): Promise<void> {
  const deadline = Date.now() + timeoutMs;
  while (!test()) {
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${what}`);
    await Bun.sleep(2);
  }
}
