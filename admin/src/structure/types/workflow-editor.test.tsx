// The content type's Workflow section (M6-3, design 15 §6.1, §11.4): the preset "One
// editor approves", steps validated before saving (a label, an approver, 1–5 approvals),
// steps reordered (the menu's Move down), saved with `PUT /workflows/{type}`, and removed
// after a confirm that says open reviews are withdrawn.

import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { WorkflowEditor } from "./workflow-editor";

beforeAll(() => {
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

afterEach(() => {
  vi.unstubAllGlobals();
});

const WF = "/api/v1/workflows/post";

function routes(start: unknown) {
  const calls: { method: string; body: unknown; ifMatch: string | null }[] = [];
  let current = start;
  stubRoutes({
    [WF]: async (_u, req) => {
      if (req.method === "GET") {
        return current
          ? json(current)
          : json({ type: "about:blank", title: "Not found", status: 404 }, 404);
      }
      const text = await req.text();
      calls.push({
        method: req.method,
        body: text ? (JSON.parse(text) as unknown) : null,
        ifMatch: req.headers.get("If-Match"),
      });
      if (req.method === "DELETE") {
        current = null;
        return new Response(null, { status: 204 });
      }
      current = {
        type: "post",
        version: 1,
        updated_at: "2026-11-03T10:00:00Z",
        ...(JSON.parse(text) as object),
      };
      return json(current);
    },
    "/api/v1/users": () => json({ items: [] }),
  });
  return calls;
}

const open = () =>
  renderWithRouter(
    <TooltipProvider>
      <WorkflowEditor type="post" />
    </TooltipProvider>,
  );

test("the_preset_saves_one_editor_step", async () => {
  const calls = routes(null);
  open();
  const section = await screen.findByRole("region", { name: "Workflow" });
  await within(section).findByText(/No workflow/);
  await userEvent.click(within(section).getByRole("button", { name: "One editor approves" }));
  await userEvent.click(within(section).getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]).toEqual({
    method: "PUT",
    ifMatch: null,
    body: {
      steps: [
        {
          key: "review",
          label: "Editor approval",
          approvers: { roles: ["editor", "admin"], users: [] },
          required: 1,
        },
      ],
    },
  });
});

test("steps_are_checked_before_saving", async () => {
  const calls = routes(null);
  open();
  const section = await screen.findByRole("region", { name: "Workflow" });
  await within(section).findByText(/No workflow/);
  await userEvent.click(within(section).getByRole("button", { name: "Add step" }));
  const label = within(section).getByLabelText("Label");
  await userEvent.clear(label);
  // No label, no approver yet.
  expect(within(section).getByText(/Give the step a label/)).toBeInTheDocument();
  expect(within(section).getByText(/Choose at least one role or person/)).toBeInTheDocument();
  expect(within(section).getByRole("button", { name: "Save workflow" })).toBeDisabled();
  await userEvent.type(label, "Legal");
  await userEvent.click(within(section).getByRole("checkbox", { name: "Admins" }));
  const required = within(section).getByLabelText("Approvals needed");
  await userEvent.clear(required);
  await userEvent.type(required, "7");
  expect(within(section).getByText(/1 to 5 approvals/)).toBeInTheDocument();
  await userEvent.clear(required);
  await userEvent.type(required, "2");
  await userEvent.click(within(section).getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]?.body).toEqual({
    steps: [
      { key: "legal", label: "Legal", approvers: { roles: ["admin"], users: [] }, required: 2 },
    ],
  });
});

test("steps_reorder_and_the_workflow_is_removed_after_a_confirm", async () => {
  const calls = routes({
    type: "post",
    version: 3,
    updated_at: "2026-11-03T10:00:00Z",
    steps: [
      { key: "copy", label: "Copy edit", approvers: { roles: ["editor"], users: [] }, required: 1 },
      { key: "legal", label: "Legal", approvers: { roles: ["admin"], users: [] }, required: 1 },
    ],
  });
  open();
  const section = await screen.findByRole("region", { name: "Workflow" });
  await within(section).findByDisplayValue("Copy edit");
  await userEvent.click(within(section).getByRole("button", { name: "Actions for Copy edit" }));
  await userEvent.click(await screen.findByRole("menuitem", { name: "Move down" }));
  await userEvent.click(within(section).getByRole("button", { name: "Save workflow" }));
  await waitFor(() => expect(calls).toHaveLength(1));
  expect(calls[0]?.ifMatch).toBe('"3"');
  expect((calls[0]?.body as { steps: { key: string }[] }).steps.map((s) => s.key)).toEqual([
    "legal",
    "copy",
  ]);

  await userEvent.click(within(section).getByRole("button", { name: "Remove workflow" }));
  const confirm = await screen.findByRole("alertdialog", { name: "Remove the workflow?" });
  expect(confirm).toHaveTextContent("Open reviews of this type are withdrawn");
  await userEvent.click(within(confirm).getByRole("button", { name: "Remove workflow" }));
  await waitFor(() => expect(calls.at(-1)?.method).toBe("DELETE"));
  expect(calls.at(-1)?.ifMatch).toBe('"1"');
});
