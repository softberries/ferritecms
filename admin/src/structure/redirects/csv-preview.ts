// What a redirect CSV import will send, read the way the server reads it
// (`ferrite-http/src/api/csv.rs`, RFC 4180): `,` between fields, CRLF or LF between
// records, `"` quotes a field that may then hold commas, line breaks and `""` (one
// quote); a leading byte-order mark and empty lines are skipped; a first record
// `source,target,code` (any case, spaces trimmed) is the header. The preview never
// refuses a file: what the server would refuse (a stray quote, an unclosed quoted field)
// is shown as read, and the server's `422` names the line.

/** Rows the preview shows. */
export const PREVIEW_ROWS = 5;

const HEADER = ["source", "target", "code"] as const;

export interface CsvPreview {
  /** Data rows (the header not counted). */
  rows: number;
  /** The first record is the header row. */
  header: boolean;
  /** The first `PREVIEW_ROWS` data rows. */
  sample: string[][];
}

/** Every record of `text` (empty lines skipped). */
function parse(text: string): string[][] {
  const input = text.startsWith("﻿") ? text.slice(1) : text;
  const records: string[][] = [];
  let fields: string[] = [];
  let field = "";
  let quoted = false;
  let inQuotes = false;
  const endRecord = () => {
    fields.push(field);
    // An empty line is one empty unquoted field.
    if (!(fields.length === 1 && fields[0] === "" && !quoted)) records.push(fields);
    fields = [];
    field = "";
    quoted = false;
  };
  for (let i = 0; i < input.length; i++) {
    const c = input[i];
    if (inQuotes) {
      if (c === '"') {
        if (input[i + 1] === '"') {
          field += '"';
          i++;
        } else {
          inQuotes = false;
        }
      } else {
        field += c;
      }
    } else if (c === ",") {
      fields.push(field);
      field = "";
      quoted = false;
    } else if (c === "\n" || (c === "\r" && input[i + 1] === "\n")) {
      if (c === "\r") i++;
      endRecord();
    } else if (c === '"' && field === "" && !quoted) {
      quoted = true;
      inQuotes = true;
    } else {
      field += c;
    }
  }
  if (field !== "" || fields.length > 0 || quoted) endRecord();
  return records;
}

const isHeader = (record: string[] | undefined) =>
  record !== undefined &&
  record.length === HEADER.length &&
  record.every((f, i) => f.trim().toLowerCase() === HEADER[i]);

/** What importing `text` sends: the row count, whether it has a header, the first rows. */
export function previewCsv(text: string): CsvPreview {
  const records = parse(text);
  const header = isHeader(records[0]);
  const data = header ? records.slice(1) : records;
  return { rows: data.length, header, sample: data.slice(0, PREVIEW_ROWS) };
}
