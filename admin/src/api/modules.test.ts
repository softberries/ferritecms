import { afterEach, expect, test, vi } from "vitest";
import { findRedirect, importRedirects, listRedirects } from "./redirects";
import { listMedia, restoreMedia } from "./media";
import { listAudit } from "./audit";
import { deletePlugin, inspectPlugin, installPlugin, patchPluginSettings } from "./plugins";
import { installTheme, patchThemeSettings } from "./themes";
import { deleteUser, listUsers } from "./users";
import { createToken } from "./tokens";

function stub() {
  const requests: Request[] = [];
  vi.stubGlobal(
    "fetch",
    vi.fn((r: Request) => {
      requests.push(r);
      return Promise.resolve(
        new Response("{}", { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  return requests;
}
afterEach(() => {
  vi.unstubAllGlobals();
});

test("list_redirects_omits_blank_q", async () => {
  const requests = stub();
  await listRedirects({ q: "   " });
  await listRedirects({ q: " old " });
  expect(new URL(requests[0]!.url).searchParams.has("q")).toBe(false);
  expect(new URL(requests[1]!.url).searchParams.get("q")).toBe("old");
});

test("list_media_omits_blank_q", async () => {
  const requests = stub();
  await listMedia({ q: "", kind: "image" });
  const url = new URL(requests[0]!.url);
  expect(url.searchParams.has("q")).toBe(false);
  expect(url.searchParams.get("kind")).toBe("image");
});

test("restore_media_posts_with_if_match", async () => {
  const requests = stub();
  await restoreMedia("m1", 4);
  expect(requests[0]!.method).toBe("POST");
  expect(new URL(requests[0]!.url).pathname).toBe("/api/v1/media/m1/restore");
  expect(requests[0]!.headers.get("If-Match")).toBe('"4"');
});

test("import_redirects_sends_csv", async () => {
  const requests = stub();
  await importRedirects("a,b,301\n");
  expect(requests[0]!.headers.get("Content-Type")).toBe("text/csv");
  expect(await requests[0]!.text()).toBe("a,b,301\n");
});

test("find_redirect_pages_until_the_exact_source", async () => {
  const requests: Request[] = [];
  const pages = [
    { items: [{ source: "/a/old" }, { source: "/old/" }], next_cursor: "c1" },
    { items: [{ source: "/old" }], next_cursor: null },
  ];
  vi.stubGlobal(
    "fetch",
    vi.fn((r: Request) => {
      requests.push(r);
      const body = JSON.stringify(pages[requests.length - 1]);
      return Promise.resolve(
        new Response(body, { status: 200, headers: { "Content-Type": "application/json" } }),
      );
    }),
  );
  expect(await findRedirect("/old")).toEqual({ source: "/old" });
  const urls = requests.map((r) => new URL(r.url).searchParams);
  expect(urls.map((u) => u.get("q"))).toEqual(["/old", "/old"]);
  expect(urls.map((u) => u.get("cursor"))).toEqual([null, "c1"]);
  // `q` is at most 200 characters: a longer source searches by its first 200.
  requests.length = 0;
  pages.splice(0, 2, { items: [], next_cursor: null });
  expect(await findRedirect(`/${"x".repeat(300)}`)).toBeNull();
  expect(new URL(requests[0]!.url).searchParams.get("q")).toHaveLength(200);
});

test("list_users_trims_q_and_omits_a_blank_one", async () => {
  const requests = stub();
  await listUsers({ q: "  ", role: "editor" });
  await listUsers({ q: " ann " });
  const first = new URL(requests[0]!.url).searchParams;
  expect(first.has("q")).toBe(false);
  expect(first.get("role")).toBe("editor");
  expect(new URL(requests[1]!.url).searchParams.get("q")).toBe("ann");
});

test("delete_user_reassigns_only_when_asked", async () => {
  const requests = stub();
  await deleteUser("u1");
  await deleteUser("u1", "u2");
  expect(requests.map((r) => [r.method, new URL(r.url).search])).toEqual([
    ["DELETE", ""],
    ["DELETE", "?reassign=u2"],
  ]);
});

test("settings_patches_send_only_the_given_keys", async () => {
  const requests = stub();
  await patchPluginSettings("seo", { api_key: null, title: "x" });
  const accent = `#${"123456"}`;
  await patchThemeSettings("blog", { accent });
  expect(requests.map((r) => [r.method, new URL(r.url).pathname])).toEqual([
    ["PATCH", "/api/v1/plugins/seo/settings"],
    ["PATCH", "/api/v1/themes/blog/settings"],
  ]);
  expect(await requests[0]!.json()).toEqual({ api_key: null, title: "x" });
  expect(await requests[1]!.json()).toEqual({ accent });
});

test("plugin_uploads_are_multipart", async () => {
  // jsdom's FormData and Blob are not the ones Node's `Request` takes; borrow Node's
  // (browsers have one kind).
  const nodeForm = await new Response(new URLSearchParams("a=1")).formData();
  vi.stubGlobal("FormData", nodeForm.constructor);
  const NodeBlob = (await new Response("x").blob()).constructor as typeof Blob;
  const requests = stub();
  const manifest = new NodeBlob(["name = 'seo'"]);
  const component = new NodeBlob([new Uint8Array([0, 97, 115, 109])]);
  await inspectPlugin({ manifest, component });
  await installPlugin({ manifest, component, approve: "f1" });
  for (const r of requests) {
    expect(r.headers.get("Content-Type")).toMatch(/^multipart\/form-data; boundary=/);
  }
  const inspected = await requests[0]!.formData();
  expect([...inspected.keys()]).toEqual(["manifest", "component"]);
  const installed = await requests[1]!.formData();
  expect([...installed.keys()]).toEqual(["manifest", "component", "approve"]);
  expect(installed.get("approve")).toBe("f1");
  expect(new URL(requests[1]!.url).pathname).toBe("/api/v1/plugins");
});

test("delete_plugin_purges_only_when_asked", async () => {
  const requests = stub();
  await deletePlugin("seo");
  await deletePlugin("seo", { purge: true });
  expect(requests.map((r) => new URL(r.url).search)).toEqual(["", "?purge=true"]);
});

test("install_theme_sends_the_tar", async () => {
  const requests = stub();
  const tar = new Uint8Array(1024);
  tar[0] = 116;
  await installTheme(tar, { activate: true });
  const r = requests[0]!;
  expect(r.method).toBe("POST");
  expect(new URL(r.url).search).toBe("?activate=true");
  expect(r.headers.get("Content-Type")).toBe("application/x-tar");
  expect(new Uint8Array(await r.arrayBuffer())).toEqual(tar);
});

test("list_audit_passes_its_filters", async () => {
  const requests = stub();
  await listAudit({ action: "user.", actor: "a1", limit: 50 });
  const q = new URL(requests[0]!.url).searchParams;
  expect([q.get("action"), q.get("actor"), q.get("limit")]).toEqual(["user.", "a1", "50"]);
});

test("create_token_posts_for_the_user", async () => {
  const requests = stub();
  await createToken("u1", { name: "CI", scopes: ["content:read"], expires_at: null });
  expect(new URL(requests[0]!.url).pathname).toBe("/api/v1/users/u1/tokens");
  expect(await requests[0]!.json()).toEqual({
    name: "CI",
    scopes: ["content:read"],
    expires_at: null,
  });
});
