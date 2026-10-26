// The detail sheet: Title and Alt text save on blur with `If-Match`; a `412` reloads the
// item and shows the server's value with a toast.

import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { toast } from "sonner";
import { afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import type { MediaDto } from "@/api/media";
import type { MeDto } from "@/auth/session";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { MediaDetail } from "./media-detail";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});
afterEach(() => {
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const ME: MeDto = {
  id: "0190a1b2-0000-7000-8000-000000000001",
  login: "author",
  email: "author@example.test",
  display_name: "Author",
  role: "author",
  must_reset: false,
  totp_enabled: false,
  capabilities: ["read_admin", "edit_own", "publish_own", "upload_media"],
  scopes: [],
};

const ITEM: MediaDto = {
  id: "0190a1b2-c3d4-7e5f-8a9b-0c1d2e3f4a5b",
  title: "Harbour",
  alt: "",
  mime: "image/png",
  bytes: 123_456,
  width: 1200,
  height: 800,
  original_name: "harbour.png",
  url: "/_media/harbour.png",
  sanitized: false,
  status: "published",
  author_id: ME.id,
  author: { id: ME.id, login: "author", display_name: "Ada Author" },
  created_at: "2026-10-20T10:00:00Z",
  updated_at: "2026-10-20T10:00:00Z",
  version: 3,
  variants: [
    {
      name: "thumbnail",
      url: "/_media/harbour-150.png",
      width: 150,
      height: 100,
      mime: "image/png",
    },
  ],
};
const PATH = `/api/v1/media/${ITEM.id}`;

describe("media_detail_test", () => {
  test("alt save sends If-Match", async () => {
    const patches: { ifMatch: string | null; body: unknown }[] = [];
    stubRoutes({
      [PATH]: async (_url, req) => {
        if (req.method === "PATCH") {
          const body = (await req.json()) as { alt?: string };
          patches.push({ ifMatch: req.headers.get("If-Match"), body });
          return json({ ...ITEM, alt: body.alt ?? "", version: 4 });
        }
        return json(ITEM);
      },
    });
    const onAction = vi.fn();
    renderWithRouter(<MediaDetail id={ITEM.id} me={ME} onClose={() => {}} onAction={onAction} />);
    const alt = await screen.findByRole("textbox", { name: "Alt text" });
    // The uploader may trash their own file.
    expect(screen.getByRole("button", { name: "Move to trash" })).toBeVisible();
    await userEvent.type(alt, "Boats at dusk");
    await userEvent.tab();
    await waitFor(() => expect(patches).toHaveLength(1));
    expect(patches[0]).toEqual({ ifMatch: '"3"', body: { alt: "Boats at dusk" } });
    // The next save goes with the new version; an unchanged field sends nothing.
    const title = screen.getByRole("textbox", { name: "Title" });
    await userEvent.click(title);
    await userEvent.tab();
    await userEvent.clear(title);
    await userEvent.type(title, "Harbour at dusk");
    await userEvent.tab();
    await waitFor(() => expect(patches).toHaveLength(2));
    expect(patches[1]).toEqual({ ifMatch: '"4"', body: { title: "Harbour at dusk" } });
  });

  test("412 reloads and shows the server's value", async () => {
    const info = vi.spyOn(toast, "info");
    let gets = 0;
    stubRoutes({
      [PATH]: (_url, req) => {
        if (req.method === "PATCH") {
          return new Response(
            JSON.stringify({
              type: "https://ferritecms.dev/problems/precondition-failed",
              title: "Precondition failed",
              status: 412,
              detail: "The item changed since you loaded it.",
            }),
            { status: 412, headers: { "Content-Type": "application/problem+json" } },
          );
        }
        gets += 1;
        return json(gets === 1 ? ITEM : { ...ITEM, alt: "Set by someone else", version: 5 });
      },
    });
    renderWithRouter(<MediaDetail id={ITEM.id} me={ME} onClose={() => {}} />);
    const alt = await screen.findByRole("textbox", { name: "Alt text" });
    await userEvent.type(alt, "Mine");
    await userEvent.tab();
    await waitFor(() => expect(alt).toHaveValue("Set by someone else"));
    expect(gets).toBe(2);
    expect(info).toHaveBeenCalledTimes(1);
    const [message, options] = info.mock.calls[0] ?? [];
    expect(message).toBe("Someone else changed this file");
    expect(String((options as { description?: unknown } | undefined)?.description)).toContain(
      "Alt text",
    );
  });

  test("a viewer who may not edit sees the values read-only", async () => {
    stubRoutes({
      [PATH]: () =>
        json({
          ...ITEM,
          author_id: "someone-else",
          author: { id: "someone-else", login: "bo", display_name: "Bo Other" },
        }),
    });
    renderWithRouter(<MediaDetail id={ITEM.id} me={ME} onClose={() => {}} onAction={() => {}} />);
    const alt = await screen.findByRole("textbox", { name: "Alt text" });
    expect(alt).toHaveAttribute("readonly");
    // The uploader by name (`MediaDto.author`).
    expect(screen.getByText("Bo Other")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Move to trash" })).toBeNull();
  });
});
