// The editor's schema lists and embed URL rules against the shared fixture the core
// renderer's tests read too (`crates/ferrite-core/tests/fixtures/editor-schema.json`):
// `blocks::is_editor_safe` and `blocks::embed_id` are the Rust twins, so the importer
// only writes bodies this editor opens editable, and every embed the editor makes plays
// on the site.

import { describe, expect, test } from "vitest";
import { EMBED_PROVIDERS, canonicalEmbedUrl, embedFromUrl, parseEmbed } from "./embed";
import { HEADING_LEVELS, SUPPORTED_MARKS, SUPPORTED_NODES } from "./schema";

interface NodeFs {
  readFileSync(path: string, encoding: "utf8"): string;
}

// A specifier TypeScript does not resolve, so the app's types stay free of Node's.
const NODE_FS = "node:fs";
const fs = (await import(/* @vite-ignore */ NODE_FS)) as NodeFs;

interface SharedSchema {
  nodes: string[];
  marks: string[];
  heading_levels: number[];
  embed_providers: string[];
  embed_cases: { url: string; provider: string | null; id: string | null }[];
}

// Relative to the admin folder, where Vitest runs.
const shared = JSON.parse(
  fs.readFileSync("../crates/ferrite-core/tests/fixtures/editor-schema.json", "utf8"),
) as SharedSchema;

const sorted = (xs: readonly (string | number)[]) => [...xs].sort();

describe("shared editor schema", () => {
  test("lists_equal_the_rust_fixture", () => {
    expect(sorted(SUPPORTED_NODES)).toEqual(sorted(shared.nodes));
    expect(sorted(SUPPORTED_MARKS)).toEqual(sorted(shared.marks));
    expect([...HEADING_LEVELS]).toEqual(shared.heading_levels);
    expect(sorted(EMBED_PROVIDERS)).toEqual(sorted(shared.embed_providers));
  });

  test("embed_urls_match_the_rust_cases", () => {
    expect(shared.embed_cases.length).toBeGreaterThan(10);
    for (const c of shared.embed_cases) {
      const want = c.provider && c.id ? { provider: c.provider, id: c.id } : null;
      expect(parseEmbed(c.url), c.url).toEqual(want);
    }
  });
});

describe("embed urls", () => {
  test("canonical_urls_parse_back", () => {
    expect(canonicalEmbedUrl("youtube", "dQw4w9WgXcQ")).toBe(
      "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
    );
    expect(canonicalEmbedUrl("vimeo", "76979871")).toBe("https://vimeo.com/76979871");
    expect(embedFromUrl("https://youtu.be/dQw4w9WgXcQ?t=3")).toEqual({
      provider: "youtube",
      url: "https://www.youtube.com/watch?v=dQw4w9WgXcQ",
    });
    expect(embedFromUrl(" https://player.vimeo.com/video/76979871 ")).toEqual({
      provider: "vimeo",
      url: "https://vimeo.com/76979871",
    });
    for (const p of EMBED_PROVIDERS) {
      const id = p === "youtube" ? "dQw4w9WgXcQ" : "76979871";
      expect(parseEmbed(canonicalEmbedUrl(p, id))).toEqual({ provider: p, id });
    }
    expect(embedFromUrl("https://example.com/video")).toBeNull();
  });
});
