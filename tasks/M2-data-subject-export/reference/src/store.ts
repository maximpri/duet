import { readFileSync } from "node:fs";
import { join } from "node:path";

/** Reads a JSON-lines store, skipping blank lines. */
export function readStore(dataDir: string, name: string): Record<string, unknown>[] {
  return readFileSync(join(dataDir, name), "utf8")
    .split("\n")
    .filter((line) => line.trim() !== "")
    .map((line) => JSON.parse(line));
}
