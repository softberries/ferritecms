import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import { ProgressBar } from "./progress-bar";

test("progress_bar_reports_a_rounded_percent_and_scales_its_fill", () => {
  render(<ProgressBar value={0.256} label="Upload progress" />);
  const bar = screen.getByRole("progressbar", { name: "Upload progress" });
  expect(bar).toHaveAttribute("aria-valuemin", "0");
  expect(bar).toHaveAttribute("aria-valuemax", "100");
  expect(bar).toHaveAttribute("aria-valuenow", "26");
  expect(bar).toHaveClass("h-1");
  expect(bar.firstElementChild).toHaveStyle({ transform: "scaleX(0.256)" });
});

test("progress_bar_takes_extra_classes", () => {
  render(<ProgressBar value={1} label="Done" className="h-1.5" />);
  const bar = screen.getByRole("progressbar", { name: "Done" });
  expect(bar).toHaveClass("h-1.5");
  expect(bar).not.toHaveClass("h-1");
  expect(bar).toHaveAttribute("aria-valuenow", "100");
});
