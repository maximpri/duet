/**
 * Minimal delimited-text reader: quoted fields (with "" escapes), a configurable
 * delimiter, blank lines skipped. Lines starting with `comment` are skipped when
 * `comment` is given.
 */
export interface CsvOptions {
  delimiter?: string;
  comment?: string;
}

export function parseRows(text: string, options: CsvOptions = {}): string[][] {
  const delimiter = options.delimiter ?? ",";
  const rows: string[][] = [];
  for (const raw of text.split(/\r?\n/)) {
    if (raw.trim() === "") continue;
    if (options.comment && raw.startsWith(options.comment)) continue;
    rows.push(splitLine(raw, delimiter));
  }
  return rows;
}

function splitLine(line: string, delimiter: string): string[] {
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
    } else if (ch === '"' && field === "") {
      quoted = true;
    } else if (line.startsWith(delimiter, i)) {
      out.push(field);
      field = "";
      i += delimiter.length - 1;
    } else {
      field += ch;
    }
  }
  out.push(field);
  return out;
}

/** Rows as objects keyed by the header row. */
export function parseRecords(text: string, options: CsvOptions = {}): Record<string, string>[] {
  const [header, ...rows] = parseRows(text, options);
  if (!header) return [];
  const keys = header.map((h) => h.trim());
  return rows.map((row) => {
    const rec: Record<string, string> = {};
    keys.forEach((k, i) => (rec[k] = (row[i] ?? "").trim()));
    return rec;
  });
}
