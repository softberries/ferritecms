// Touch targets (11 §4, §8): on a coarse pointer every button is at least 44 px, the
// small icon sizes included. jsdom has no layout, so the class is what is checked; the
// E2E `styleguide_touch_targets` measures the boxes in a touch browser.

import { render, screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import { Button } from "./button";
import { DialogHeader } from "./dialog";
import { InputGroup, InputGroupAddon, InputGroupButton } from "./input-group";
import { SheetHeader } from "./sheet";

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

  test.each(["icon-xs", "icon-sm"] as const)("button_%s_is_44px_under_a_coarse_pointer", (size) => {
    render(
      <Button size={size} aria-label="Close">
        x
      </Button>,
    );
    expect(screen.getByRole("button", { name: "Close" })).toHaveClass("pointer-coarse:size-11");
  });

  // Inside a field (itself 44 px tall on touch screens) the button keeps its look; a
  // pseudo-element widens what a finger hits to 44 px, staying inside the field on the
  // right (3 px): 24 + 2 × 10 tall and 24 + 17 + 3 wide; 32 + 2 × 6 and 32 + 9 + 3.
  test.each([
    ["icon-xs", ["-inset-y-2.5", "-right-0.75", "-left-4.25"]],
    ["icon-sm", ["-inset-y-1.5", "-right-0.75", "-left-2.25"]],
  ] as const)("input_group_button_%s_hits_44px_under_a_coarse_pointer", (size, insets) => {
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
      "pointer-coarse:after:absolute",
      ...insets.map((inset) => `pointer-coarse:after:${inset}`),
    );
  });
});
