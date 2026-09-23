import { existsSync, readFileSync } from "node:fs";

/** Parses a dotenv file: `KEY=value`, optional `export `, optional quotes, `#` comments. */
export function parseEnv(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    const body = line.startsWith("export ") ? line.slice(7) : line;
    const eq = body.indexOf("=");
    if (eq <= 0) continue;
    const key = body.slice(0, eq).trim();
    let value = body.slice(eq + 1).trim();
    if (value.length >= 2 && (value[0] === '"' || value[0] === "'") && value.endsWith(value[0])) {
      value = value.slice(1, -1);
    }
    out[key] = value;
  }
  return out;
}

/** Process environment overlaid with `path` if it exists (the file wins). */
export function loadEnv(path = ".env"): Record<string, string | undefined> {
  const env: Record<string, string | undefined> = { ...process.env };
  if (existsSync(path)) Object.assign(env, parseEnv(readFileSync(path, "utf8")));
  return env;
}
