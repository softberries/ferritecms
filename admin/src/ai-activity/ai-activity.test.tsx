import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { ChangeDto, ChangeSetDto } from "@/api/change-sets";
import { i18n } from "@/i18n";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ActivityList } from "./activity-list";
import { ChangeSetScreen } from "./change-set-screen";
import { conflictText, contentFieldChanges, forceable, jsonDiff } from "./model";

const t = i18n.getFixedT(null, "ai");
const SET = "0196f6b0-0000-7000-8000-00000000c501";
const USER = "0196f6b0-0000-7000-8000-00000000a001";
const EDITOR = "0196f6b0-0000-7000-8000-00000000a002";
const NOW = new Date().toISOString();

function set(over: Partial<ChangeSetDto> = {}): ChangeSetDto {
  return {
    id: SET,
    user: { id: USER, login: "ada", display_name: "Ada" },
    agent: {
      kind: "oauth",
      id: "0196f6b0-0000-7000-8000-00000000b001",
      client: "Claude",
      connected: true,
    },
    started_at: NOW,
    last_at: NOW,
    open: true,
    changes: 2,
    reverted: 0,
    status: "open",
    ...over,
  };
}

function change(over: Partial<ChangeDto> = {}): ChangeDto {
  return {
    seq: 2,
    at: NOW,
    op: "content.update",
    object: { kind: "content", id: "x1", label: "Spring hours", admin_path: "/content/post/x1" },
    created: false,
    deleted: false,
    revertable: true,
    reverted_at: null,
    reverted_by: null,
    forced: false,
    ...over,
  };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

test("model_json_diff_and_conflicts", () => {
  expect(jsonDiff({ a: 1, b: { c: [1, 2] } }, { a: 1, b: { c: [1, 3] }, d: "x" })).toEqual([
    { path: "b.c.1", before: 2, after: 3 },
    { path: "d", after: "x" },
  ]);
  expect(jsonDiff(undefined, { value: 1 })).toEqual([{ path: "(value)", after: { value: 1 } }]);
  expect(
    contentFieldChanges({ title: "A", status: "draft" }, { title: "B", status: "published" }),
  ).toEqual([{ path: "status", before: "draft", after: "published" }]);
  const changed = {
    seq: 1,
    reason: "changed_since" as const,
    object: { kind: "content", id: "x", label: "X" },
    by: EDITOR,
    at: NOW,
  };
  expect(
    conflictText(
      t,
      changed,
      () => "Ana",
      () => "10 minutes ago",
    ),
  ).toBe("Edited by Ana 10 minutes ago");
  expect(forceable([changed])).toBe(true);
  expect(forceable([{ ...changed, reason: "gone" }])).toBe(false);
  expect(forceable([])).toBe(false);
});

test("list_renders_sets_with_status_badges", async () => {
  const seen = stubRoutes({
    "/api/v1/change-sets": () =>
      json({
        items: [
          set(),
          set({
            id: "0196f6b0-0000-7000-8000-00000000c502",
            agent: {
              kind: "token",
              id: "0196f6b0-0000-7000-8000-00000000b002",
              client: "API token cli",
              connected: false,
            },
            status: "reverted",
            open: false,
            reverted: 2,
          }),
        ],
        next_cursor: null,
      }),
  });
  renderWithRouter(<ActivityList search={{ all: true }} setSearch={() => {}} admin />);
  await screen.findByText("Claude");
  expect(screen.getByText("API token cli")).toBeTruthy();
  const states = [...document.querySelectorAll("[data-slot='set-status']")].map((s) =>
    s.getAttribute("data-status"),
  );
  expect(states).toEqual(["open", "reverted"]);
  expect(seen[0]?.searchParams.get("user")).toBe("all");
});

test("empty_list_explains", async () => {
  stubRoutes({ "/api/v1/change-sets": () => json({ items: [], next_cursor: null }) });
  renderWithRouter(<ActivityList search={{}} setSearch={() => {}} admin={false} />);
  await screen.findByText(t("activity.empty.title"));
});

function detailRoutes(revert: (body: Record<string, unknown>) => Response) {
  return stubRoutes({
    [`/api/v1/change-sets/${SET}`]: () =>
      json({
        set: set(),
        changes: [
          change(),
          change({
            seq: 1,
            op: "settings.update",
            object: { kind: "option", id: "site.title", label: "", admin_path: "/settings" },
          }),
        ],
      }),
    [`/api/v1/change-sets/${SET}/changes/2`]: () =>
      json({
        change: change(),
        images: "shown",
        before: {
          title: "Hours",
          excerpt: "",
          body: { type: "doc", content: [] },
          status: "draft",
        },
        after: {
          title: "Spring hours",
          excerpt: "",
          body: { type: "doc", content: [] },
          status: "published",
        },
      }),
    [`/api/v1/change-sets/${SET}/changes/1`]: () =>
      json({ change: change({ seq: 1 }), images: "hidden", before: null, after: null }),
    [`/api/v1/change-sets/${SET}/revert`]: async (_url, req) =>
      revert((await req.json()) as Record<string, unknown>),
  });
}

test("detail_renders_content_diff_and_hidden_images", async () => {
  detailRoutes(() => json({ reverted: [], skipped: [], forced: [], notes: [], dry_run: true }));
  renderWithRouter(<ChangeSetScreen id={SET} />);
  await screen.findByText("Spring hours");
  const user = userEvent.setup();
  await user.click(screen.getByRole("button", { name: /Show the changes of Spring hours/ }));
  const diff = await waitFor(() => {
    const d = document.querySelector("[data-slot='content-change-diff']");
    if (!d) throw new Error("no diff yet");
    return d as HTMLElement;
  });
  expect(diff.querySelector("[data-diff='added']")?.textContent).toContain("Spring");
  expect(within(diff).getByText("status")).toBeTruthy();
  await user.click(screen.getByRole("button", { name: /Show the changes of site.title/ }));
  await screen.findByText(t("activity.hidden"));
});

test("revert_dialog_dry_runs_and_needs_overwrite_for_conflicts", async () => {
  const bodies: Record<string, unknown>[] = [];
  detailRoutes((body) => {
    bodies.push(body);
    if (body.force === true) {
      return json({ reverted: [2, 1], skipped: [], forced: [2], notes: [], dry_run: false });
    }
    return new Response(
      JSON.stringify({
        type: "https://ferrite.dev/problems/change-conflict",
        title: "Changes conflict",
        status: 409,
        detail: "see report",
        report: {
          conflicts: [
            {
              seq: 2,
              reason: "changed_since",
              object: { kind: "content", id: "x1", label: "Spring hours" },
              by: USER,
              at: NOW,
            },
          ],
        },
      }),
      { status: 409, headers: { "Content-Type": "application/problem+json" } },
    );
  });
  renderWithRouter(<ChangeSetScreen id={SET} />);
  const user = userEvent.setup();
  await user.click(await screen.findByRole("button", { name: t("activity.revertAll") }));
  const dialog = await screen.findByRole("alertdialog");
  await within(dialog).findByText(/Edited by Ada/);
  expect(bodies[0]).toMatchObject({ dry_run: true, force: false });
  const confirm = within(dialog).getByRole("button", { name: t("activity.dialog.confirm") });
  expect((confirm as HTMLButtonElement).disabled).toBe(true);
  await user.click(within(dialog).getByRole("checkbox"));
  expect((confirm as HTMLButtonElement).disabled).toBe(false);
  await user.click(confirm);
  await waitFor(() => expect(bodies.at(-1)).toMatchObject({ dry_run: false, force: true }));
});
