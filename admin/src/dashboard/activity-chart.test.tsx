import { screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ActivityChart } from "./activity-chart";

const EMPTY = { items: [], next_cursor: null };

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("activity-chart", () => {
  it("empty_activity_state", async () => {
    const seen = stubRoutes({
      "/api/v1/content/post": () => json(EMPTY),
      "/api/v1/content/page": () => json(EMPTY),
    });
    renderWithRouter(<ActivityChart canWrite />);

    const card = await screen.findByRole("region", { name: "Publishing activity" });
    expect(
      await within(card).findByRole("heading", {
        name: "No posts published in the last 30 days",
      }),
    ).toBeVisible();
    expect(within(card).getByRole("link", { name: "Write a post" })).toHaveAttribute(
      "href",
      "/admin/content/post/new",
    );
    // No chart, no legend and no table for nothing.
    expect(card.querySelector("[data-slot=chart]")).toBeNull();
    expect(within(card).queryByRole("table")).toBeNull();
    // Both series were asked for, published only, newest first.
    expect(seen.map((u) => u.pathname).sort()).toEqual([
      "/api/v1/content/page",
      "/api/v1/content/post",
    ]);
    for (const url of seen) {
      expect(url.searchParams.get("status")).toBe("published");
      expect(url.searchParams.get("order")).toBe("published");
    }
  });

  it("offers no write action to a role that cannot write", async () => {
    stubRoutes({
      "/api/v1/content/post": () => json(EMPTY),
      "/api/v1/content/page": () => json(EMPTY),
    });
    renderWithRouter(<ActivityChart canWrite={false} />);
    expect(
      await screen.findByRole("heading", { name: "No posts published in the last 30 days" }),
    ).toBeVisible();
    expect(screen.queryByRole("link", { name: "Write a post" })).toBeNull();
  });
});
