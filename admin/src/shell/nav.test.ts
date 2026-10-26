import { expect, test } from "vitest";
import type { ContentTypeDto } from "@/api/types";
import type { MeDto } from "@/auth/session";
import { ShapesIcon } from "lucide-react";
import { navFor, navItemAt, type ScreenEntry } from "./nav";

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
        // Registered screens follow the types (Task 7: Media).
        ["Media", "/media"],
      ],
    ],
  ]);
  expect(navFor(AUTHOR, TYPES)[0]?.items[0]?.shortcut).toBe("g d");
  // 11 §6: the lists' chords; custom types have none; `g m` Media.
  expect(navFor(AUTHOR, TYPES)[1]?.items.map((i) => i.shortcut)).toEqual([
    "g p",
    "g g",
    undefined,
    undefined,
    "g m",
  ]);
});

test("nothing_above_its_rights", () => {
  // Without `read_admin` the content APIs answer 403, so no content section.
  expect(shape(navFor(me("none", []), TYPES))).toEqual([["overview", [["Dashboard", "/"]]]]);
  // Nothing beyond Overview and Content until a screen registers its entry, whatever
  // the role holds (so far only Media, in Content).
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
  expect(navFor(AUTHOR, [], undefined, []).map((s) => s.id)).toEqual(["overview"]);
});

test("media_is_registered", () => {
  // Task 7 registers Media: every `read_admin` role sees it, with `g m`.
  const media = navFor(me("contributor", ["read_admin", "edit_own"]), [])
    .flatMap((s) => s.items)
    .find((i) => i.id === "media");
  expect(media).toMatchObject({ label: "Media", to: "/media", shortcut: "g m" });
  expect(navItemAt(navFor(AUTHOR, TYPES), "/media")?.id).toBe("media");
  expect(
    navFor(me("none", []), TYPES)
      .flatMap((s) => s.items)
      .map((i) => i.id),
  ).toEqual(["dashboard"]);
});

test("content_items_name_their_type_and_paths_find_their_item", () => {
  const sections = navFor(AUTHOR, TYPES);
  const content = sections.find((s) => s.id === "content")?.items ?? [];
  expect(content.map((i) => i.contentType)).toEqual(["post", "page", "event", "recipe", undefined]);
  expect(navItemAt(sections, "/")?.id).toBe("dashboard");
  expect(navItemAt(sections, "/content/page")?.id).toBe("content-page");
  expect(navItemAt(sections, "/content/page/")?.id).toBe("content-page");
  expect(navItemAt(sections, "/content/page/new")).toBeUndefined();
});

// M2b-2 screens (F-A): each screen task registers its entry; the rules for who sees it
// and where it goes live in `navFor`. Every screen registered here, as the later tasks will.
const ENTRIES: ScreenEntry[] = [
  { screen: "redirects", to: "/redirects", label: "nav.redirects", icon: ShapesIcon },
  { screen: "media", to: "/media", label: "nav.media", icon: ShapesIcon },
  { screen: "menus", to: "/menus", label: "nav.menus", icon: ShapesIcon },
  { screen: "comments", to: "/comments", label: "nav.comments", icon: ShapesIcon },
  { screen: "taxonomies", to: "/taxonomies", label: "nav.taxonomies", icon: ShapesIcon },
  { screen: "types", to: "/types", label: "nav.contentTypes", icon: ShapesIcon },
];

const CONTRIBUTOR = me("contributor", ["read_admin", "edit_own"]);
const EDITOR = me("editor", [
  "read_admin",
  "edit_own",
  "publish_own",
  "upload_media",
  "edit_others",
  "publish_others",
  "manage_terms",
  "manage_menus",
  "moderate_comments",
]);
const ADMIN = me("admin", [
  ...EDITOR.capabilities,
  "manage_users",
  "manage_settings",
  "manage_themes",
  "manage_plugins",
  "import",
  "manage_redirects",
  "manage_system",
]);
const TWO_TYPES = TYPES.slice(0, 2);

test("screen_entries_by_role", () => {
  const rows = (who: MeDto) => shape(navFor(who, TWO_TYPES, undefined, ENTRIES)).slice(1);
  const content = [
    ["Posts", "/content/post"],
    ["Pages", "/content/page"],
  ];
  // Contributors and authors: Media (read-only for contributors, by the screen).
  expect(rows(CONTRIBUTOR)).toEqual([["content", [...content, ["Media", "/media"]]]]);
  expect(rows(AUTHOR)).toEqual([["content", [...content, ["Media", "/media"]]]]);
  // Editors: Comments too, and the structure they manage: terms (Taxonomies) and menus.
  expect(rows(EDITOR)).toEqual([
    ["content", [...content, ["Media", "/media"], ["Comments", "/comments"]]],
    [
      "structure",
      [
        ["Taxonomies", "/taxonomies"],
        ["Menus", "/menus"],
      ],
    ],
  ]);
  // Admins: everything, in the fixed order whatever the registration order.
  expect(rows(ADMIN)).toEqual([
    ["content", [...content, ["Media", "/media"], ["Comments", "/comments"]]],
    [
      "structure",
      [
        ["Content types", "/types"],
        ["Taxonomies", "/taxonomies"],
        ["Menus", "/menus"],
        ["Redirects", "/redirects"],
      ],
    ],
  ]);
  // Without `read_admin` nothing past the dashboard, even registered.
  expect(rows(me("none", []))).toEqual([]);
  // The chords of 11 §6.
  const items = navFor(ADMIN, TWO_TYPES, undefined, ENTRIES).flatMap((s) => s.items);
  expect(items.find((i) => i.id === "media")?.shortcut).toBe("g m");
  expect(items.find((i) => i.id === "comments")?.shortcut).toBe("g c");
  expect(navItemAt(navFor(ADMIN, TWO_TYPES, undefined, ENTRIES), "/menus")?.id).toBe("menus");
});

test("screen_entries_need_no_content_types", () => {
  // Media shows even before the types have loaded (or when none is listed).
  expect(shape(navFor(AUTHOR, [], undefined, ENTRIES))).toEqual([
    ["overview", [["Dashboard", "/"]]],
    ["content", [["Media", "/media"]]],
  ]);
});
