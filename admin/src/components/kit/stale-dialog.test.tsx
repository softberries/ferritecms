import { render, screen } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test, vi } from "vitest";
import { StaleDialog } from "./stale-dialog";

test("stale_dialog_offers_reload_overwrite_cancel", async () => {
  const onChoose = vi.fn();
  render(<StaleDialog open resource="Menu main" onChoose={onChoose} />);
  expect(screen.getByText("Changed by someone else")).toBeInTheDocument();
  expect(screen.getByText(/Menu main/)).toBeInTheDocument();
  await userEvent.click(screen.getByRole("button", { name: "Overwrite" }));
  await userEvent.click(screen.getByRole("button", { name: "Reload" }));
  expect(onChoose.mock.calls).toEqual([["overwrite"], ["reload"]]);
});
