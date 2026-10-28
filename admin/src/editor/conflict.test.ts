// The 412 conflict (07 §1, Review Focus 3): which members differ between the server's
// newer copy and the working draft, Keep mine saving over it with the newer version as
// the `If-Match`, Take theirs keeping mine in the autosave slot first, and a transition
// that waited for the refused save not running behind the user's back (Task 15 carry).

import { act, cleanup, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { Link, useLocation } from "@tanstack/react-router";
import { Fragment, createElement } from "react";
import { toast } from "sonner";
import { afterEach, beforeAll, describe, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import { ApiError } from "@/api/problem";
import type { SchemaContentDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import type { Draft } from "./autosave";
import { canonicalBody, differingFields, onPreconditionFailed } from "./conflict";
import { EditorScreen, type EditorScreenProps } from "./editor";
import { newBodyUploadQueue } from "./body-uploads";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";

const para = (text: string) => ({
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text }] }],
});

function itemWith(extra: Partial<SchemaContentDto> = {}): SchemaContentDto {
  return {
    id: ID,
    type: "post",
    title: "Spring notes",
    slug: "spring-notes",
    status: "draft",
    body: para("The first paragraph."),
    excerpt: "An excerpt",
    fields: {},
    seo: { title: null, description: null, canonical: null, robots: [] },
    terms: [],
    menu_order: 0,
    permalink: "/spring-notes/",
    author_id: "0190f1c0-0000-7000-8000-0000000000bb",
    author: { id: "0190f1c0-0000-7000-8000-0000000000bb", login: "me", display_name: "Me" },
    created_at: "2026-10-01T10:00:00Z",
    updated_at: "2026-10-01T10:00:00Z",
    version: 4,
    allowed: {
      edit: true,
      delete: false,
      publish: true,
      restore: false,
      schedule: false,
      submit: false,
      trash: true,
      trash_undoable: true,
      unpublish: false,
    },
    ...extra,
  };
}

function draftOf(item: SchemaContentDto, extra: Partial<Draft> = {}): Draft {
  return {
    base_version: item.version,
    title: item.title,
    excerpt: item.excerpt,
    body: item.body,
    fields: item.fields,
    slug: item.slug,
    parent_id: item.parent_id ?? null,
    menu_order: item.menu_order,
    terms: [...item.terms],
    ...extra,
  };
}

function preconditionFailed(current: unknown): ApiError {
  return new ApiError({
    type: "https://ferritecms.dev/problems/precondition-failed",
    title: "Precondition failed",
    status: 412,
    detail: "the item changed",
    current,
  });
}

/** A `412` response carrying `current`, as the API sends it. */
function conflictResponse(current: SchemaContentDto): Response {
  return new Response(
    JSON.stringify({
      type: "https://ferritecms.dev/problems/precondition-failed",
      title: "Precondition failed",
      status: 412,
      detail: "the item changed since version 4",
      current,
    }),
    { status: 412, headers: { "Content-Type": "application/problem+json" } },
  );
}

function openScreen(item: SchemaContentDto, extra: Partial<EditorScreenProps> = {}) {
  const qc = renderWithRouter(
    createElement(
      TooltipProvider,
      null,
      createElement(EditorScreen, {
        uploads: newBodyUploadQueue(),
        type: "post",
        typeLabel: "Posts",
        item,
        ...extra,
      }),
    ),
  );
  return screen.findByRole("textbox", { name: "Body" }).then(() => qc);
}

const SLOT = `/api/v1/content/post/${ID}/autosave`;

/** Records the autosave slot's requests (`PUT` bodies, `DELETE`s). */
function slotRoute(log: { method: string; body: unknown }[]) {
  return async (_url: URL, req: Request) => {
    const body: unknown = req.method === "PUT" ? await req.json() : null;
    log.push({ method: req.method, body });
    if (req.method === "DELETE") return new Response(null, { status: 204 });
    return json({ ...(body as object), saved_at: "2026-10-01T10:06:00Z" });
  };
}

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

describe("conflict", () => {
  test("differing_fields_are_detected", () => {
    const theirs = itemWith({ version: 5, title: "Their title", terms: ["t1"] });
    // The same draft: nothing differs.
    expect(differingFields(theirs, draftOf(theirs))).toEqual([]);
    // Members the draft leaves unchanged (`null`) never count.
    expect(
      differingFields(
        theirs,
        draftOf(theirs, { slug: null, terms: null, menu_order: null, excerpt: null }),
      ),
    ).toEqual([]);
    const mine = draftOf(theirs, {
      title: "My title",
      excerpt: "My excerpt",
      body: para("My paragraph."),
      slug: "my-slug",
      terms: ["t1", "t2"],
      parent_id: "0190f1c0-0000-7000-8000-0000000000cc",
      menu_order: 3,
    });
    expect(differingFields(theirs, mine)).toEqual([
      "title",
      "excerpt",
      "body",
      "slug",
      "terms",
      "parent",
      "menu_order",
    ]);
    // Term order is part of the draft.
    expect(
      differingFields(
        itemWith({ terms: ["t1", "t2"] }),
        draftOf(itemWith(), { terms: ["t2", "t1"] }),
      ),
    ).toEqual(["terms"]);
  });

  test("fields_and_seo_differences_are_listed", () => {
    const theirs = itemWith({
      fields: { price: 3, subtitle: null },
      seo: { title: "Theirs", description: null, canonical: null, robots: [] },
    });
    // No SEO in the draft (an old slot) and the same fields: nothing differs.
    expect(differingFields(theirs, draftOf(theirs, { seo: null }))).toEqual([]);
    // A blank SEO member is "not set"; key order does not matter.
    expect(
      differingFields(
        theirs,
        draftOf(theirs, {
          fields: { subtitle: null, price: 3 },
          seo: { title: "Theirs", description: "  " },
        }),
      ),
    ).toEqual([]);
    // `null` and absent are different values.
    expect(differingFields(theirs, draftOf(theirs, { fields: { price: 3 } }))).toEqual(["fields"]);
    expect(
      differingFields(theirs, draftOf(theirs, { seo: { title: "Mine", description: null } })),
    ).toEqual(["seo"]);
  });

  test("bodies_compare_without_editor_defaults", () => {
    // TipTap fills in null attribute defaults the stored copy may not have (F32).
    const stored = {
      type: "doc",
      content: [{ type: "orderedList", attrs: { start: 1 }, content: [] }],
    };
    const edited = {
      type: "doc",
      content: [{ type: "orderedList", attrs: { start: 1, type: null }, content: [] }],
    };
    expect(canonicalBody(stored)).toBe(canonicalBody(edited));
    expect(canonicalBody({ type: "paragraph", attrs: {} })).toBe(
      canonicalBody({ type: "paragraph" }),
    );
    expect(canonicalBody(para("a"))).not.toBe(canonicalBody(para("b")));
  });

  test("the_problem_carries_theirs", () => {
    const theirs = itemWith({ version: 5, title: "Theirs" });
    const state = onPreconditionFailed(preconditionFailed(theirs), draftOf(itemWith()));
    expect(state?.theirs.version).toBe(5);
    expect(state?.fields).toEqual(["title"]);
    // Without a usable `current` there is nothing to compare.
    expect(onPreconditionFailed(preconditionFailed(undefined), draftOf(theirs))).toBeNull();
    expect(onPreconditionFailed(preconditionFailed({ id: 1 }), draftOf(theirs))).toBeNull();
  });

  test("keep_mine_sends_the_new_version", async () => {
    const item = itemWith();
    const theirs = itemWith({
      version: 5,
      title: "Their title",
      body: para("Their paragraph."),
      updated_at: "2026-10-01T10:05:00Z",
    });
    const patches: { ifMatch: string | null; body: Record<string, unknown> }[] = [];
    stubRoutes({
      [`/api/v1/content/post/${ID}`]: async (_url, req) => {
        patches.push({ ifMatch: req.headers.get("If-Match"), body: (await req.json()) as never });
        if (patches.length === 1) return conflictResponse(theirs);
        return json({ ...theirs, title: "My title", version: 6 });
      },
    });
    const qc = await openScreen(item);
    const user = userEvent.setup();
    const title = screen.getByRole("textbox", { name: "Title" });
    await user.clear(title);
    await user.type(title, "My title");
    await user.click(screen.getByRole("button", { name: "Save" }));

    const dialog = await screen.findByRole("dialog", {
      name: "This item changed while you were editing",
    });
    // Both versions of what differs, side by side; my text is still in the editor.
    const titleDiff = within(dialog).getByRole("region", { name: "Title" });
    expect(titleDiff.querySelector("[data-side=theirs]")).toHaveTextContent("Their title");
    expect(titleDiff.querySelector("[data-side=mine]")).toHaveTextContent("My title");
    expect(within(dialog).getByRole("region", { name: "Body" })).toHaveTextContent(
      "Their paragraph.",
    );
    expect(within(dialog).getByRole("region", { name: "Body" })).toHaveTextContent(
      "The first paragraph.",
    );
    // (The dialog is modal: the rest of the screen is hidden from the accessibility tree.)
    expect(document.querySelector("[data-slot=editor-title]")).toHaveValue("My title");
    // Not closed by Esc.
    await user.keyboard("{Escape}");
    expect(screen.getByRole("dialog")).toBeInTheDocument();

    await user.click(within(dialog).getByRole("button", { name: "Keep mine" }));
    await waitFor(() => expect(patches).toHaveLength(2));
    // Saved over theirs: their version as the `If-Match`, my members in the body.
    expect(patches[0]?.ifMatch).toBe('"4"');
    expect(patches[1]?.ifMatch).toBe('"5"');
    expect(patches[1]?.body).toMatchObject({
      title: "My title",
      body: para("The first paragraph."),
    });
    await waitFor(() => expect(screen.queryByRole("dialog")).toBeNull());
    // Children show this item as their parent: the reference follows the save (carry c).
    expect(qc.getQueryData(qk.content.parentRef("post", ID))).toEqual({
      title: "My title",
      slug: "spring-notes",
    });
  });

  test("take_theirs_keeps_mine_in_the_slot", async () => {
    const item = itemWith();
    const theirs = itemWith({ version: 5, title: "Their title" });
    const puts: unknown[] = [];
    stubRoutes({
      [`/api/v1/content/post/${ID}`]: () => conflictResponse(theirs),
      [`/api/v1/content/post/${ID}/autosave`]: async (_url, req) => {
        puts.push(await req.json());
        return json({ ...(puts.at(-1) as object), saved_at: "2026-10-01T10:06:00Z" });
      },
    });
    const reopen = vi.fn();
    renderWithRouter(
      createElement(
        TooltipProvider,
        null,
        createElement(EditorScreen, {
          uploads: newBodyUploadQueue(),
          type: "post",
          typeLabel: "Posts",
          item,
          onReopen: reopen,
        }),
      ),
    );
    await screen.findByRole("textbox", { name: "Body" });
    const user = userEvent.setup();
    const title = screen.getByRole("textbox", { name: "Title" });
    await user.clear(title);
    await user.type(title, "My title");
    await user.click(screen.getByRole("button", { name: "Save" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(within(dialog).getByRole("button", { name: "Take theirs" }));
    await waitFor(() => expect(reopen).toHaveBeenCalledTimes(1));
    // Mine went to the slot first, made from the version it was based on.
    expect(puts).toHaveLength(1);
    expect(puts[0]).toMatchObject({ base_version: 4, title: "My title" });
    expect(reopen).toHaveBeenCalledWith({
      item: theirs,
      kept: expect.objectContaining({ title: "My title", base_version: 4 }) as unknown,
    });
  });

  test("a_transition_waiting_for_the_refused_save_does_not_run", async () => {
    const item = itemWith();
    const theirs = itemWith({ version: 5, title: "Their title" });
    let answer: (r: Response) => void = () => undefined;
    const seen = stubRoutes({
      // What the publish dialog reads for its summary.
      "/api/v1/taxonomies": () => json({ items: [] }),
      "/api/v1/meta": () => json({ api: "v1", version: "0", site_url: "http://localhost" }),
      [`/api/v1/content/post/${ID}`]: () =>
        new Promise<Response>((resolve) => {
          answer = resolve;
        }),
    });
    await openScreen(item);
    const user = userEvent.setup();
    const title = screen.getByRole("textbox", { name: "Title" });
    await user.type(title, "!");
    // The phone publish bar (jsdom is narrow): Publish… opens the dialog.
    const bar = screen.getByRole("region", { name: "Publishing" });
    await user.click(within(bar).getByRole("button", { name: "Publish…" }));
    const publish = await screen.findByRole("dialog");
    // A save starts (the shortcut), then Publish now joins it while it is in flight.
    await user.keyboard("{Control>}s{/Control}");
    await user.click(within(publish).getByRole("button", { name: "Publish now" }));
    await act(async () => {
      answer(conflictResponse(theirs));
      await Promise.resolve();
    });
    const dialog = await screen.findByRole("dialog", {
      name: "This item changed while you were editing",
    });
    // It says what did not happen, and nothing was published.
    expect(dialog.querySelector("[data-slot=conflict-paused]")).toHaveTextContent(
      "Publishing was paused so nothing goes live by surprise.",
    );
    expect(seen.map((u) => u.pathname).filter((p) => p.startsWith("/api/v1/content/"))).toEqual([
      `/api/v1/content/post/${ID}`,
    ]);
  });

  test("a_transition_412_without_edits_moves_to_their_version", async () => {
    // Nothing typed here: someone else saved, then Publish meets the newer version. There
    // is no "mine" to keep, so no dialog: the screen moves to theirs and says so.
    const item = itemWith();
    const theirs = itemWith({ version: 5, title: "Their title" });
    const seen = stubRoutes({
      "/api/v1/taxonomies": () => json({ items: [] }),
      "/api/v1/meta": () => json({ api: "v1", version: "0", site_url: "http://localhost" }),
      [`/api/v1/content/post/${ID}/publish`]: () => conflictResponse(theirs),
    });
    const reopen = vi.fn();
    await openScreen(item, { onReopen: reopen });
    const user = userEvent.setup();
    const bar = screen.getByRole("region", { name: "Publishing" });
    await user.click(within(bar).getByRole("button", { name: "Publish…" }));
    const publish = await screen.findByRole("dialog");
    await user.click(within(publish).getByRole("button", { name: "Publish now" }));
    await waitFor(() => expect(reopen).toHaveBeenCalledWith({ item: theirs }));
    expect(document.querySelector("[data-slot=conflict-dialog]")).toBeNull();
    expect(seen.filter((u) => u.pathname.startsWith("/api/v1/content/")).length).toBe(1);
  });

  test("the_kept_copy_is_not_autosaved_over_until_discarded", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const item = itemWith({ version: 5, title: "Their title" });
    const mine = draftOf(itemWith({ title: "My kept title" }));
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({ [SLOT]: slotRoute(slot) });
    await openScreen(item, { kept: mine });
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    expect(screen.getByText("Your version is kept as an autosave.")).toBeInTheDocument();
    await user.type(screen.getByRole("textbox", { name: "Title" }), " edited");
    await act(() => vi.advanceTimersByTimeAsync(25_000));
    // The kept copy stays the slot's: nothing was written over it, and the tab would ask.
    expect(slot).toEqual([]);
    expect(document.querySelector("[data-slot=editor-bar] [data-slot=save-state]")).toHaveAttribute(
      "data-state",
      "dirty",
    );
    expect(screen.getByText(/not autosaved while your version is kept/)).toBeInTheDocument();

    // Discard the kept copy: the slot is deleted, then the new edits go there.
    await user.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(slot.map((r) => r.method)).toEqual(["DELETE"]));
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(slot.map((r) => r.method)).toEqual(["DELETE", "PUT"]));
    expect(slot[1]?.body).toMatchObject({ title: "Their title edited", base_version: 5 });
  });

  test("restoring_the_kept_copy_over_new_edits_asks_and_flushes", async () => {
    const item = itemWith({ version: 5, title: "Their title" });
    const mine = draftOf(itemWith({ title: "My kept title" }));
    stubRoutes({});
    const reopen = vi.fn();
    await openScreen(item, { kept: mine, onReopen: reopen });
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Title" }), "!");
    await user.click(screen.getByRole("button", { name: "Restore mine" }));
    // Asked first: the edits since would be replaced.
    expect(reopen).not.toHaveBeenCalled();
    expect(screen.getByText(/replaces the edits you made since/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Replace my edits" }));
    expect(reopen).toHaveBeenCalledWith({
      item,
      restore: expect.objectContaining({ draft: mine, flush: true }) as unknown,
    });
  });

  test("discarding_a_restored_draft_sticks", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const item = itemWith();
    const restored = draftOf(item, { title: "Restored title" });
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({
      [SLOT]: slotRoute(slot),
      [`/api/v1/content/post/${ID}`]: () => json(item),
    });
    const reopen = vi.fn();
    await openScreen(item, {
      restore: { draft: restored, at: "2026-10-01T10:04:00Z" },
      onReopen: reopen,
    });
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    // An edit after the restore: the autosaver has something pending.
    await user.type(screen.getByRole("textbox", { name: "Title" }), "!");
    await user.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() =>
      expect(reopen).toHaveBeenCalledWith({
        item,
        discarded: expect.objectContaining({ title: "Restored title!" }) as unknown,
      }),
    );
    // The screen goes (the route reopens it); nothing writes the discarded draft back.
    cleanup();
    await act(() => vi.advanceTimersByTimeAsync(30_000));
    expect(slot.map((r) => r.method)).toEqual(["DELETE"]);
  });

  test("the_discard_undo_goes_with_the_reopened_screen", async () => {
    // M2b-1 follow-up: the Discard's Undo toast outlived the screen, so an Undo after
    // leaving wrote the discarded draft back into the item's slot. The reopened screen
    // shows it now and dismisses it when it goes (and at its first edit).
    const item = itemWith();
    const restored = draftOf(item, { title: "Restored title" });
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({
      [SLOT]: slotRoute(slot),
      [`/api/v1/content/post/${ID}`]: () => json(item),
    });
    const success = vi.spyOn(toast, "success");
    const dismiss = vi.spyOn(toast, "dismiss");
    const reopen = vi.fn();
    await openScreen(item, {
      restore: { draft: restored, at: "2026-10-01T10:04:00Z" },
      onReopen: reopen,
    });
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(reopen).toHaveBeenCalledOnce());
    const next = reopen.mock.calls[0]?.[0] as { item: unknown; discarded: Draft };
    expect(next).toMatchObject({ item, discarded: { title: "Restored title" } });
    expect(success).not.toHaveBeenCalled();
    cleanup();

    // The reopened screen offers the Undo...
    await openScreen(item, { discarded: next.discarded, onReopen: reopen });
    await waitFor(() =>
      expect(success).toHaveBeenCalledWith(
        "Restored changes discarded",
        expect.objectContaining({
          id: "editor-discarded",
          action: expect.objectContaining({ label: "Undo" }) as unknown,
        }),
      ),
    );
    expect(dismiss).not.toHaveBeenCalledWith("editor-discarded");
    // ...and takes it away when it goes.
    cleanup();
    expect(dismiss).toHaveBeenCalledWith("editor-discarded");
  });

  test("the_discard_undo_goes_at_the_first_edit", async () => {
    const item = itemWith();
    stubRoutes({ [SLOT]: slotRoute([]) });
    const dismiss = vi.spyOn(toast, "dismiss");
    await openScreen(item, { discarded: draftOf(item, { title: "Old" }), onReopen: vi.fn() });
    expect(dismiss).not.toHaveBeenCalledWith("editor-discarded");
    await userEvent.setup().type(screen.getByRole("textbox", { name: "Title" }), "!");
    await waitFor(() => expect(dismiss).toHaveBeenCalledWith("editor-discarded"));
  });

  test("a_discard_whose_reload_fails_reopens_on_the_copy_it_has", async () => {
    // The slot is gone; the item could not be read again: the screen still reopens (on
    // the copy it has), so no screen goes on with a stopped autosaver.
    const item = itemWith();
    const restored = draftOf(item, { title: "Restored title" });
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({
      [SLOT]: slotRoute(slot),
      [`/api/v1/content/post/${ID}`]: () =>
        new Response(
          JSON.stringify({
            type: "https://ferritecms.dev/problems/busy",
            title: "Busy",
            status: 503,
            detail: "try again",
          }),
          { status: 503, headers: { "Content-Type": "application/problem+json" } },
        ),
    });
    const reopen = vi.fn();
    await openScreen(item, {
      restore: { draft: restored, at: "2026-10-01T10:04:00Z" },
      onReopen: reopen,
    });
    await userEvent.setup().click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(reopen).toHaveBeenCalledOnce());
    expect(slot.map((r) => r.method)).toEqual(["DELETE"]);
    expect(reopen.mock.calls[0]?.[0]).toMatchObject({ item: { id: ID, version: 4 } });
  });

  test("the_kept_discard_undo_goes_with_the_screen", async () => {
    const item = itemWith({ version: 5, title: "Their title" });
    const mine = draftOf(itemWith({ title: "My kept title" }));
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({ [SLOT]: slotRoute(slot) });
    const success = vi.spyOn(toast, "success");
    const dismiss = vi.spyOn(toast, "dismiss");
    await openScreen(item, { kept: mine });
    await userEvent.setup().click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() =>
      expect(success).toHaveBeenCalledWith(
        "Kept version discarded",
        expect.objectContaining({ id: "editor-kept" }),
      ),
    );
    expect(slot.map((r) => r.method)).toEqual(["DELETE"]);
    expect(dismiss).not.toHaveBeenCalledWith("editor-kept");
    cleanup();
    expect(dismiss).toHaveBeenCalledWith("editor-kept");
  });

  test("a_failed_discard_keeps_the_autosaver_and_the_draft", async () => {
    vi.useFakeTimers({ shouldAdvanceTime: true });
    const item = itemWith();
    const restored = draftOf(item, { title: "Restored title" });
    const slot: { method: string; body: unknown }[] = [];
    const record = slotRoute(slot);
    stubRoutes({
      // The DELETE fails (the server is busy); PUTs work.
      [SLOT]: (url, req) =>
        req.method === "DELETE"
          ? (slot.push({ method: "DELETE", body: null }),
            new Response(
              JSON.stringify({
                type: "https://ferritecms.dev/problems/busy",
                title: "Busy",
                status: 503,
                detail: "try again",
              }),
              { status: 503, headers: { "Content-Type": "application/problem+json" } },
            ))
          : record(url, req),
    });
    const reopen = vi.fn();
    await openScreen(item, {
      restore: { draft: restored, at: "2026-10-01T10:04:00Z" },
      onReopen: reopen,
    });
    const user = userEvent.setup({ advanceTimers: (ms) => vi.advanceTimersByTime(ms) });
    await user.click(screen.getByRole("button", { name: "Discard" }));
    await waitFor(() => expect(slot.map((r) => r.method)).toEqual(["DELETE"]));
    // Nothing was discarded: the screen stays, with the banner and the restored text.
    expect(reopen).not.toHaveBeenCalled();
    await waitFor(() =>
      expect(document.querySelector("[data-slot=restore-banner]")).not.toBeNull(),
    );
    expect(screen.getByRole("textbox", { name: "Title" })).toHaveValue("Restored title");
    // And the autosaver still works: an edit is written to the slot at the next tick.
    await user.type(screen.getByRole("textbox", { name: "Title" }), "!");
    await act(() => vi.advanceTimersByTimeAsync(11_000));
    await waitFor(() => expect(slot.map((r) => r.method)).toEqual(["DELETE", "PUT"]));
    expect(slot[1]?.body).toMatchObject({ title: "Restored title!" });
  });

  test("leaving_with_discard_keeps_the_kept_copy", async () => {
    const item = itemWith({ version: 5, title: "Their title" });
    const mine = draftOf(itemWith({ title: "My kept title" }));
    const slot: { method: string; body: unknown }[] = [];
    stubRoutes({ [SLOT]: slotRoute(slot) });
    renderWithRouter(
      createElement(
        TooltipProvider,
        null,
        createElement(
          Fragment,
          null,
          createElement(EditorScreen, {
            uploads: newBodyUploadQueue(),
            type: "post",
            typeLabel: "Posts",
            item,
            kept: mine,
          }),
          createElement(Link, { to: "/elsewhere" }, "Elsewhere"),
        ),
      ),
    );
    await screen.findByRole("textbox", { name: "Body" });
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Title" }), " edited");
    await user.click(screen.getByRole("link", { name: "Elsewhere" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Your own version stays kept as an autosave.");
    await user.click(within(dialog).getByRole("button", { name: "Discard changes" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    // The slot (the kept copy) is untouched: no DELETE, no PUT.
    expect(slot).toEqual([]);
  });

  test("an_explicit_save_puts_the_kept_copy_back", async () => {
    const item = itemWith({ version: 5, title: "Their title" });
    const mine = draftOf(itemWith({ title: "My kept title" }));
    const order: string[] = [];
    const puts: unknown[] = [];
    stubRoutes({
      [`/api/v1/content/post/${ID}`]: () => {
        order.push("PATCH");
        return json({ ...item, title: "Their title!", version: 6 });
      },
      [SLOT]: async (_url, req) => {
        order.push(req.method);
        puts.push(await req.json());
        return json({ ...(puts.at(-1) as object), saved_at: "2026-10-01T10:06:00Z" });
      },
    });
    await openScreen(item, { kept: mine });
    const user = userEvent.setup();
    await user.type(screen.getByRole("textbox", { name: "Title" }), "!");
    await user.click(screen.getByRole("button", { name: "Save" }));
    // The PATCH cleared the slot on the server; the kept copy goes straight back.
    await waitFor(() => expect(order).toEqual(["PATCH", "PUT"]));
    expect(puts[0]).toMatchObject({ title: "My kept title", base_version: 4 });
    expect(screen.getByText("Your version is kept as an autosave.")).toBeInTheDocument();
  });

  describe("a_kept_copy_only_on_this_page", () => {
    beforeAll(() => {
      globalThis.ResizeObserver ??= class {
        observe() {}
        unobserve() {}
        disconnect() {}
      } as unknown as typeof ResizeObserver;
    });

    /**
     * Take-theirs state, an edit, Save: the PATCH works but the kept copy's re-PUT fails
     * (503), so it is only on this page. Then a navigation away. `later` answers the next
     * slot PUTs (the retries).
     */
    async function setup(later: "fail" | "ok") {
      const item = itemWith({ version: 5, title: "Their title" });
      const mine = draftOf(itemWith({ title: "My kept title" }));
      const puts: unknown[] = [];
      let first = true;
      stubRoutes({
        [`/api/v1/content/post/${ID}`]: () => json({ ...item, title: "Their title!", version: 6 }),
        [SLOT]: async (_url, req) => {
          puts.push(await req.json());
          const fail = first || later === "fail";
          first = false;
          return fail
            ? new Response(
                JSON.stringify({
                  type: "https://ferritecms.dev/problems/busy",
                  title: "Busy",
                  status: 503,
                  detail: "try again",
                }),
                { status: 503, headers: { "Content-Type": "application/problem+json" } },
              )
            : json({ ...(puts.at(-1) as object), saved_at: "2026-10-01T10:06:00Z" });
        },
      });
      function Where() {
        return createElement("output", { "data-testid": "where" }, useLocation().pathname);
      }
      renderWithRouter(
        createElement(
          TooltipProvider,
          null,
          createElement(
            Fragment,
            null,
            createElement(EditorScreen, {
              uploads: newBodyUploadQueue(),
              type: "post",
              typeLabel: "Posts",
              item,
              kept: mine,
            }),
            createElement(Link, { to: "/elsewhere" }, "Elsewhere"),
            createElement(Where),
          ),
        ),
      );
      await screen.findByRole("textbox", { name: "Body" });
      const user = userEvent.setup();
      await user.type(screen.getByRole("textbox", { name: "Title" }), "!");
      await user.click(screen.getByRole("button", { name: "Save" }));
      await screen.findByText(/only on this page/, undefined, { timeout: 3000 });
      expect(puts).toHaveLength(1);
      // Clean now, but the copy is not stored: leaving asks, and says so truthfully.
      await user.click(screen.getByRole("link", { name: "Elsewhere" }));
      const dialog = await screen.findByRole("alertdialog");
      expect(dialog).toHaveTextContent(
        "could not be stored as an autosave and is only on this page",
      );
      expect(dialog).not.toHaveTextContent("stays kept as an autosave");
      return { user, dialog, puts, mine };
    }

    const where = () => screen.getByTestId("where").textContent;

    for (const button of ["Save and leave", "Discard changes"]) {
      test(`${button}_stays_when_the_copy_still_cannot_be_stored`, async () => {
        const { user, dialog, puts, mine } = await setup("fail");
        await user.click(within(dialog).getByRole("button", { name: button }));
        await within(dialog).findByText(/still could not be stored/);
        // Retried, and nobody left.
        expect(puts).toHaveLength(2);
        expect(puts[1]).toMatchObject({ title: mine.title });
        expect(where()).not.toBe("/elsewhere");
        expect(screen.getByRole("alertdialog")).toBeInTheDocument();
      });

      test(`${button}_after_new_edits_puts_the_copy_once`, async () => {
        // M2b-1 follow-up: Save and leave ran `storeKept` from the render before the
        // save, so after the save's own re-PUT it wrote the copy a second time.
        const { user, dialog, puts, mine } = await setup("ok");
        await user.click(within(dialog).getByRole("button", { name: "Stay" }));
        await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
        await user.type(screen.getByRole("textbox", { name: "Title" }), "?");
        await user.click(screen.getByRole("link", { name: "Elsewhere" }));
        const again = await screen.findByRole("alertdialog");
        await user.click(within(again).getByRole("button", { name: button }));
        await waitFor(() => expect(where()).toBe("/elsewhere"));
        expect(puts).toHaveLength(2);
        expect(puts[1]).toMatchObject({ title: mine.title, base_version: 4 });
      });

      test(`${button}_stores_the_copy_then_leaves`, async () => {
        const { user, dialog, puts, mine } = await setup("ok");
        await user.click(within(dialog).getByRole("button", { name: button }));
        await waitFor(() => expect(where()).toBe("/elsewhere"));
        expect(puts).toHaveLength(2);
        expect(puts[1]).toMatchObject({ title: mine.title, base_version: 4 });
      });
    }
  });
});
