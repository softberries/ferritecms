// Unit tests (Vitest, the `scripts` project); not `*.test.ts`, which Playwright would
// collect as a spec.

import { describe, expect, it } from "vitest";
import { mediaFileName, OwnMedia } from "./own-media";

const HASH = "a".repeat(64);
const OTHER = "b".repeat(64);

describe("mediaFileName", () => {
  it("a_stored_file_url_gives_its_decoded_name", () => {
    expect(mediaFileName(`http://127.0.0.1:5/_media/${HASH}/caf%C3%A9.png`)).toBe("café.png");
    expect(mediaFileName(`/_media/${HASH}/a%2Cb%201.png`)).toBe("a,b 1.png");
  });

  it("other_urls_are_not_media_files", () => {
    expect(mediaFileName("http://127.0.0.1:5/api/v1/media")).toBeNull();
    expect(mediaFileName(`http://127.0.0.1:5/_media/${HASH}/`)).toBeNull();
    expect(mediaFileName(`http://127.0.0.1:5/_media/short/x.png`)).toBeNull();
    expect(mediaFileName(`http://127.0.0.1:5/_media/${HASH}/x.png?v=1`)).toBeNull();
  });
});

describe("OwnMedia", () => {
  it("a_404_of_an_own_upload_or_its_variant_is_not_excused", () => {
    const own = new OwnMedia();
    own.note(`/_media/${HASH}/harbour-x1.png`);
    // Variants share the original's name under their own hash.
    expect(own.isOwn(`http://127.0.0.1:5/_media/${OTHER}/harbour-x1.png`)).toBe(true);
    expect(own.isOwn(`http://127.0.0.1:5/_media/${HASH}/harbour-x1.png`)).toBe(true);
  });

  it("another_tests_file_is_not_own_and_clear_forgets", () => {
    const own = new OwnMedia();
    own.note(`/_media/${HASH}/mine.png`);
    expect(own.isOwn(`http://127.0.0.1:5/_media/${HASH}/theirs.png`)).toBe(false);
    expect(own.isOwn("http://127.0.0.1:5/api/v1/media/mine.png")).toBe(false);
    own.clear();
    expect(own.isOwn(`http://127.0.0.1:5/_media/${HASH}/mine.png`)).toBe(false);
  });

  it("names_compare_decoded", () => {
    const own = new OwnMedia();
    own.note(`/_media/${HASH}/caf%C3%A9.png`);
    expect(own.isOwn(`http://127.0.0.1:5/_media/${HASH}/caf%c3%a9.png`)).toBe(true);
  });
});
