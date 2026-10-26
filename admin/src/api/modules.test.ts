import { afterEach, expect, test, vi } from "vitest";
import { importRedirects, listRedirects } from "./redirects";
import { listMedia, restoreMedia } from "./media";

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
