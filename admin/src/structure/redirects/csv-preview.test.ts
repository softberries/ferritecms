import { expect, test } from "vitest";
import { PREVIEW_ROWS, previewCsv } from "./csv-preview";

test("header_detection", () => {
  // The server's header row: `source,target,code`, any case, spaces trimmed.
  expect(previewCsv(" Source ,TARGET,code\n/a,/b,301\n")).toEqual({
    rows: 1,
    header: true,
    sample: [["/a", "/b", "301"]],
  });
  expect(previewCsv("/a,/b,301\n/c,/d,302")).toEqual({
    rows: 2,
    header: false,
    sample: [
      ["/a", "/b", "301"],
      ["/c", "/d", "302"],
    ],
  });
  // Only the first record can be the header.
  expect(previewCsv("/a,/b,301\nsource,target,code\n").header).toBe(false);
  // A header alone is no row.
  expect(previewCsv("source,target,code\r\n")).toEqual({ rows: 0, header: true, sample: [] });
});

test("quoted_commas_and_doubled_quotes", () => {
  const text = '"/a,b","/say ""hi""",301\n"/multi\nline",/x,302\n';
  expect(previewCsv(text).sample).toEqual([
    ["/a,b", '/say "hi"', "301"],
    ["/multi\nline", "/x", "302"],
  ]);
  expect(previewCsv(text).rows).toBe(2);
});

test("crlf_and_blank_lines", () => {
  const text = "﻿source,target,code\r\n/a,/b,301\r\n\r\n/c,/d,410\r\n";
  expect(previewCsv(text)).toEqual({
    rows: 2,
    header: true,
    sample: [
      ["/a", "/b", "301"],
      ["/c", "/d", "410"],
    ],
  });
});

test("sample_holds_the_first_rows_only", () => {
  const text = Array.from({ length: 12 }, (_, i) => `/s${i},/t${i},301`).join("\n");
  const preview = previewCsv(text);
  expect(PREVIEW_ROWS).toBe(5);
  expect(preview.rows).toBe(12);
  expect(preview.sample).toHaveLength(PREVIEW_ROWS);
  expect(preview.sample[4]).toEqual(["/s4", "/t4", "301"]);
});

test("empty_file", () => {
  expect(previewCsv("")).toEqual({ rows: 0, header: false, sample: [] });
  expect(previewCsv("\r\n\n")).toEqual({ rows: 0, header: false, sample: [] });
  expect(previewCsv("﻿")).toEqual({ rows: 0, header: false, sample: [] });
});

test("unclosed_quote_keeps_the_rest_as_one_field", () => {
  // The server refuses it with the line; the preview only shows what would be sent.
  expect(previewCsv('/a,"/b\n/c,/d,301').sample).toEqual([["/a", "/b\n/c,/d,301"]]);
});
