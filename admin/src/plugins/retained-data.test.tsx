import { screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { RetainedData } from "./retained-data";

const ROW = {
  name: "old-seo",
  deleted_at: "2026-10-20T10:00:00Z",
  purge_after: "2026-11-19T10:00:00Z",
};

test("erase_now_of_data_already_gone_refreshes_the_list", async () => {
  let listed = [ROW];
  stubRoutes({
    "/api/v1/plugins/retained": () => json({ items: listed }),
    // Erased meanwhile (another admin, or the 30-day job).
    "/api/v1/plugins/old-seo/purge": () => {
      listed = [];
      return new Response(
        JSON.stringify({
          type: "/api/v1/problems/not-found",
          title: "Not found",
          status: 404,
          detail: "no data of that name is retained",
        }),
        { status: 404, headers: { "Content-Type": "application/problem+json" } },
      );
    },
  });
  renderWithRouter(<RetainedData />);
  const section = await screen.findByRole("region", { name: "Retained data" });
  await userEvent.click(
    within(section).getByRole("button", { name: "Erase the data of old-seo now" }),
  );
  const confirm = await screen.findByRole("alertdialog", { name: "Erase the data of old-seo?" });
  await userEvent.click(within(confirm).getByRole("button", { name: "Erase" }));
  // No error: the dialog closes, the list is read again and the row is gone.
  await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
  await waitFor(() => expect(screen.queryByText("old-seo")).toBeNull());
  expect(screen.queryByRole("alert")).toBeNull();
});
