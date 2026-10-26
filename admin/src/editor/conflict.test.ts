// The 412 conflict (07 §1, Review Focus 3): which members differ between the server's
// newer copy and the working draft, Keep mine saving over it with the newer version as
// the `If-Match`, Take theirs keeping mine in the autosave slot first, and a transition
// that waited for the refused save not running behind the user's back (Task 15 carry).

import { act, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { createElement } from "react";
import { afterEach, describe, expect, test, vi } from "vitest";
import { ApiError } from "@/api/problem";
import type { SchemaContentDto } from "@/api/schema";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import type { Draft } from "./autosave";
import { canonicalBody, differingFields, onPreconditionFailed } from "./conflict";
import { EditorScreen } from "./editor";

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

function openScreen(item: SchemaContentDto) {
  renderWithRouter(
    createElement(
      TooltipProvider,
      null,
      createElement(EditorScreen, { type: "post", typeLabel: "Posts", item }),
    ),
  );
  return screen.findByRole("textbox", { name: "Body" });
}

afterEach(() => {
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
    await openScreen(item);
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
        createElement(EditorScreen, { type: "post", typeLabel: "Posts", item, onReopen: reopen }),
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
});
