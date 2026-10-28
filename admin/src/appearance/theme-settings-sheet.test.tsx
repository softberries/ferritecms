import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { ThemeSettingsSheet } from "./theme-settings-sheet";

const SETTINGS = {
  schema: [{ key: "tagline", type: "text", label: "Tagline", default: "", options: [] }],
  values: { tagline: "Hello" },
};

test("closing_with_unsaved_changes_asks", async () => {
  stubRoutes({ "/api/v1/themes/blog/settings": () => json(SETTINGS) });
  const onClose = vi.fn();
  renderWithRouter(<ThemeSettingsSheet name="blog" onClose={onClose} />);
  const field = await screen.findByRole("textbox", { name: "Tagline" });
  // Nothing edited: Escape closes at once.
  await userEvent.keyboard("{Escape}");
  expect(onClose).toHaveBeenCalledTimes(1);
  onClose.mockClear();

  await userEvent.type(field, " world");
  await userEvent.keyboard("{Escape}");
  const ask = await screen.findByRole("alertdialog", { name: "Leave without saving?" });
  expect(onClose).not.toHaveBeenCalled();
  await userEvent.click(screen.getByRole("button", { name: "Stay" }));
  await waitFor(() => expect(ask).not.toBeInTheDocument());
  expect(screen.getByRole("textbox", { name: "Tagline" })).toHaveValue("Hello world");
  expect(onClose).not.toHaveBeenCalled();

  await userEvent.click(screen.getByRole("button", { name: "Close" }));
  await userEvent.click(await screen.findByRole("button", { name: "Leave and drop changes" }));
  expect(onClose).toHaveBeenCalledTimes(1);
});
