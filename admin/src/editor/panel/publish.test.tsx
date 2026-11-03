// The status section and the publish dialog (Review Focus 4): the buttons come from the
// item's `allowed` and nothing else; a schedule time in the past is refused before any
// request; the RFC 3339 conversion keeps the wall-clock time the user picked.

import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import type { SchemaAllowedActionsDto, SchemaContentDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { type EditorDocument, metaOf } from "../editor-state";
import { PublishDialog } from "./publish-dialog";
import {
  type PanelAction,
  defaultScheduleWall,
  formatWhen,
  isPast,
  notAParent,
  predictPermalink,
  statusActions,
  utcToWallClock,
  wallClockToUtc,
} from "./publishing";
import { StatusSection } from "./status-section";

beforeAll(() => {
  globalThis.ResizeObserver ??= class {
    observe() {}
    unobserve() {}
    disconnect() {}
  } as unknown as typeof ResizeObserver;
});

afterEach(() => {
  vi.useRealTimers();
});

const NONE: SchemaAllowedActionsDto = {
  edit: false,
  publish: false,
  schedule: false,
  submit: false,
  unpublish: false,
  trash: false,
  trash_undoable: false,
  needs_approval: false,
  stage: false,
  override: false,
  restore: false,
  delete: false,
};

function itemWith(status: string, allowed: Partial<SchemaAllowedActionsDto>): SchemaContentDto {
  return {
    id: "0190f1c0-0000-7000-8000-0000000000aa",
    type: "post",
    title: "Spring notes",
    slug: "spring-notes",
    status,
    body: { type: "doc", content: [] },
    excerpt: "",
    fields: {},
    seo: { title: null, description: null, canonical: null, robots: [] },
    locale: "en",
    translation_group: "0190f1c0-0000-7000-8000-0000000000aa",
    translations: [],
    terms: [],
    menu_order: 0,
    permalink: "/2026/10/spring-notes/",
    author_id: "0190f1c0-0000-7000-8000-0000000000bb",
    author: { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" },
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version: 4,
    allowed: { ...NONE, ...allowed },
  };
}

function fakeDoc(
  item: SchemaContentDto | null,
  extra: Partial<EditorDocument> = {},
): EditorDocument {
  const meta = metaOf(item);
  return {
    item,
    title: item?.title ?? "",
    setTitle: vi.fn(),
    readOnly: null,
    meta,
    savedMeta: meta,
    setMeta: vi.fn(),
    setField: vi.fn(),
    pending: null,
    storedBody: item?.body,
    initialFocus: null,
    dirty: false,
    saveState: "saved",
    savedAt: null,
    indicator: "saved",
    indicatorAt: null,
    unprotected: false,
    restored: null,
    draft: vi.fn(() => ({
      base_version: item?.version ?? 0,
      title: item?.title ?? "",
      excerpt: null,
      body: item?.body ?? null,
      fields: {},
      slug: null,
      parent_id: null,
      menu_order: null,
      terms: null,
    })),
    isDirtyNow: vi.fn(() => false),
    keepMine: vi.fn(() => Promise.resolve(item)),
    rebase: vi.fn(),
    keepInSlot: vi.fn(() => Promise.resolve()),
    discard: vi.fn(() => Promise.resolve()),
    kept: null,
    keptUnstored: false,
    storeKept: vi.fn(() => Promise.resolve(true)),
    discardKept: vi.fn(() => Promise.resolve()),
    backup: null,
    dropBackup: vi.fn(),
    backupNow: () => null,
    save: vi.fn(() => Promise.resolve(item)),
    transition: vi.fn(() => Promise.resolve(item)),
    adopt: vi.fn(),
    restoreRevision: vi.fn(() => Promise.resolve(item)),
    restoring: false,
    fieldDefs: null,
    fieldErrors: {},
    clearFieldError: vi.fn(),
    saveAttempted: false,
    onAutosaved: () => () => {},
    setAutosaveDebounce: vi.fn(),
    autosaves: true,
    editor: {} as EditorDocument["editor"],
    ...extra,
  };
}

function wrap(ui: ReactNode) {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return render(
    <QueryClientProvider client={qc}>
      <TooltipProvider>{ui}</TooltipProvider>
    </QueryClientProvider>,
  );
}

const ids = (a: { primary: PanelAction[]; secondary: PanelAction[] }) => [
  ...a.primary,
  "|",
  ...a.secondary,
];

describe("publish", () => {
  test("buttons_follow_allowed", () => {
    // The four roles' draft: admin/editor/author (publish) and contributor (submit only).
    expect(
      ids(
        statusActions(
          itemWith("draft", { edit: true, publish: true, schedule: true, submit: true }),
          false,
        ),
      ),
    ).toEqual(["publish", "schedule", "save", "|", "preview"]);
    expect(ids(statusActions(itemWith("draft", { edit: true, submit: true }), false))).toEqual([
      "submit",
      "save",
      "|",
      "preview",
    ]);
    // A contributor's pending item: no submit (only from draft), still editable.
    expect(ids(statusActions(itemWith("pending", { edit: true }), false))).toEqual([
      "save",
      "|",
      "preview",
    ]);
    // An editor reviewing someone's pending item.
    expect(
      ids(statusActions(itemWith("pending", { edit: true, publish: true, schedule: true }), false)),
    ).toEqual(["publish", "schedule", "save", "|", "preview"]);
    expect(
      ids(
        statusActions(itemWith("scheduled", { edit: true, publish: true, unpublish: true }), false),
      ),
    ).toEqual(["save", "publishNow", "|", "unpublish", "preview"]);
    expect(
      ids(statusActions(itemWith("published", { edit: true, unpublish: true }), false)),
    ).toEqual(["save", "|", "unpublish", "preview"]);
    // Someone else's published post an author can only read.
    expect(ids(statusActions(itemWith("published", {}), false))).toEqual(["|", "preview"]);
    // Read-only screens: Preview only; trash: nothing.
    expect(ids(statusActions(itemWith("draft", { edit: true, publish: true }), true))).toEqual([
      "|",
      "preview",
    ]);
    expect(ids(statusActions(itemWith("trash", { restore: true }), false))).toEqual(["|"]);
    // A new item: only saving it as a draft.
    expect(ids(statusActions(null, false))).toEqual(["save", "|"]);
  });

  test("buttons_rendered_for_each_allowed_combination", () => {
    const flags = ["edit", "publish", "schedule", "submit", "unpublish"] as const;
    const statuses = ["draft", "pending", "scheduled", "published"];
    for (const status of statuses) {
      for (let mask = 0; mask < 1 << flags.length; mask++) {
        const allowed = Object.fromEntries(flags.map((f, i) => [f, (mask & (1 << i)) !== 0]));
        const item = itemWith(status, allowed);
        const { unmount } = wrap(<StatusSection doc={fakeDoc(item)} onPublish={vi.fn()} />);
        const shown = [
          ...document.querySelectorAll<HTMLElement>("[data-slot=panel-actions] [data-action]"),
        ].map((el) => el.dataset.action);
        const expected = statusActions(item, false);
        expect(shown, `${status} ${JSON.stringify(allowed)}`).toEqual(
          [...expected.primary, ...expected.secondary].filter((a) => a !== "preview"),
        );
        // Never a Publish without `allowed.publish`, never a Submit without `allowed.submit`.
        if (!allowed.publish) expect(screen.queryByRole("button", { name: /^Publish/ })).toBeNull();
        if (!allowed.submit)
          expect(screen.queryByRole("button", { name: "Submit for review" })).toBeNull();
        unmount();
      }
    }
  });

  test("contributor_sees_submit_not_publish", async () => {
    const item = itemWith("draft", { edit: true, submit: true, trash: true });
    const doc = fakeDoc(item, { dirty: true });
    wrap(<StatusSection doc={doc} onPublish={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Publish…" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Schedule…" })).toBeNull();
    await userEvent.setup().click(screen.getByRole("button", { name: "Submit for review" }));
    expect(doc.transition).toHaveBeenCalledWith("submit");
  });

  test("schedule_in_the_past_is_refused", async () => {
    const item = itemWith("draft", { edit: true, publish: true, schedule: true });
    const doc = fakeDoc(item);
    wrap(
      <PublishDialog
        doc={doc}
        mode="schedule"
        onClose={vi.fn()}
        siteUrl="http://localhost:8080"
        taxonomies={[]}
        terms={new Map()}
      />,
    );
    const dialog = screen.getByRole("dialog");
    const user = userEvent.setup();
    // Today at 00:00 has passed.
    await pickDay(dialog, user, new Date());
    const time = within(dialog).getByLabelText("Time");
    await user.clear(time);
    await user.type(time, "00:00");
    expect(within(dialog).getByRole("alert")).toHaveTextContent("That time has passed");
    const confirm = within(dialog).getByRole("button", { name: /^Schedule for/ });
    expect(confirm).toBeDisabled();
    await user.click(confirm);
    expect(doc.transition).not.toHaveBeenCalled();
    // A future time is accepted and sent as RFC 3339 UTC.
    await pickDay(dialog, user, tomorrow());
    await user.clear(time);
    await user.type(time, "09:30");
    expect(within(dialog).queryByRole("alert")).toBeNull();
    await act(async () => {
      await user.click(within(dialog).getByRole("button", { name: /^Schedule for/ }));
    });
    const call = vi.mocked(doc.transition).mock.calls[0];
    expect(call?.[0]).toBe("schedule");
    const at = call?.[1]?.at ?? "";
    expect(at).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:00Z$/);
    const local = new Date(at);
    expect([local.getHours(), local.getMinutes()]).toEqual([9, 30]);
  });

  test("publish_now_summarises_and_confirms", async () => {
    const item = itemWith("draft", { edit: true, publish: true, schedule: true });
    const doc = fakeDoc(item, { dirty: true });
    wrap(
      <PublishDialog
        doc={doc}
        mode="now"
        onClose={vi.fn()}
        siteUrl="http://localhost:8080"
        taxonomies={[]}
        terms={new Map()}
      />,
    );
    const dialog = screen.getByRole("dialog");
    const summary = dialog.querySelector("[data-slot=publish-summary]");
    expect(summary).toHaveTextContent("Spring notes");
    expect(summary).toHaveTextContent("localhost:8080/2026/10/spring-notes/");
    expect(summary).toHaveTextContent("Taken from the start of the body");
    expect(dialog).toHaveTextContent("Your unsaved changes are saved first.");
    await act(async () => {
      await userEvent.setup().click(within(dialog).getByRole("button", { name: "Publish now" }));
    });
    expect(vi.mocked(doc.transition).mock.calls[0]?.[0]).toBe("publish");
  });

  test("rfc3339_keeps_the_wall_clock_time", () => {
    // Winter (CET, +1) and summer (CEST, +2) in Warsaw; New York (-5/-4); UTC; Kolkata (+5:30).
    expect(wallClockToUtc({ date: "2026-11-03", time: "09:30" }, "Europe/Warsaw")).toBe(
      "2026-11-03T08:30:00Z",
    );
    expect(wallClockToUtc({ date: "2026-07-01", time: "09:30" }, "Europe/Warsaw")).toBe(
      "2026-07-01T07:30:00Z",
    );
    expect(wallClockToUtc({ date: "2026-12-31", time: "23:15" }, "America/New_York")).toBe(
      "2027-01-01T04:15:00Z",
    );
    expect(wallClockToUtc({ date: "2026-06-15", time: "00:00" }, "UTC")).toBe(
      "2026-06-15T00:00:00Z",
    );
    expect(wallClockToUtc({ date: "2026-03-10", time: "18:00" }, "Asia/Kolkata")).toBe(
      "2026-03-10T12:30:00Z",
    );
    // The day the clocks change (Warsaw, 25 Oct 2026, 03:00 → 02:00): still the chosen time.
    expect(wallClockToUtc({ date: "2026-10-25", time: "12:00" }, "Europe/Warsaw")).toBe(
      "2026-10-25T11:00:00Z",
    );
    // The edges: a time the spring change skips (Warsaw 29 Mar 2026, 02:00 → 03:00)
    // moves forward by the gap (02:30 → 03:30 CEST); an ambiguous one in the autumn
    // (25 Oct 2026, 02:30 happens twice) takes the later, standard-time instant.
    expect(wallClockToUtc({ date: "2026-03-29", time: "02:30" }, "Europe/Warsaw")).toBe(
      "2026-03-29T01:30:00Z",
    );
    expect(wallClockToUtc({ date: "2026-10-25", time: "02:30" }, "Europe/Warsaw")).toBe(
      "2026-10-25T01:30:00Z",
    );
    // And back again.
    for (const [zone, wall] of [
      ["Europe/Warsaw", { date: "2026-11-03", time: "09:30" }],
      ["America/New_York", { date: "2026-07-04", time: "17:45" }],
      ["Asia/Kolkata", { date: "2026-01-01", time: "00:05" }],
    ] as const) {
      const utc = wallClockToUtc(wall, zone);
      expect(utc && utcToWallClock(utc, zone)).toEqual(wall);
    }
    expect(wallClockToUtc({ date: "2026-13-01", time: "09:30" }, "UTC")).toBeNull();
    expect(wallClockToUtc({ date: "2026-11-03", time: "9:30" }, "UTC")).toBeNull();
  });

  test("past_and_default_times", () => {
    const now = Date.parse("2026-10-26T12:00:00Z");
    expect(isPast("2026-10-26T11:59:00Z", now)).toBe(true);
    expect(isPast("2026-10-26T12:00:00Z", now)).toBe(true);
    expect(isPast("2026-10-26T12:01:00Z", now)).toBe(false);
    expect(defaultScheduleWall("Europe/Warsaw", now)).toEqual({
      date: "2026-10-27",
      time: "09:00",
    });
    // Late evening in New York is already tomorrow in UTC: tomorrow is New York's.
    expect(defaultScheduleWall("America/New_York", Date.parse("2026-10-27T03:00:00Z"))).toEqual({
      date: "2026-10-27",
      time: "09:00",
    });
  });

  test("permalink_and_parent_helpers", () => {
    expect(predictPermalink("/2026/10/spring-notes/", "spring-notes", "Spring")).toBe(
      "/2026/10/spring/",
    );
    expect(predictPermalink("/about/team/", "team", "people")).toBe("/about/people/");
    expect(predictPermalink("/p/123/", "team", "people")).toBeNull();
    expect(predictPermalink("/about/", "about", "")).toBe("/about/");
    const items = [
      { id: "a", parent_id: null },
      { id: "b", parent_id: "a" },
      { id: "c", parent_id: "b" },
      { id: "d", parent_id: null },
    ];
    expect([...notAParent("a", items)].sort()).toEqual(["a", "b", "c"]);
    expect([...notAParent(null, items)]).toEqual([]);
    // A time with its zone named (Intl refuses `dateStyle` with `timeZoneName`).
    expect(formatWhen("2026-11-03T08:30:00Z")).toMatch(/2026/);
  });
});

/**
 * Clicks `day` in the calendar, moving a month either way when it is not in the shown
 * grid (the picker opens on tomorrow's month; today or tomorrow can sit across a month
 * boundary).
 */
async function pickDay(
  dialog: HTMLElement,
  user: ReturnType<typeof userEvent.setup>,
  day: Date,
): Promise<void> {
  const find = () => dialog.querySelector<HTMLElement>(`[data-day="${day.toLocaleDateString()}"]`);
  for (const nav of [null, ".rdp-button_next", ".rdp-button_previous", ".rdp-button_previous"]) {
    if (nav) {
      const button = dialog.querySelector<HTMLElement>(nav);
      if (button && button.getAttribute("aria-disabled") !== "true") await user.click(button);
    }
    const cell = find();
    if (cell) {
      await user.click(cell);
      return;
    }
  }
  throw new Error(`no calendar cell for ${day.toDateString()}`);
}

function tomorrow(): Date {
  const d = new Date();
  d.setDate(d.getDate() + 1);
  return d;
}
