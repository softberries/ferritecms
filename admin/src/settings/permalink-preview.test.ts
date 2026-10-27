import { expect, test } from "vitest";
import {
  DEFAULT_PAGE_STRUCTURE,
  DEFAULT_POST_STRUCTURE,
  type PermalinkSample,
  appendToken,
  checkStructure,
  permalinkPreview,
} from "./permalink-preview";

const SAMPLE: PermalinkSample = {
  slug: "hello-world",
  date: "2026-03-07T23:30:00Z",
  category: "news",
  parents: ["about", "team"],
};

test("permalink_preview", () => {
  // Each token on its own.
  expect(permalinkPreview("/%slug%/", SAMPLE)).toBe("/hello-world/");
  expect(permalinkPreview("/%year%/%slug%/", SAMPLE)).toBe("/2026/hello-world/");
  // Two digits, in UTC (core's ruling R4).
  expect(permalinkPreview("/%month%/%slug%/", SAMPLE)).toBe("/03/hello-world/");
  expect(
    permalinkPreview("/%month%/%slug%/", { ...SAMPLE, date: "2026-12-31T23:59:00-05:00" }),
  ).toBe("/01/hello-world/");
  expect(permalinkPreview("/%category%/%slug%/", SAMPLE)).toBe("/news/hello-world/");
  // Without a category core writes `uncategorized`.
  expect(permalinkPreview("/%category%/%slug%/", { ...SAMPLE, category: null })).toBe(
    "/uncategorized/hello-world/",
  );
  expect(permalinkPreview("/%parent_path%/%slug%/", SAMPLE)).toBe("/about/team/hello-world/");
  // A top-level item: `%parent_path%` collapses to nothing.
  expect(permalinkPreview("/%parent_path%/%slug%/", { ...SAMPLE, parents: [] })).toBe(
    "/hello-world/",
  );
  // Literals stay; segments are percent-encoded like core does (unreserved kept).
  expect(permalinkPreview("/blog/%year%/%month%/%slug%/", SAMPLE)).toBe(
    "/blog/2026/03/hello-world/",
  );
  expect(permalinkPreview("/%slug%/", { ...SAMPLE, slug: "zażółć it's" })).toBe(
    "/za%C5%BC%C3%B3%C5%82%C4%87%20it%27s/",
  );
  expect(permalinkPreview("/%slug%/", { ...SAMPLE, slug: "a.b_c~d-e" })).toBe("/a.b_c~d-e/");
  // A structure core refuses has no preview.
  expect(permalinkPreview("/%year%/", SAMPLE)).toBeNull();
  expect(permalinkPreview("/%title%/", SAMPLE)).toBeNull();
});

test("page_structure_needs_parent_path", () => {
  expect(checkStructure(DEFAULT_PAGE_STRUCTURE, "page")).toBeNull();
  expect(checkStructure("/%slug%/", "page")).toEqual({ problem: "noParentPath" });
  expect(checkStructure("/%slug%/", "post")).toBeNull();
  expect(checkStructure("/pages/%parent_path%/%slug%/", "page")).toBeNull();
  expect(permalinkPreview("/%slug%/", SAMPLE, "page")).toBeNull();
});

test("structure_checks_follow_core", () => {
  expect(checkStructure(DEFAULT_POST_STRUCTURE, "post")).toBeNull();
  expect(checkStructure("%slug%/", "post")).toEqual({ problem: "slashes" });
  expect(checkStructure("/%slug%", "post")).toEqual({ problem: "slashes" });
  expect(checkStructure("/", "post")).toEqual({ problem: "noSlug" });
  expect(checkStructure("/%year%/%month%/", "post")).toEqual({ problem: "noSlug" });
  expect(checkStructure("/a//%slug%/", "post")).toEqual({ problem: "emptySegment" });
  expect(checkStructure("/%postname%/", "post")).toEqual({
    problem: "unknownToken",
    segment: "%postname%",
  });
  expect(checkStructure("/x-%slug%/", "post")).toEqual({
    problem: "unknownToken",
    segment: "x-%slug%",
  });
  expect(checkStructure("/../%slug%/", "post")).toEqual({ problem: "dotSegment" });
  expect(checkStructure("/Blog/%slug%/", "post")).toEqual({
    problem: "uppercase",
    segment: "Blog",
  });
  expect(checkStructure("/a b/%slug%/", "post")).toEqual({ problem: "badLiteral", segment: "a b" });
  expect(checkStructure("/admin/%slug%/", "post")).toEqual({
    problem: "reserved",
    segment: "admin",
  });
  // Only the first segment is checked against the reserved words.
  expect(checkStructure("/x/admin/%slug%/", "post")).toBeNull();
});

test("append_token_adds_a_segment", () => {
  expect(appendToken("", "%slug%")).toBe("/%slug%/");
  expect(appendToken("/blog/", "%year%")).toBe("/blog/%year%/");
  expect(appendToken("/blog", "%year%")).toBe("/blog/%year%/");
});
