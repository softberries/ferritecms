import { describe, expect, test } from "vitest";
import { i18n } from "@/i18n";
import { clientSnippets, monogram, permissionSummary, safeRedirect } from "./model";
import { areasOf, beyondRole, levelsOf, readOnly, scopesOf } from "./scope-levels";

const t = i18n.getFixedT("en", "ai");

describe("scope_levels", () => {
  test("levels_keep_the_strongest_access_per_area", () => {
    expect(levelsOf(["content:read", "content:write", "media:read", "bogus:write", "x"])).toEqual({
      content: "write",
      media: "read",
    });
  });

  test("scopes_are_one_per_area_in_area_order", () => {
    expect(scopesOf({ system: "read", content: "write", media: "none" })).toEqual([
      "content:write",
      "system:read",
    ]);
    expect(areasOf({ media: "none" })).toEqual([]);
  });

  test("read_only_never_widens", () => {
    expect(readOnly({ content: "write", media: "read", terms: "none" })).toEqual({
      content: "read",
      media: "read",
    });
  });

  test("beyond_role_lists_requested_areas_with_nothing_grantable", () => {
    const requested = levelsOf(["content:write", "users:write", "system:read"]);
    const grantable = levelsOf(["content:write", "users:read"]);
    expect(beyondRole(requested, grantable)).toEqual(["system"]);
  });
});

describe("oauth_model", () => {
  test("redirects_refuse_script_and_local_schemes", () => {
    expect(safeRedirect("http://127.0.0.1:3000/cb?code=a&state=b")).toBe(
      "http://127.0.0.1:3000/cb?code=a&state=b",
    );
    expect(safeRedirect("cursor://anysphere.cursor-retrieval/oauth?code=a")).toBe(
      "cursor://anysphere.cursor-retrieval/oauth?code=a",
    );
    for (const bad of ["javascript:alert(1)", "data:text/html,x", "file:///etc", "/relative", ""]) {
      expect(safeRedirect(bad)).toBeNull();
    }
  });

  test("monogram_takes_a_whole_code_point", () => {
    expect(monogram("  claude")).toBe("C");
    expect(monogram("𝒳 app")).toBe("𝒳");
    expect(monogram("")).toBe("?");
  });

  test("permission_summary_names_written_areas", () => {
    expect(permissionSummary(["content:read", "media:read"], t)).toBe("Read only · 2 areas");
    expect(permissionSummary(["content:write", "media:write", "system:read"], t)).toBe(
      "Writes Content and Media · 3 areas",
    );
  });

  test("snippets_carry_the_url", () => {
    const s = clientSnippets("https://example.test/mcp");
    expect(s.claudeCode).toBe("claude mcp add --transport http ferrite https://example.test/mcp");
    expect(s.claudeCodeToken).toContain('--header "Authorization: Bearer <token>"');
    expect(JSON.parse(s.cursor)).toEqual({
      mcpServers: { ferrite: { url: "https://example.test/mcp" } },
    });
    expect(JSON.parse(s.vscode)).toEqual({
      servers: { ferrite: { type: "http", url: "https://example.test/mcp" } },
    });
  });
});
