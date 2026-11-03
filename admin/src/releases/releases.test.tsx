// The release screens (M6-3, design 15 §11.3): the entries table's chips and conflict
// badge with Rebase, Check's inline report, the apply dialog listing what goes live (with
// the admins' override), the share dialog showing a token once, and a failed release's
// report on top.

import { QueryClient } from "@tanstack/react-query";
import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import type { ReleaseDetail } from "@/api/releases";
import type { MeDto } from "@/auth/session";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes, type Handler } from "@/test/render";
import { ITEM_A, ITEM_B, ME, REL, entry, release } from "./fixtures";
import { ReleaseDetailScreen } from "./release-detail";
import { ShareDialog } from "./share-dialog";

beforeAll(() => {
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const BASE = `/api/v1/releases/${REL}`;

function me(role: "admin" | "editor" = "editor"): MeDto {
  return {
    id: ME,
    login: role,
    email: `${role}@example.test`,
    display_name: role,
    role,
    capabilities: ["read_admin", "edit_own", "edit_others", "publish_own", "publish_others"],
    scopes: [],
    csrf_token: "t",
    must_reset: false,
    totp_enabled: false,
  };
}

function open(r: ReleaseDetail, extra: Record<string, Handler> = {}, role?: "admin" | "editor") {
  const calls: { method: string; path: string; body: unknown; ifMatch: string | null }[] = [];
  const record =
    (h: Handler): Handler =>
    async (url, req) => {
      const text = req.method === "GET" ? "" : await req.clone().text();
      calls.push({
        method: req.method,
        path: url.pathname,
        body: text ? JSON.parse(text) : null,
        ifMatch: req.headers.get("If-Match"),
      });
      return h(url, req);
    };
  const table: Record<string, Handler> = {
    [BASE]: () => json(r),
    [`${BASE}/preview-links`]: () => json({ items: [] }),
    ...extra,
  };
  stubRoutes(Object.fromEntries(Object.entries(table).map(([k, h]) => [k, record(h)])));
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  client.setQueryData(qk.me, me(role));
  renderWithRouter(
    <TooltipProvider>
      <ReleaseDetailScreen id={REL} />
    </TooltipProvider>,
    client,
  );
  return calls;
}

test("entries_show_their_action_chips", async () => {
  open(release());
  const table = await screen.findByRole("table", { name: "Entries" });
  const rows = within(table).getAllByRole("row").slice(1);
  expect(within(rows[0]!).getByText("About us")).toBeInTheDocument();
  expect(within(rows[0]!).getByText("Changes")).toBeInTheDocument();
  expect(within(rows[0]!).queryByText("Publish")).toBeNull();
  expect(within(rows[1]!).getByText("Spring sale")).toBeInTheDocument();
  expect(within(rows[1]!).getByText("Publish")).toBeInTheDocument();
  expect(within(rows[1]!).queryByText("Changes")).toBeNull();
  // An entry with a staged change opens it in change mode.
  expect(within(rows[0]!).getByRole("link", { name: "About us" })).toHaveAttribute(
    "href",
    `/admin/content/page/${ITEM_A}?release=${REL}`,
  );
  expect(within(rows[1]!).getByRole("link", { name: "Spring sale" })).toHaveAttribute(
    "href",
    `/admin/content/page/${ITEM_B}`,
  );
});

test("a_conflict_shows_a_badge_and_rebases", async () => {
  const conflicted = release({
    entries: [entry({ conflict: true, item_version: 5 }), release().entries[1]!],
  });
  const calls = open(conflicted, {
    [`${BASE}/items/${ITEM_A}/rebase`]: () => json(release()),
  });
  const table = await screen.findByRole("table", { name: "Entries" });
  const row = within(table).getAllByRole("row")[1]!;
  expect(within(row).getByText("Conflict")).toBeInTheDocument();
  await userEvent.click(within(row).getByRole("button", { name: "Rebase" }));
  await waitFor(() =>
    expect(calls.find((c) => c.path.endsWith("/rebase"))).toMatchObject({
      method: "POST",
      ifMatch: '"4"',
    }),
  );
  await waitFor(() => expect(within(table).queryByText("Conflict")).toBeNull());
});

test("check_reports_inline", async () => {
  open(release(), {
    [`${BASE}/check`]: () =>
      json({
        clean: false,
        entries: [
          { item: ITEM_A, outcome: "ok" },
          { item: ITEM_B, outcome: "invalid", field: "slug", message: "slug is taken" },
        ],
      }),
  });
  await screen.findByRole("table", { name: "Entries" });
  await userEvent.click(screen.getByRole("button", { name: "Check" }));
  const report = await screen.findByRole("region", { name: "Check report" });
  expect(within(report).getByText(/Spring sale/)).toBeInTheDocument();
  expect(within(report).getByText(/slug is taken/)).toBeInTheDocument();
  expect(within(report).getByText(/1 of 2 entries needs attention/)).toBeInTheDocument();
});

test("apply_dialog_lists_what_goes_live", async () => {
  const calls = open(release(), {
    [`${BASE}/apply`]: () =>
      json(release({ status: "applied", applied_at: "2026-11-03T11:00:00Z" })),
  });
  await screen.findByRole("table", { name: "Entries" });
  await userEvent.click(screen.getByRole("button", { name: "Apply now…" }));
  const dialog = await screen.findByRole("dialog", { name: "Apply “Spring launch” now?" });
  const live = within(dialog).getByRole("list", { name: "What goes live" });
  const items = within(live).getAllByRole("listitem");
  expect(items[0]).toHaveTextContent("About us");
  expect(items[0]).toHaveTextContent("Changes go live");
  expect(items[1]).toHaveTextContent("Spring sale");
  expect(items[1]).toHaveTextContent("Published");
  // Editors get no override.
  expect(within(dialog).queryByText("Override approval")).toBeNull();
  await userEvent.click(within(dialog).getByRole("button", { name: "Apply now" }));
  await waitFor(() =>
    expect(calls.find((c) => c.path.endsWith("/apply"))).toMatchObject({
      method: "POST",
      ifMatch: '"4"',
      body: {},
    }),
  );
});

test("an_admin_overrides_with_a_reason", async () => {
  const calls = open(
    release(),
    { [`${BASE}/apply`]: () => json(release({ status: "applied" })) },
    "admin",
  );
  await screen.findByRole("table", { name: "Entries" });
  await userEvent.click(screen.getByRole("button", { name: "Apply now…" }));
  const dialog = await screen.findByRole("dialog");
  await userEvent.click(within(dialog).getByText("Override approval"));
  const confirm = within(dialog).getByRole("button", { name: "Apply now" });
  await userEvent.click(within(dialog).getByRole("checkbox", { name: /Apply without/ }));
  expect(confirm).toBeDisabled();
  await userEvent.type(within(dialog).getByLabelText("Reason"), "Launch event");
  await userEvent.click(confirm);
  await waitFor(() =>
    expect(calls.find((c) => c.path.endsWith("/apply"))?.body).toEqual({
      override_reason: "Launch event",
    }),
  );
});

test("a_failed_release_shows_its_report_on_top", async () => {
  open(
    release({
      status: "failed",
      failure: {
        at: "2026-11-03T12:00:00Z",
        reason: "conflict",
        entries: [{ item: ITEM_A, outcome: "conflict", current_version: 7 }],
      },
    }),
  );
  const banner = await screen.findByRole("alert");
  expect(banner).toHaveTextContent("The scheduled apply failed");
  expect(banner).toHaveTextContent("About us");
  expect(banner).toHaveTextContent("changed since it was added");
});

test("share_dialog_shows_the_token_once_and_revokes", async () => {
  const made: unknown[] = [];
  let links = [
    {
      id: "old",
      target_kind: "release",
      target_id: REL,
      created_by: ME,
      created_at: "2026-11-02T10:00:00Z",
      expires_at: "2026-11-09T10:00:00Z",
    },
  ];
  const revoked: string[] = [];
  stubRoutes({
    [`${BASE}/preview-links`]: async (_u, req) => {
      if (req.method === "POST") {
        made.push(await req.json());
        return json(
          {
            id: "new",
            target_kind: "release",
            target_id: REL,
            created_by: ME,
            created_at: "2026-11-03T10:00:00Z",
            expires_at: "2026-11-04T10:00:00Z",
            token: `ferp_${"x".repeat(43)}`,
            url: `http://localhost:8080/_preview/link/ferp_${"x".repeat(43)}`,
          },
          201,
        );
      }
      return json({ items: links });
    },
    "/api/v1/preview-links/old": () => {
      revoked.push("old");
      links = [];
      return new Response(null, { status: 204 });
    },
  });
  renderWithRouter(
    <ShareDialog target={{ kind: "release", id: REL }} open onOpenChange={() => {}} />,
  );
  const dialog = await screen.findByRole("dialog", { name: "Share preview" });
  await userEvent.click(within(dialog).getByRole("radio", { name: "24 hours" }));
  await userEvent.click(within(dialog).getByRole("button", { name: "Create link" }));
  const field = await within(dialog).findByLabelText("Preview link");
  expect(field).toHaveValue(`http://localhost:8080/_preview/link/ferp_${"x".repeat(43)}`);
  expect(made).toEqual([{ expires_in_hours: 24 }]);
  expect(within(dialog).getByText(/Shown only now/)).toBeInTheDocument();
  // The earlier link, with Revoke.
  const list = within(dialog).getByRole("list", { name: "Links" });
  await userEvent.click(within(list).getAllByRole("button", { name: "Revoke" })[0]!);
  await waitFor(() => expect(revoked).toEqual(["old"]));
});
