// The text the page builds from the release: no message keeps a {name}, and
// a size reads as each language writes it.
import { expect, test } from "bun:test";
import { fill, megabytes } from "../src/format.ts";
import en from "../src/i18n/en.ts";
import zhCN from "../src/i18n/zh-CN.ts";

test("fill puts each value in, and a name without one is an error", () => {
  expect(fill("Version {version} · {size}", { version: "0.1.0", size: "7.4 MB" })).toBe("Version 0.1.0 · 7.4 MB");
  expect(() => fill("Version {version}", {})).toThrow("a message names {version}");
});

test("the download's line in each language", () => {
  expect(fill(en.release.facts, { version: "0.1.0", size: megabytes(7438342, "en") })).toBe("v0.1.0 · 7.4 MB");
  expect(fill(zhCN.release.facts, { version: "0.1.0", size: megabytes(7438342, "zh-CN") })).toBe("v0.1.0 · 7.4 MB");
});

test("a size has one decimal in every language", () => {
  expect(megabytes(12_000_000, "zh-CN")).toBe("12.0 MB");
});

test("every language names the same pieces in the same messages", () => {
  const names = (message: string) => [...message.matchAll(/\{(\w+)\}/g)].map((match) => match[1]).sort();
  const walk = (a: object, b: object, at: string) => {
    for (const [key, value] of Object.entries(a)) {
      const other = (b as Record<string, unknown>)[key];
      if (typeof value === "string") expect([at + key, names(other as string)]).toEqual([at + key, names(value)]);
      else walk(value, other as object, `${at}${key}.`);
    }
  };
  walk(en, zhCN, "");
});
