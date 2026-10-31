// Touch targets (11 §4, §8): on a coarse pointer every button is at least 44 px, the
// small icon sizes included. jsdom has no layout, so the class is what is checked; the
// E2E `styleguide_touch_targets` measures the boxes in a touch browser.

import { cleanup, render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { Button } from "./button";
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from "./dialog";
import { InputGroup, InputGroupAddon, InputGroupButton } from "./input-group";
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from "./sheet";

describe("touch_targets", () => {
  test("dialog_and_sheet_titles_keep_clear_of_the_44px_close_button", () => {
    // Dialog: the close button sits at right 8..52 px, the header's text at 16 + 40.
    // Sheet: right 12..56 px, the header's text at 60.
    render(
      <>
        <DialogHeader data-testid="dialog-header" />
        <SheetHeader data-testid="sheet-header" />
      </>,
    );
    expect(screen.getByTestId("dialog-header")).toHaveClass("pointer-coarse:pr-10");
    expect(screen.getByTestId("sheet-header")).toHaveClass("pointer-coarse:pr-15");
  });

  test("close_buttons_centre_on_the_title_line_on_touch", () => {
    // Dialog: 16 px padding + half a 14 px title line = 23 px; a 44 px button at 1 px.
    // Sheet: `--sheet-title-center` (28 px by default) − 22 px.
    render(
      <Dialog open>
        <DialogContent>
          <DialogTitle>Dialog</DialogTitle>
          <DialogDescription>Body</DialogDescription>
        </DialogContent>
      </Dialog>,
    );
    const dialogClose = screen.getByRole("button", { name: "Close" });
    expect(dialogClose).toHaveClass("top-2", "pointer-coarse:top-px");
    cleanup();
    render(
      <Sheet open>
        <SheetContent>
          <SheetTitle>Sheet</SheetTitle>
          <SheetDescription>Body</SheetDescription>
        </SheetContent>
      </Sheet>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveClass(
      "top-3",
      "pointer-coarse:top-[calc(var(--sheet-title-center,28px)-22px)]",
    );
  });

  test.each(["icon-xs", "icon-sm"] as const)("button_%s_is_44px_under_a_coarse_pointer", (size) => {
    render(
      <Button size={size} aria-label="Close">
        x
      </Button>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveClass("pointer-coarse:size-11");
  });

  // Inside a field (itself 44 px tall on touch screens, 1 px border) the button is 44 px
  // wide and fills the inner height; a pseudo-element covers the border rows, so the hit
  // area is 44 × 44 and stays inside the field.
  test.each(["icon-xs", "icon-sm"] as const)(
    "input_group_button_%s_hits_44px_under_a_coarse_pointer",
    (size) => {
      render(
        <InputGroup>
          <InputGroupAddon align="inline-end">
            <InputGroupButton size={size} aria-label="Clear">
              x
            </InputGroupButton>
          </InputGroupAddon>
        </InputGroup>,
      );
      expect(screen.getByRole("button", { name: "Clear" })).toHaveClass(
        "relative",
        "pointer-coarse:w-11",
        "pointer-coarse:h-[calc(var(--control-h)-2px)]",
        "pointer-coarse:after:absolute",
        "pointer-coarse:after:-inset-x-px",
        "pointer-coarse:after:-inset-y-0.5",
      );
    },
  );
});
