import { screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { SettingsDto } from "@/api/settings";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ReadingGroup } from "./reading";

const PAGE_ID = "44444444-4444-4444-8444-444444444444";

const settings = (patch: SettingsDto = {}): SettingsDto => ({
  "reading.front_page": null,
  "reading.per_page": null,
  "reading.discourage_indexing": null,
  ...patch,
});

test("reading_front_page_named_by_refs", async () => {
  const seen = stubRoutes({
    "/api/v1/content/refs": () =>
      json({
        items: [
          { id: PAGE_ID, title: "Welcome home", status: "draft", type: "page", permalink: null },
        ],
      }),
  });
  renderWithRouter(<ReadingGroup settings={settings({ "reading.front_page": PAGE_ID })} />);
  // The stored id is named by `GET /content/refs`.
  expect(await screen.findByText("Welcome home")).toBeInTheDocument();
  const refs = seen.find((u) => u.pathname === "/api/v1/content/refs");
  expect(refs?.searchParams.get("ids")).toBe(PAGE_ID);
  expect(screen.getByRole("radio", { name: /A page/ })).toBeChecked();
  // A page that is not public yet: `/` keeps the post listing until it is.
  expect(screen.getByText(/not published/i)).toBeInTheDocument();
});

test("front_page_gone_is_said", async () => {
  stubRoutes({ "/api/v1/content/refs": () => json({ items: [] }) });
  renderWithRouter(<ReadingGroup settings={settings({ "reading.front_page": PAGE_ID })} />);
  expect(await screen.findByText(/no longer exists/i)).toBeInTheDocument();
});

test("per_page_limits_are_checked_before_the_save", async () => {
  const seen = stubRoutes({});
  renderWithRouter(<ReadingGroup settings={settings({ "reading.per_page": 10 })} />);
  const input = await screen.findByRole("textbox", { name: "Posts per page" });
  await userEvent.clear(input);
  await userEvent.type(input, "101");
  expect(await screen.findByText("Enter a whole number from 1 to 100.")).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(seen).toEqual([]);
  // Choosing a page without picking one is refused too.
  await userEvent.clear(input);
  await userEvent.type(input, "12");
  await userEvent.click(screen.getByRole("radio", { name: /A page/ }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  expect(await screen.findByText("Pick a page.")).toBeInTheDocument();
  expect(seen).toEqual([]);
});

test("front_page_name_failure_offers_retry", async () => {
  let calls = 0;
  stubRoutes({
    "/api/v1/content/refs": () => {
      calls += 1;
      return calls === 1
        ? json({ title: "Server error", status: 500 }, 500)
        : json({
            items: [
              {
                id: PAGE_ID,
                title: "Welcome home",
                status: "published",
                type: "page",
                permalink: "/",
              },
            ],
          });
    },
  });
  renderWithRouter(<ReadingGroup settings={settings({ "reading.front_page": PAGE_ID })} />);
  // A failed lookup is not a deleted page.
  expect(await screen.findByText(/could not be loaded/i)).toBeInTheDocument();
  expect(screen.queryByText(/no longer exists/i)).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Retry" }));
  expect(await screen.findByText("Welcome home")).toBeInTheDocument();
});
