// Reviews (M6-3, design 15 §6, §11.4): the decision dialog asks for nothing but takes a
// comment; the queue's tabs follow the role and its rows open the editor or the release;
// the editor's Review section offers decisions to editors and Withdraw to the submitter;
// the dashboard card counts what waits and lists the oldest three.

import { QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, expect, test, vi } from "vitest";
import { qk } from "@/api/keys";
import type { MeDto } from "@/auth/session";
import { TooltipProvider } from "@/components/ui/tooltip";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { DecisionDialog } from "./decision-dialog";
import { AUTHOR_ID, EDITOR_ID, ITEM, REVIEW, me, review } from "./fixtures";
import { ReviewQueue } from "./review-queue";
import { ReviewDetail } from "./review-section";
import { ReviewsCard } from "@/dashboard/reviews-card";

beforeAll(() => {
  const proto = Element.prototype as unknown as Record<string, unknown>;
  proto.scrollIntoView ??= () => {};
  proto.hasPointerCapture ??= () => false;
  proto.releasePointerCapture ??= () => {};
});

afterEach(() => {
  vi.unstubAllGlobals();
});

function client(who: MeDto) {
  const c = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  c.setQueryData(qk.me, who);
  return c;
}

test("the_decision_dialog_takes_an_optional_comment", async () => {
  const onConfirm = vi.fn();
  const { rerender } = render(
    <DecisionDialog
      decision="approve"
      title="Spring notes"
      open
      busy={false}
      onOpenChange={() => {}}
      onConfirm={onConfirm}
    />,
  );
  const dialog = screen.getByRole("alertdialog", { name: "Approve “Spring notes”?" });
  await userEvent.click(within(dialog).getByRole("button", { name: "Approve" }));
  expect(onConfirm).toHaveBeenLastCalledWith(undefined);
  rerender(
    <DecisionDialog
      decision="request_changes"
      title="Spring notes"
      open
      busy={false}
      onOpenChange={() => {}}
      onConfirm={onConfirm}
    />,
  );
  const changes = screen.getByRole("alertdialog", { name: "Request changes to “Spring notes”?" });
  await userEvent.type(within(changes).getByLabelText("Comment"), "  Fix the dates  ");
  await userEvent.click(within(changes).getByRole("button", { name: "Request changes" }));
  expect(onConfirm).toHaveBeenLastCalledWith("Fix the dates");
});

test("queue_tabs_follow_the_role_and_rows_open_their_target", async () => {
  stubRoutes({
    "/api/v1/reviews": () =>
      json({
        assigned_to_me: 1,
        items: [
          review(),
          review({
            id: "0190f1c0-0000-7000-8000-0000000000d2",
            target_kind: "entry",
            release_id: "0190f1c0-0000-7000-8000-0000000000c1",
            item_title: "About us",
            item_type: "page",
          }),
        ],
      }),
  });
  renderWithRouter(<ReviewQueue tab="open" />, client(me("editor")));
  const nav = await screen.findByRole("navigation", { name: "Review queues" });
  expect(
    within(nav)
      .getAllByRole("link")
      .map((l) => l.textContent),
  ).toEqual(["Assigned to me", "All open", "Submitted by me"]);
  const table = await screen.findByRole("table", { name: "Reviews" });
  await within(table).findByText("Spring notes");
  expect(within(table).getByText("About us")).toBeInTheDocument();
  expect(within(table).getByText("Change in a release")).toBeInTheDocument();
});

test("an_author_has_no_all_open_tab", async () => {
  stubRoutes({ "/api/v1/reviews": () => json({ assigned_to_me: 0, items: [] }) });
  renderWithRouter(<ReviewQueue tab="mine" />, client(me("author")));
  const nav = await screen.findByRole("navigation", { name: "Review queues" });
  expect(
    within(nav)
      .getAllByRole("link")
      .map((l) => l.textContent),
  ).toEqual(["Assigned to me", "Submitted by me"]);
});

test("an_editor_decides_and_the_submitter_withdraws", async () => {
  const calls: { path: string; body: unknown; ifMatch: string | null }[] = [];
  stubRoutes({
    [`/api/v1/reviews/${REVIEW}`]: () =>
      json(
        review({
          decisions: [
            {
              step: 0,
              decision: "approve",
              at: "2026-11-03T10:00:00Z",
              user_id: AUTHOR_ID,
              comment: "Looks good",
            },
          ],
          steps: [
            { key: "copy", label: "Copy edit", approvers: { roles: ["editor"] }, required: 2 },
            { key: "legal", label: "Legal", approvers: { roles: ["admin"] }, required: 1 },
          ],
        }),
      ),
    [`/api/v1/reviews/${REVIEW}/decisions`]: async (url, req) => {
      calls.push({
        path: url.pathname,
        body: await req.json(),
        ifMatch: req.headers.get("If-Match"),
      });
      return json(review({ step: 1 }));
    },
  });
  const onChanged = vi.fn();
  renderWithRouter(
    <TooltipProvider>
      <ReviewDetail id={REVIEW} title="Spring notes" onChanged={onChanged} />
    </TooltipProvider>,
    client(me("editor")),
  );
  const section = await screen.findByRole("list", { name: "Steps" });
  const steps = within(section).getAllByRole("listitem");
  expect(steps[0]).toHaveTextContent("Copy edit");
  expect(steps[0]).toHaveTextContent("1 of 2 approvals");
  expect(steps[1]).toHaveTextContent("Legal");
  expect(screen.getByText("Looks good")).toBeInTheDocument();
  expect(screen.getByText("Please check the dates")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Approve…" }));
  const dialog = await screen.findByRole("alertdialog");
  await userEvent.click(within(dialog).getByRole("button", { name: "Approve" }));
  await waitFor(() =>
    expect(calls).toEqual([
      {
        path: `/api/v1/reviews/${REVIEW}/decisions`,
        body: { decision: "approve" },
        ifMatch: '"2"',
      },
    ]),
  );
  await waitFor(() => expect(onChanged).toHaveBeenCalled());
});

test("the_submitter_sees_withdraw_not_decisions", async () => {
  stubRoutes({ [`/api/v1/reviews/${REVIEW}`]: () => json(review()) });
  renderWithRouter(
    <TooltipProvider>
      <ReviewDetail id={REVIEW} title="Spring notes" onChanged={() => {}} />
    </TooltipProvider>,
    client(me("author")),
  );
  await screen.findByRole("list", { name: "Steps" });
  expect(screen.getByRole("button", { name: "Withdraw" })).toBeInTheDocument();
  expect(screen.queryByRole("button", { name: "Approve…" })).toBeNull();
});

test("the_dashboard_card_counts_and_lists_the_oldest_three", async () => {
  const items = [1, 2, 3, 4].map((n) =>
    review({ id: `0190f1c0-0000-7000-8000-00000000000${n}`, item_title: `Item ${n}` }),
  );
  stubRoutes({
    "/api/v1/reviews": (url) => {
      expect(url.searchParams.get("assigned")).toBe("me");
      return json({ assigned_to_me: 4, items });
    },
  });
  renderWithRouter(<ReviewsCard />, client(me("editor")));
  const card = await screen.findByRole("region", { name: "Waiting for your review" });
  await within(card).findByText("4 reviews wait for you");
  const links = within(card)
    .getAllByRole("link")
    .filter((l) => /Item \d/.test(l.textContent ?? ""));
  expect(links.map((l) => l.textContent)).toEqual([
    expect.stringContaining("Item 1"),
    expect.stringContaining("Item 2"),
    expect.stringContaining("Item 3"),
  ]);
  expect(links[0]).toHaveAttribute("href", `/admin/content/post/${ITEM}`);
});

test("the_submitter_assigns_reviewers_from_the_steps_approvers", async () => {
  const puts: unknown[] = [];
  const seen = stubRoutes({
    [`/api/v1/reviews/${REVIEW}`]: () => json(review()),
    "/api/v1/workflows/post/approvers": () =>
      json({
        items: [
          { id: EDITOR_ID, display_name: "Eve Editor" },
          { id: AUTHOR_ID, display_name: "The submitter" },
        ],
      }),
    [`/api/v1/reviews/${REVIEW}/assignees`]: async (_url, req) => {
      puts.push(await req.json());
      return json({ items: [{ step: 0, user_id: EDITOR_ID }] });
    },
  });
  // An author has no `manage_users`: the picker comes from the approvers endpoint.
  renderWithRouter(
    <TooltipProvider>
      <ReviewDetail id={REVIEW} title="Spring notes" onChanged={() => {}} />
    </TooltipProvider>,
    client(me("author")),
  );
  await userEvent.click(await screen.findByRole("button", { name: "Assign…" }));
  const dialog = await screen.findByRole("dialog", { name: "Assign reviewers" });
  await userEvent.click(await within(dialog).findByLabelText("Eve Editor"));
  expect(within(dialog).queryByLabelText("The submitter")).toBeNull();
  await userEvent.click(within(dialog).getByRole("button", { name: "Save" }));
  await waitFor(() => expect(puts).toEqual([{ step: 0, users: [EDITOR_ID] }]));
  const asked = seen.find((u) => u.pathname === "/api/v1/workflows/post/approvers");
  expect(asked?.searchParams.get("step")).toBe("0");
});
