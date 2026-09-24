/**
 * Minimal CSV reader: quoted fields (with "" escapes), blank lines skipped, lines starting with
 * `#` skipped. Values are trimmed.
 */
export function parseRows(text: string): string[][] {
  const rows: string[][] = [];
  for (const raw of text.split(/\r?\n/)) {
    if (raw.trim() === "" || raw.startsWith("#")) continue;
    rows.push(splitLine(raw));
  }
  return rows;
}

function splitLine(line: string): string[] {
  const out: string[] = [];
  let field = "";
  let quoted = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (quoted) {
      if (ch === '"' && line[i + 1] === '"') {
        field += '"';
        i++;
      } else if (ch === '"') {
        quoted = false;
      } else {
        field += ch;
      }
    } else if (ch === '"') {
      quoted = true;
    } else if (ch === ",") {
      out.push(field.trim());
      field = "";
    } else {
      field += ch;
    }
  }
  out.push(field.trim());
  return out;
}

/** Rows as records keyed by the header row. */
export function parseRecords(text: string): Record<string, string>[] {
  const [header, ...rows] = parseRows(text);
  if (!header) return [];
  return rows.map((row) => Object.fromEntries(header.map((h, i) => [h, row[i] ?? ""])));
}
