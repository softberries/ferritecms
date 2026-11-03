import { expect, test } from "vitest";
import type { ContentTypeDto } from "@/api/types";
import type { MeDto } from "@/auth/session";
import { ShapesIcon } from "lucide-react";
import { navFor, navItemAt, withBadges, type ScreenEntry } from "./nav";

const type = (name: string, label: string, built_in = false): ContentTypeDto => ({
  name,
  label,
  quality_gate: "off",
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
  // Nothing beyond what the registered screens give, whatever the role holds (so far
  // Media in Content, Content types, Taxonomies and Menus in Structure).
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
  expect(navFor(admin, TYPES).map((s) => s.id)).toEqual([
    "overview",
    "content",
    "structure",
    "admin",
  ]);
});

test("users_is_registered", () => {
  // M2b-3 Task 7 registers Users under Administration: only `manage_users` sees it.
  const admin = navFor(me("admin", ["read_admin", "manage_users"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.label).toBe("Administration");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["users", "Users", "/users", "g u"],
  ]);
  expect(navItemAt(admin, "/users")?.id).toBe("users");
  expect(navItemAt(admin, "/users/")?.id).toBe("users");
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(navFor(editor, TYPES).find((s) => s.id === "admin")).toBeUndefined();
});

test("appearance_is_registered", () => {
  // M2b-3 Task 9 registers Appearance (the Themes screen) under Administration: only
  // `manage_themes` sees it, with `g a`.
  const admin = navFor(me("admin", ["read_admin", "manage_themes"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["appearance", "Appearance", "/appearance/themes", "g a"],
  ]);
  expect(navItemAt(admin, "/appearance/themes")?.id).toBe("appearance");
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(navFor(editor, TYPES).find((s) => s.id === "admin")).toBeUndefined();
});

test("plugins_is_registered", () => {
  // M2b-3 Task 10 registers Plugins under Administration: only `manage_plugins` sees it,
  // with `g l` (`g p` is Posts).
  const admin = navFor(me("admin", ["read_admin", "manage_plugins"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["plugins", "Plugins", "/plugins", "g l"],
  ]);
  expect(navItemAt(admin, "/plugins")?.id).toBe("plugins");
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(navFor(editor, TYPES).find((s) => s.id === "admin")).toBeUndefined();
});

test("nav shows Import only with the import capability", () => {
  // M3-2 P32 registers Import under Administration, between Plugins and Settings: only
  // `import` sees it, with `g i`.
  const admin = navFor(me("admin", ["read_admin", "import"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["import", "Import", "/import", "g i"],
  ]);
  expect(navItemAt(admin, "/import")?.id).toBe("import");
  const others = me("admin", ["read_admin", "manage_plugins", "manage_settings"]);
  expect(
    navFor(others, TYPES)
      .flatMap((s) => s.items)
      .some((i) => i.id === "import"),
  ).toBe(false);
  // Never without `read_admin`, whatever else is held.
  expect(
    navFor(me("none", ["import"]), TYPES)
      .flatMap((s) => s.items)
      .map((i) => i.id),
  ).toEqual(["dashboard"]);
});

test("system_is_registered", () => {
  // M2b-3 Task 11 registers System under Administration: only `manage_system` sees it,
  // with `g y`.
  const admin = navFor(me("admin", ["read_admin", "manage_system"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["system", "System", "/system", "g y"],
  ]);
  expect(navItemAt(admin, "/system")?.id).toBe("system");
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(navFor(editor, TYPES).find((s) => s.id === "admin")).toBeUndefined();
});

test("settings_is_registered", () => {
  // M2b-3 Task 8 registers Settings under Administration: only `manage_settings` sees it,
  // and it is current on every group's page.
  const admin = navFor(me("admin", ["read_admin", "manage_settings"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["settings", "Settings", "/settings", "g s"],
  ]);
  expect(navItemAt(admin, "/settings")?.id).toBe("settings");
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(navFor(editor, TYPES).find((s) => s.id === "admin")).toBeUndefined();
});

test("content_types_is_registered", () => {
  // Task 9 registers Content types under Structure: only `manage_settings` sees it.
  const ADMIN_CAPS = me("admin", ["read_admin", "manage_settings"]);
  const structure = navFor(ADMIN_CAPS, TYPES).find((s) => s.id === "structure");
  expect(structure?.items.map((i) => [i.id, i.label, i.to])).toEqual([
    ["types", "Content types", "/structure/types"],
    ["taxonomies", "Taxonomies", "/structure/taxonomies"],
  ]);
  expect(structure?.items[0]?.shortcut).toBeUndefined();
  expect(navItemAt(navFor(ADMIN_CAPS, TYPES), "/structure/types")?.id).toBe("types");
  // Editors manage terms and menus, not types.
  const editor = me("editor", ["read_admin", "edit_others", "manage_terms", "manage_menus"]);
  expect(
    navFor(editor, TYPES)
      .find((s) => s.id === "structure")
      ?.items.map((i) => i.id),
  ).toEqual(["taxonomies", "menus"]);
});

test("taxonomies_is_registered", () => {
  // Task 10 registers Taxonomies under Structure: `manage_settings` (the taxonomies
  // themselves) or `manage_terms` (their terms) sees it.
  const ids = (who: MeDto) =>
    navFor(who, TYPES)
      .find((s) => s.id === "structure")
      ?.items.map((i) => i.id) ?? [];
  expect(ids(me("admin", ["read_admin", "manage_settings"]))).toContain("taxonomies");
  expect(ids(me("editor", ["read_admin", "manage_terms"]))).toEqual(["taxonomies"]);
  expect(ids(AUTHOR)).toEqual([]);
  const sections = navFor(me("editor", ["read_admin", "manage_terms"]), TYPES);
  expect(navItemAt(sections, "/structure/taxonomies")?.id).toBe("taxonomies");
  expect(navItemAt(sections, "/structure/taxonomies/")?.id).toBe("taxonomies");
});

test("menus_is_registered", () => {
  // Task 11 registers Menus under Structure, after Taxonomies: only `manage_menus` sees it.
  const ids = (who: MeDto) =>
    navFor(who, TYPES)
      .find((s) => s.id === "structure")
      ?.items.map((i) => i.id) ?? [];
  expect(ids(me("editor", ["read_admin", "manage_menus"]))).toEqual(["menus"]);
  expect(ids(me("admin", ["read_admin", "manage_settings", "manage_menus"]))).toEqual([
    "types",
    "taxonomies",
    "menus",
  ]);
  expect(ids(me("admin", ["read_admin", "manage_settings"]))).not.toContain("menus");
  expect(ids(AUTHOR)).toEqual([]);
  const sections = navFor(me("editor", ["read_admin", "manage_menus"]), TYPES);
  const menus = sections.flatMap((s) => s.items).find((i) => i.id === "menus");
  expect(menus).toMatchObject({ label: "Menus", to: "/structure/menus" });
  expect(navItemAt(sections, "/structure/menus/")?.id).toBe("menus");
});

test("redirects_is_registered", () => {
  // Task 13 registers Redirects under Structure, last: only `manage_redirects` sees it.
  const ids = (who: MeDto) =>
    navFor(who, TYPES)
      .find((s) => s.id === "structure")
      ?.items.map((i) => i.id) ?? [];
  expect(
    ids(me("admin", ["read_admin", "manage_settings", "manage_menus", "manage_redirects"])),
  ).toEqual(["types", "taxonomies", "menus", "redirects"]);
  expect(ids(me("admin", ["read_admin", "manage_redirects"]))).toEqual(["redirects"]);
  expect(ids(me("editor", ["read_admin", "manage_menus"]))).not.toContain("redirects");
  // Not without `read_admin`, whatever else the role holds.
  expect(ids(me("odd", ["manage_redirects"]))).toEqual([]);
  const sections = navFor(me("admin", ["read_admin", "manage_redirects"]), TYPES);
  const redirects = sections.flatMap((s) => s.items).find((i) => i.id === "redirects");
  expect(redirects).toMatchObject({ label: "Redirects", to: "/structure/redirects" });
  expect(redirects?.shortcut).toBeUndefined();
  expect(navItemAt(sections, "/structure/redirects/")?.id).toBe("redirects");
});

test("comments_is_registered", () => {
  // Task 12 registers Comments in Content, after Media: only `moderate_comments` sees it,
  // with `g c`.
  const editor = me("editor", ["read_admin", "edit_others", "moderate_comments"]);
  const items = navFor(editor, TYPES).flatMap((s) => s.items);
  const comments = items.find((i) => i.id === "comments");
  expect(comments).toMatchObject({ label: "Comments", to: "/comments", shortcut: "g c" });
  // Content health (M6-4) follows for `edit_others`.
  expect(items.map((i) => i.id).slice(-3)).toEqual(["media", "comments", "health"]);
  expect(navItemAt(navFor(editor, TYPES), "/comments/")?.id).toBe("comments");
  expect(
    navFor(AUTHOR, TYPES)
      .flatMap((s) => s.items)
      .map((i) => i.id),
  ).not.toContain("comments");
});

test("badges_go_on_their_items", () => {
  const editor = me("editor", ["read_admin", "moderate_comments"]);
  const sections = navFor(editor, TYPES);
  const badged = withBadges(sections, { comments: 3 });
  const badgeOf = (s: typeof sections, id: string) =>
    s.flatMap((x) => x.items).find((i) => i.id === id)?.badge;
  expect(badgeOf(badged, "comments")).toBe(3);
  expect(badgeOf(badged, "media")).toBeUndefined();
  // Zero or unknown: no badge; the input is not changed.
  expect(badgeOf(withBadges(sections, { comments: 0 }), "comments")).toBeUndefined();
  expect(badgeOf(withBadges(sections, {}), "comments")).toBeUndefined();
  expect(badgeOf(sections, "comments")).toBeUndefined();
  // Nothing to badge: the same sections.
  expect(withBadges(sections, {})).toBe(sections);
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

// M2b-3: the Administration section, after Structure. Each screen task registers its own
// entry; every one is registered here, in an order unlike the sidebar's.
const ADMIN_ENTRIES: ScreenEntry[] = [
  ...ENTRIES,
  { screen: "system", to: "/system", label: "nav.system", icon: ShapesIcon },
  { screen: "settings", to: "/settings", label: "nav.settings", icon: ShapesIcon },
  { screen: "plugins", to: "/plugins", label: "nav.plugins", icon: ShapesIcon },
  { screen: "import", to: "/import", label: "nav.import", icon: ShapesIcon },
  { screen: "appearance", to: "/appearance/themes", label: "nav.appearance", icon: ShapesIcon },
  { screen: "users", to: "/users", label: "nav.users", icon: ShapesIcon },
];

test("admin_section_by_role", () => {
  const admin = (who: MeDto) => {
    const sections = navFor(who, TWO_TYPES, undefined, ADMIN_ENTRIES);
    const section = sections.find((s) => s.id === "admin");
    return section
      ? { label: section.label, items: section.items.map((i) => [i.label, i.to, i.shortcut]) }
      : null;
  };
  // Admins: every item, in the fixed order, after Structure, with their chords.
  expect(admin(ADMIN)).toEqual({
    label: "Administration",
    items: [
      ["Users", "/users", "g u"],
      ["Appearance", "/appearance/themes", "g a"],
      ["Plugins", "/plugins", "g l"],
      ["Import", "/import", "g i"],
      ["Settings", "/settings", "g s"],
      ["System", "/system", "g y"],
    ],
  });
  expect(navFor(ADMIN, TWO_TYPES, undefined, ADMIN_ENTRIES).map((s) => s.id)).toEqual([
    "overview",
    "content",
    "structure",
    "admin",
  ]);
  // Editors, authors and contributors hold none of the capabilities: no section.
  for (const who of [EDITOR, AUTHOR, CONTRIBUTOR]) expect(admin(who)).toBeNull();
  // Each item by its own capability.
  const ids = (caps: string[]) =>
    navFor(me("odd", ["read_admin", ...caps]), [], undefined, ADMIN_ENTRIES)
      .find((s) => s.id === "admin")
      ?.items.map((i) => i.id) ?? [];
  expect(ids(["manage_users"])).toEqual(["users"]);
  expect(ids(["manage_themes"])).toEqual(["appearance"]);
  expect(ids(["manage_plugins"])).toEqual(["plugins"]);
  expect(ids(["manage_settings"])).toEqual(["settings"]);
  expect(ids(["manage_system"])).toEqual(["system"]);
  // Not without `read_admin`, whatever else the role holds.
  expect(
    navFor(me("odd", ["manage_users", "manage_system"]), [], undefined, ADMIN_ENTRIES).map(
      (s) => s.id,
    ),
  ).toEqual(["overview"]);
  // `g p` stays Posts.
  const chords = navFor(ADMIN, TWO_TYPES, undefined, ADMIN_ENTRIES)
    .flatMap((s) => s.items)
    .map((i) => i.shortcut)
    .filter((k) => k !== undefined);
  expect(new Set(chords).size).toBe(chords.length);
});

test("nav_entry_only_for_manage_webhooks", () => {
  // M6-6 registers Webhooks under Administration, after Plugins: only `manage_webhooks`
  // sees it, with `g w`.
  const admin = navFor(me("admin", ["read_admin", "manage_plugins", "manage_webhooks"]), TYPES);
  const section = admin.find((s) => s.id === "admin");
  expect(section?.items.map((i) => [i.id, i.label, i.to, i.shortcut])).toEqual([
    ["plugins", "Plugins", "/plugins", "g l"],
    ["webhooks", "Webhooks", "/webhooks", "g w"],
  ]);
  expect(navItemAt(admin, "/webhooks")?.id).toBe("webhooks");
  const others = me("admin", ["read_admin", "manage_system", "manage_settings"]);
  expect(
    navFor(others, TYPES)
      .flatMap((s) => s.items)
      .some((i) => i.id === "webhooks"),
  ).toBe(false);
});

test("nav_entry_health_for_edit_others", () => {
  // M6-4 registers Content health in Content, after Comments: `edit_others`, `g h`.
  const editor = me("editor", ["read_admin", "edit_others"]);
  const item = navFor(editor, TYPES)
    .flatMap((s) => s.items)
    .find((i) => i.id === "health");
  expect(item).toMatchObject({ label: "Content health", to: "/health", shortcut: "g h" });
  expect(
    navFor(AUTHOR, TYPES)
      .flatMap((s) => s.items)
      .some((i) => i.id === "health"),
  ).toBe(false);
});
