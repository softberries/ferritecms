// Add to release… / Save as change (design 15 §11.2): pick an open release or name a new
// one, the transition, and whether the editor's working draft becomes the staged change.
// An item already in the release has its entry replaced.

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, expect, test, vi } from "vitest";
import type { StagedChange } from "@/api/releases";
import type { SchemaContentDto } from "@/api/schema";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { AddToReleaseDialog } from "./add-to-release-dialog";
import { ITEM_A, REL, release } from "./fixtures";

afterEach(() => {
  vi.unstubAllGlobals();
});

const ITEM = {
  id: ITEM_A,
  type: "page",
  title: "New page",
  status: "draft",
  allowed: { publish: true, unpublish: false, edit: true, stage: false },
} as unknown as SchemaContentDto;

const STAGED: StagedChange = { title: "New page", body: { type: "doc", content: [] } };

function routes() {
  const calls: { method: string; path: string; body: unknown; ifMatch: string | null }[] = [];
  const other = release({
    id: "0190f1c0-0000-7000-8000-0000000000c2",
    title: "Autumn",
    entries: [],
    version: 2,
  });
  stubRoutes({
    "/api/v1/releases": async (url, req) => {
      if (req.method === "POST") {
        const body = (await req.json()) as unknown;
        calls.push({ method: "POST", path: url.pathname, body, ifMatch: null });
        return json(
          { ...other, id: "0190f1c0-0000-7000-8000-0000000000c3", title: "Fresh", version: 1 },
          201,
        );
      }
      expect(url.searchParams.get("status")).toBe("open");
      return json({ items: [release(), other] });
    },
    [`/api/v1/releases/${REL}`]: () => json(release()),
    [`/api/v1/releases/0190f1c0-0000-7000-8000-0000000000c3`]: () =>
      json({ ...other, id: "0190f1c0-0000-7000-8000-0000000000c3", title: "Fresh", version: 1 }),
    [`/api/v1/releases/${REL}/items/${ITEM_A}`]: async (url, req) => {
      calls.push({
        method: req.method,
        path: url.pathname,
        body: await req.json(),
        ifMatch: req.headers.get("If-Match"),
      });
      return json(release({ version: 5 }));
    },
    [`/api/v1/releases/0190f1c0-0000-7000-8000-0000000000c3/items`]: async (url, req) => {
      calls.push({
        method: req.method,
        path: url.pathname,
        body: await req.json(),
        ifMatch: req.headers.get("If-Match"),
      });
      return json(release({ version: 2 }));
    },
  });
  return calls;
}

test("an_item_already_in_the_release_has_its_entry_replaced_with_my_changes", async () => {
  const calls = routes();
  const onAdded = vi.fn();
  renderWithRouter(
    <AddToReleaseDialog
      open
      onOpenChange={() => {}}
      item={ITEM}
      staged={() => STAGED}
      dirty
      onAdded={onAdded}
    />,
  );
  const dialog = await screen.findByRole("dialog", { name: "Add to release" });
  await userEvent.click(await within(dialog).findByRole("radio", { name: "Spring launch" }));
  await userEvent.click(within(dialog).getByRole("radio", { name: "Publish it" }));
  expect(within(dialog).getByRole("checkbox", { name: "Include my changes" })).toBeChecked();
  await userEvent.click(within(dialog).getByRole("button", { name: "Add to release" }));
  await waitFor(() => expect(onAdded).toHaveBeenCalled());
  // The fixture release holds ITEM_A already: PUT, with its version.
  expect(calls).toEqual([
    {
      method: "PUT",
      path: `/api/v1/releases/${REL}/items/${ITEM_A}`,
      ifMatch: '"4"',
      body: { transition: "publish", changes: STAGED },
    },
  ]);
  expect(onAdded.mock.calls[0]?.[1]).toBe(true);
});

test("a_new_release_is_created_then_the_item_added", async () => {
  const calls = routes();
  renderWithRouter(
    <AddToReleaseDialog
      open
      onOpenChange={() => {}}
      item={ITEM}
      staged={() => STAGED}
      dirty={false}
      onAdded={() => {}}
    />,
  );
  const dialog = await screen.findByRole("dialog");
  await userEvent.click(await within(dialog).findByRole("radio", { name: "New release" }));
  await userEvent.type(within(dialog).getByLabelText("Title of the new release"), "Fresh");
  await userEvent.click(within(dialog).getByRole("radio", { name: "Publish it" }));
  // Nothing unsaved: the changes are not included by default.
  expect(within(dialog).getByRole("checkbox", { name: "Include my changes" })).not.toBeChecked();
  await userEvent.click(within(dialog).getByRole("button", { name: "Add to release" }));
  await waitFor(() => expect(calls).toHaveLength(2));
  expect(calls[0]).toMatchObject({
    method: "POST",
    path: "/api/v1/releases",
    body: { title: "Fresh" },
  });
  expect(calls[1]).toMatchObject({
    method: "POST",
    ifMatch: '"1"',
    body: { item_id: ITEM_A, transition: "publish" },
  });
  expect(calls[1]?.body).not.toHaveProperty("changes");
});

test("save_as_change_always_stages_the_draft", async () => {
  routes();
  renderWithRouter(
    <AddToReleaseDialog
      open
      onOpenChange={() => {}}
      item={{ ...ITEM, status: "published" }}
      staged={() => STAGED}
      dirty
      asChange
      onAdded={() => {}}
    />,
  );
  const dialog = await screen.findByRole("dialog", { name: "Save as change" });
  expect(within(dialog).queryByRole("checkbox", { name: "Include my changes" })).toBeNull();
  expect(within(dialog).getByText(/staged, not saved to the live item/)).toBeInTheDocument();
  // A published item can be unpublished, not published.
  expect(within(dialog).queryByRole("radio", { name: "Publish it" })).toBeNull();
  expect(within(dialog).getByRole("radio", { name: "Unpublish it" })).toBeInTheDocument();
});
