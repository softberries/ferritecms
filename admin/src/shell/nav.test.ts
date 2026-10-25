import { expect, test } from "vitest";
import type { ContentTypeDto } from "@/api/types";
import type { MeDto } from "@/auth/session";
import { navFor, navItemAt } from "./nav";

const type = (name: string, label: string, built_in = false): ContentTypeDto => ({
  name,
  label,
  built_in,
  hierarchical: false,
  has_archive: false,
});

// As `GET /types` answers: built-ins first, then custom types by name.
const TYPES = [
  type("post", "Posts", true),
  type("page", "Pages", true),
  type("media", "Media", true),
  type("recipe", "Recipes"),
  type("event", "Events"),
];

const me = (role: string, capabilities: string[]): MeDto => ({
  id: "00000000-0000-0000-0000-000000000001",
  login: role,
  email: `${role}@example.test`,
  display_name: role,
  role,
  must_reset: false,
  totp_enabled: false,
  capabilities,
  scopes: [],
});

// `can()` in crates/ferrite-core/src/actor.rs.
const AUTHOR = me("author", ["read_admin", "edit_own", "publish_own", "upload_media"]);

const shape = (sections: ReturnType<typeof navFor>) =>
  sections.map((s) => [s.id, s.items.map((i) => [i.label, i.to])]);

test("author_sees_posts_pages_and_custom_types", () => {
  expect(shape(navFor(AUTHOR, TYPES))).toEqual([
    ["overview", [["Dashboard", "/"]]],
    [
      "content",
      [
        ["Posts", "/content/post"],
        ["Pages", "/content/page"],
        // Custom types by label; never `media` (its own screen, M2b-2).
        ["Events", "/content/event"],
        ["Recipes", "/content/recipe"],
      ],
    ],
  ]);
  expect(navFor(AUTHOR, TYPES)[0]?.items[0]?.shortcut).toBe("g d");
  // 11 §6: the lists' chords; custom types have none.
  expect(navFor(AUTHOR, TYPES)[1]?.items.map((i) => i.shortcut)).toEqual([
    "g p",
    "g g",
    undefined,
    undefined,
  ]);
});

test("nothing_above_its_rights", () => {
  // Without `read_admin` the content APIs answer 403, so no content section.
  expect(shape(navFor(me("none", []), TYPES))).toEqual([["overview", [["Dashboard", "/"]]]]);
  // Nothing beyond Overview and Content exists in M2b-1, whatever the role holds.
  const admin = me("admin", [
    "read_admin",
    "edit_own",
    "publish_own",
    "edit_others",
    "publish_others",
    "upload_media",
    "manage_terms",
    "manage_menus",
    "moderate_comments",
    "manage_users",
    "manage_settings",
    "manage_themes",
    "manage_plugins",
    "import",
    "manage_redirects",
    "manage_system",
  ]);
  expect(navFor(admin, TYPES).map((s) => s.id)).toEqual(["overview", "content"]);
});

test("no_types_yet_means_no_content_section", () => {
  expect(navFor(AUTHOR, []).map((s) => s.id)).toEqual(["overview"]);
});

test("content_items_name_their_type_and_paths_find_their_item", () => {
  const sections = navFor(AUTHOR, TYPES);
  const content = sections.find((s) => s.id === "content")?.items ?? [];
  expect(content.map((i) => i.contentType)).toEqual(["post", "page", "event", "recipe"]);
  expect(navItemAt(sections, "/")?.id).toBe("dashboard");
  expect(navItemAt(sections, "/content/page")?.id).toBe("content-page");
  expect(navItemAt(sections, "/content/page/")?.id).toBe("content-page");
  expect(navItemAt(sections, "/content/page/new")).toBeUndefined();
});
