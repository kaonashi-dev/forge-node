export type CsvTable = { rows: string[][]; width: number; truncated: boolean };

// A preview is a glance, not a spreadsheet: every cell is a DOM node.
export const MAX_CSV_ROWS = 1_000;
export const MAX_CSV_COLUMNS = 100;

/** A tab for `.tsv`, a comma for everything else. */
export function csvDelimiter(path: string): "," | "\t" {
  return path.toLowerCase().endsWith(".tsv") ? "\t" : ",";
}

/**
 * RFC 4180 fields: `""` inside quotes is one quote, and a quoted field may
 * hold the delimiter or a newline. Stops at the row and column caps instead
 * of building rows it would not show.
 */
export function parseCsv(
  text: string,
  delimiter: string,
  maxRows = MAX_CSV_ROWS,
  maxColumns = MAX_CSV_COLUMNS,
): CsvTable {
  const rows: string[][] = [];
  let row: string[] = [];
  let field = "";
  let quoted = false;
  let truncated = false;
  let width = 0;
  let at = text.charCodeAt(0) === 0xfeff ? 1 : 0;

  const endField = () => {
    if (row.length < maxColumns) row.push(field);
    else truncated = true;
    field = "";
  };
  const endRow = () => {
    endField();
    width = Math.max(width, row.length);
    rows.push(row);
    row = [];
  };

  while (at < text.length) {
    const char = text[at];
    if (quoted) {
      if (char === '"' && text[at + 1] === '"') {
        field += '"';
        at += 2;
        continue;
      }
      if (char === '"') quoted = false;
      else field += char;
      at += 1;
      continue;
    }
    if (char === '"' && field === "") {
      quoted = true;
    } else if (char === delimiter) {
      endField();
    } else if (char === "\n" || char === "\r") {
      endRow();
      if (char === "\r" && text[at + 1] === "\n") at += 1;
      if (rows.length === maxRows) {
        truncated = at + 1 < text.length;
        return { rows, width, truncated };
      }
    } else {
      field += char;
    }
    at += 1;
  }
  if (field !== "" || row.length > 0 || quoted) endRow();
  return { rows, width, truncated };
}
