import { render } from "@testing-library/react";
import { expect, test } from "vitest";
import { SearchPreview } from "./search-preview";

test("preview_title_wraps_at_words_not_letters", () => {
  // `break-all` cut every word at the line's end; only an over-long word may break.
  const { container } = render(
    <SearchPreview
      title="Harbour notes from a long winter"
      url="https://example.com/2026/10/harbour-notes/"
      description="A walk along the quay."
      siteName="Example"
    />,
  );
  const title = container.querySelector("[data-slot=search-preview-title]");
  expect(title).toHaveClass("break-words");
  expect(title).not.toHaveClass("break-all");
  expect(title).toHaveAttribute("title", "Harbour notes from a long winter");
});
