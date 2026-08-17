// LIF-418: delimited-text parsing for the CSV/TSV attachment viewer.
//
// RFC 4180 rules: fields may be quoted with `"`, a quote inside a quoted field
// is doubled, and a quoted field may span newlines. Everything else is a
// split. No dependency for this; it is ~60 lines and a library would be a
// bigger maintenance surface than the parser.

export interface ParsedTable {
  /** First row, treated as the header. Empty array for an empty document. */
  headers: string[];
  /** Body rows, capped at the caller's `maxRows`. */
  rows: string[][];
  /** Total body rows in the document, before the cap. */
  totalRows: number;
  /** True when `rows` is shorter than `totalRows`. */
  truncated: boolean;
  /** Widest row seen, so the header can report a column count that does not
   *  lie when the file is ragged. */
  columnCount: number;
}

export type Delimiter = "," | "\t" | ";" | "|";

/** Pick a delimiter from the filename, falling back to whichever candidate
 *  appears most often in the first line of the document. */
export function detectDelimiter(filename: string, sample: string): Delimiter {
  const lower = filename.toLowerCase();
  if (lower.endsWith(".tsv") || lower.endsWith(".tab")) return "\t";
  if (lower.endsWith(".csv")) return ",";
  const firstLine = sample.split("\n", 1)[0] ?? "";
  const candidates: Delimiter[] = [",", "\t", ";", "|"];
  let best: Delimiter = ",";
  let bestCount = 0;
  for (const candidate of candidates) {
    const count = firstLine.split(candidate).length - 1;
    if (count > bestCount) {
      best = candidate;
      bestCount = count;
    }
  }
  return best;
}

/** Split the whole document into rows of fields. Stops scanning once
 *  `maxRows` body rows have been collected, but keeps counting rows so the
 *  header can say "200 of 40318". */
export function parseDelimited(
  text: string,
  options: { delimiter?: Delimiter; maxRows?: number } = {},
): ParsedTable {
  const delimiter = options.delimiter ?? ",";
  const maxRows = options.maxRows ?? 200;

  // Strip a UTF-8 BOM: Excel writes one and it would otherwise become part of
  // the first header cell.
  const src = text.charCodeAt(0) === 0xfeff ? text.slice(1) : text;

  const rows: string[][] = [];
  let totalRows = 0;
  let columnCount = 0;
  let field = "";
  let row: string[] = [];
  let inQuotes = false;
  let sawAnyChar = false;

  const endField = () => {
    row.push(field);
    field = "";
  };
  const endRow = () => {
    endField();
    // A trailing newline produces one empty row; drop it rather than render a
    // blank line at the bottom of every table.
    const isBlank = row.length === 1 && row[0] === "";
    if (!isBlank) {
      columnCount = Math.max(columnCount, row.length);
      if (rows.length === 0) rows.push(row);
      else {
        totalRows += 1;
        if (rows.length <= maxRows) rows.push(row);
      }
    }
    row = [];
  };

  for (let i = 0; i < src.length; i++) {
    const ch = src[i];
    sawAnyChar = true;
    if (inQuotes) {
      if (ch === '"') {
        if (src[i + 1] === '"') {
          field += '"';
          i += 1;
        } else {
          inQuotes = false;
        }
      } else {
        field += ch;
      }
      continue;
    }
    if (ch === '"' && field === "") {
      inQuotes = true;
      continue;
    }
    if (ch === delimiter) {
      endField();
      continue;
    }
    if (ch === "\r") continue;
    if (ch === "\n") {
      endRow();
      continue;
    }
    field += ch;
  }
  if (sawAnyChar && (field !== "" || row.length > 0)) endRow();

  const headers = rows.shift() ?? [];
  return {
    headers,
    rows,
    totalRows,
    truncated: totalRows > rows.length,
    columnCount: Math.max(columnCount, headers.length),
  };
}

/** Numeric-aware cell comparison for client-side column sorting. Numbers sort
 *  numerically, everything else case-insensitively, and blanks sink to the
 *  bottom regardless of direction (an empty cell is not "the smallest value",
 *  it is missing data). */
export function compareCells(a: string, b: string): number {
  const left = a?.trim() ?? "";
  const right = b?.trim() ?? "";
  if (left === right) return 0;
  if (left === "") return 1;
  if (right === "") return -1;
  const ln = Number(left);
  const rn = Number(right);
  if (Number.isFinite(ln) && Number.isFinite(rn)) return ln - rn;
  return left.localeCompare(right, undefined, { numeric: true, sensitivity: "base" });
}

/** Sort a copy of `rows` by one column. Blank-sinking means a descending sort
 *  is NOT a plain reversal, so direction is applied to the comparison rather
 *  than to the result array. */
export function sortRows(
  rows: string[][],
  column: number,
  direction: "asc" | "desc",
): string[][] {
  const sign = direction === "asc" ? 1 : -1;
  return [...rows].sort((a, b) => {
    const left = a[column] ?? "";
    const right = b[column] ?? "";
    if (left.trim() === "" && right.trim() !== "") return 1;
    if (right.trim() === "" && left.trim() !== "") return -1;
    return sign * compareCells(left, right);
  });
}
