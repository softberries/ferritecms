import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { useState } from "react";
import { describe, expect, test, vi } from "vitest";
import { type EditorDocument, type SeoDraft, canonicalAccepted, seoOfDraft } from "../editor-state";
import { SeoSection } from "./seo-section";

/** The members of the document `SeoSection` reads, around a `setMeta` spy. */
function docWith(seo: SeoDraft, fieldErrors: Record<string, string> = {}) {
  const setMeta = vi.fn();
  const clearFieldError = vi.fn();
  const doc = {
    title: "Harbour notes",
    item: { permalink: "/harbour-notes/" },
    meta: { excerpt: "", seo },
    fieldErrors,
    setMeta,
    clearFieldError,
  } as unknown as EditorDocument;
  return { doc, setMeta, clearFieldError };
}

const EMPTY: SeoDraft = { title: "", description: "", canonical: "", robots: [] };

function renderSeo(doc: EditorDocument) {
  return render(
    <SeoSection doc={doc} siteUrl="https://example.com" siteName="Example" disabled={false} />,
  );
}

describe("seo_section", () => {
  test("canonical error from seo.canonical shows under the field", () => {
    const { doc } = docWith(
      { ...EMPTY, canonical: "example.com/a" },
      { "seo.canonical": "seo.canonical must be an absolute http(s) URL" },
    );
    renderSeo(doc);
    const input = screen.getByRole("textbox", { name: "Canonical URL" });
    expect(input).toHaveAttribute("type", "url");
    expect(input).toHaveAttribute("aria-invalid", "true");
    // The server's message, as a sentence without the path, described by the input.
    expect(input).toHaveAccessibleDescription(
      expect.stringContaining("Must be an absolute http(s) URL"),
    );
    // The item's own address is the default the field shows.
    expect(input).toHaveAttribute("placeholder", "https://example.com/harbour-notes/");
  });

  test("an edited canonical clears its server error", () => {
    const { doc, setMeta, clearFieldError } = docWith(EMPTY, {
      "seo.canonical": "seo.canonical must be an absolute http(s) URL",
    });
    renderSeo(doc);
    fireEvent.change(screen.getByRole("textbox", { name: "Canonical URL" }), {
      target: { value: "https://elsewhere.example/" },
    });
    expect(clearFieldError).toHaveBeenCalledWith("seo.canonical");
    expect(setMeta).toHaveBeenCalledWith({
      seo: { ...EMPTY, canonical: "https://elsewhere.example/" },
    });
  });

  test("a canonical that is not a full address is named once the field is left", () => {
    const { doc } = docWith({ ...EMPTY, canonical: "elsewhere.example/a" });
    renderSeo(doc);
    const input = screen.getByRole("textbox", { name: "Canonical URL" });
    expect(input).not.toHaveAttribute("aria-invalid");
    fireEvent.blur(input);
    expect(input).toHaveAttribute("aria-invalid", "true");
    expect(input).toHaveAccessibleDescription(expect.stringContaining("https://"));
  });

  test("robots toggles are a labelled group and keep the directives sorted", () => {
    const { doc, setMeta } = docWith({ ...EMPTY, robots: ["nofollow"] });
    renderSeo(doc);
    const group = screen.getByRole("group", { name: "Search engine directives" });
    const switches = within(group).getAllByRole("switch");
    expect(switches).toHaveLength(5);
    expect(within(group).getByRole("switch", { name: "Do not follow links" })).toBeChecked();
    fireEvent.click(within(group).getByRole("switch", { name: "Hide from search results" }));
    expect(setMeta).toHaveBeenLastCalledWith({
      seo: { ...EMPTY, robots: ["nofollow", "noindex"] },
    });
    fireEvent.click(within(group).getByRole("switch", { name: "Do not follow links" }));
    expect(setMeta).toHaveBeenLastCalledWith({ seo: { ...EMPTY, robots: [] } });
  });

  test("reset clears all four", () => {
    const { doc, setMeta } = docWith({
      title: "Search title",
      description: "A description.",
      canonical: "https://elsewhere.example/orig/",
      robots: ["noindex", "nosnippet"],
    });
    renderSeo(doc);
    fireEvent.click(screen.getByRole("button", { name: "Reset to defaults" }));
    expect(setMeta).toHaveBeenCalledWith({ seo: EMPTY });
  });

  test("reset shows while only a directive is set", () => {
    const { doc } = docWith({ ...EMPTY, robots: ["noarchive"] });
    renderSeo(doc);
    expect(screen.getByRole("button", { name: "Reset to defaults" })).toBeVisible();
  });

  test("noindex is noted at the search preview", () => {
    const { doc } = docWith({ ...EMPTY, robots: ["noindex"] });
    renderSeo(doc);
    expect(screen.getByText("Search engines are asked not to list this page.")).toBeVisible();
  });

  test("the preview shows the canonical address once it is a full one", () => {
    const { doc } = docWith({ ...EMPTY, canonical: "https://elsewhere.example/orig/" });
    const { container } = renderSeo(doc);
    expect(container.querySelector("[data-slot=search-preview]")).toHaveTextContent(
      "elsewhere.example › orig",
    );
  });

  test("seo_max_length_counts_code_points", () => {
    // The server counts characters (code points) of the trimmed text; a native
    // `maxLength` counts UTF-16 units and cut 100 emoji off at a limit of 200.
    const { doc, setMeta } = docWith(EMPTY);
    renderSeo(doc);
    const title = screen.getByRole("textbox", { name: "SEO title" });
    const description = screen.getByRole("textbox", { name: "Meta description" });
    const canonical = screen.getByRole("textbox", { name: "Canonical URL" });
    for (const el of [title, description, canonical]) expect(el).not.toHaveAttribute("maxlength");

    const emoji = "\u{1F30A}";
    fireEvent.change(title, { target: { value: emoji.repeat(150) } });
    expect(setMeta).toHaveBeenLastCalledWith({ seo: { ...EMPTY, title: emoji.repeat(150) } });
    // An over-long paste is cut at the limit, on a code-point boundary.
    fireEvent.change(title, { target: { value: emoji.repeat(250) } });
    expect(setMeta).toHaveBeenLastCalledWith({ seo: { ...EMPTY, title: emoji.repeat(200) } });
    // The server trims edge whitespace before it counts, so that does not count here.
    fireEvent.change(title, { target: { value: `  ${"a".repeat(200)}\t ` } });
    expect(setMeta).toHaveBeenLastCalledWith({
      seo: { ...EMPTY, title: `  ${"a".repeat(200)}\t ` },
    });
    fireEvent.change(description, { target: { value: emoji.repeat(501) } });
    expect(setMeta).toHaveBeenLastCalledWith({
      seo: { ...EMPTY, description: emoji.repeat(500) },
    });
    fireEvent.change(canonical, { target: { value: `https://a.test/${"é".repeat(2100)}` } });
    const last = setMeta.mock.lastCall?.[0] as { seo: SeoDraft };
    expect([...last.seo.canonical]).toHaveLength(2048);
  });

  test("bom_is_kept_like_the_server", () => {
    // Rust's `trim()` (Unicode White_Space) keeps U+FEFF and `is_control` is Cc only, so
    // the server stores a BOM-only value and accepts a canonical that ends in one; JS
    // `trim()` and `\s` treat it as whitespace.
    const bom = "﻿";
    expect(seoOfDraft({ ...EMPTY, title: bom }).title).toBe(bom);
    expect(seoOfDraft({ ...EMPTY, description: ` ${bom} ` }).description).toBe(` ${bom} `);
    expect(canonicalAccepted(`https://a.test/${bom}`)).toBe(true);
    // Real whitespace still trims away or is refused, as on the server.
    expect(seoOfDraft({ ...EMPTY, title: "  　\n" }).title).toBeNull();
    expect(canonicalAccepted(" https://a.test/ \n")).toBe(true);
    expect(canonicalAccepted("https://a.test/a b")).toBe(false);
    expect(canonicalAccepted("https://a.test/a b")).toBe(false);
    expect(canonicalAccepted("https://a.test/a\u0085b")).toBe(false);
  });

  test("the counter's live region speaks only when a limit is crossed", () => {
    const { doc } = docWith({ ...EMPTY, title: "a".repeat(59) });
    const { rerender } = renderSeo(doc);
    const live = document.querySelector<HTMLElement>("[data-slot=seo-title-live]")!;
    // Mounted from the first render, empty, so its first words are read.
    expect(live).toHaveAttribute("role", "status");
    expect(live).toBeEmptyDOMElement();
    const again = (title: string) =>
      rerender(
        <SeoSection
          doc={docWith({ ...EMPTY, title }).doc}
          siteUrl="https://example.com"
          siteName="Example"
          disabled={false}
        />,
      );
    again("a".repeat(60));
    expect(live).toBeEmptyDOMElement();
    again("a".repeat(61));
    expect(live).toHaveTextContent("Over 60 characters: search results may cut it off");
    // Typing on past the soft limit says nothing new.
    again("a".repeat(120));
    expect(live).toHaveTextContent("Over 60 characters: search results may cut it off");
    again("a".repeat(200));
    expect(live).toHaveTextContent("200 characters, the most allowed");
    again("a".repeat(30));
    expect(live).toHaveTextContent("Within 60 characters");
    // The description has its own region.
    expect(document.querySelector("[data-slot=seo-description-live]")).toBeEmptyDOMElement();
  });

  test("typing_at_the_limit_is_refused_like_max_length", () => {
    // Full text: a typed character is refused before it lands (the caret stays put), and
    // an edit that still arrives longer is not applied, so the end is never pushed off.
    const full = "a".repeat(200);
    const { doc, setMeta } = docWith({ ...EMPTY, title: full });
    renderSeo(doc);
    const title = screen.getByRole<HTMLInputElement>("textbox", { name: "SEO title" });
    title.setSelectionRange(10, 10);
    const typed = new InputEvent("beforeinput", {
      inputType: "insertText",
      data: "b",
      bubbles: true,
      cancelable: true,
    });
    title.dispatchEvent(typed);
    expect(typed.defaultPrevented).toBe(true);
    // Replacing a selection with one character fits, so it is let through.
    title.setSelectionRange(10, 12);
    const replace = new InputEvent("beforeinput", {
      inputType: "insertText",
      data: "b",
      bubbles: true,
      cancelable: true,
    });
    title.dispatchEvent(replace);
    expect(replace.defaultPrevented).toBe(false);
    fireEvent.change(title, { target: { value: `${"a".repeat(10)}b${"a".repeat(190)}` } });
    expect(setMeta).not.toHaveBeenCalled();
  });

  /** A cancelable `beforeinput`; `paste` carries its text on a `dataTransfer`. */
  function beforeInput(inputType: string, data: string | null, paste?: string): InputEvent {
    const e = new InputEvent("beforeinput", { inputType, data, bubbles: true, cancelable: true });
    if (paste !== undefined) {
      Object.defineProperty(e, "dataTransfer", {
        value: { getData: (type: string) => (type === "text/plain" ? paste : "") },
      });
    }
    return e;
  }

  test("enter_in_a_full_description_is_refused_before_it_lands", () => {
    const { doc, setMeta } = docWith({ ...EMPTY, description: "a".repeat(500) });
    renderSeo(doc);
    const description = screen.getByRole<HTMLTextAreaElement>("textbox", {
      name: "Meta description",
    });
    description.setSelectionRange(10, 10);
    const enter = beforeInput("insertLineBreak", null);
    description.dispatchEvent(enter);
    expect(enter.defaultPrevented).toBe(true);
    // Any other insert that would overflow is refused too (a replacement, a drop).
    for (const type of ["insertParagraph", "insertReplacementText", "insertFromDrop"]) {
      const e = beforeInput(type, type === "insertReplacementText" ? "xy" : null, "xy");
      description.dispatchEvent(e);
      expect(e.defaultPrevented, type).toBe(true);
    }
    // The caret stays where it was: the value never changed.
    expect(description.selectionStart).toBe(10);
    expect(setMeta).not.toHaveBeenCalled();
  });

  test("a_paste_into_a_full_field_is_refused_and_one_that_overflows_is_cut", () => {
    const { doc, setMeta } = docWith({ ...EMPTY, title: "a".repeat(200) });
    const { rerender } = renderSeo(doc);
    const title = screen.getByRole<HTMLInputElement>("textbox", { name: "SEO title" });
    title.setSelectionRange(5, 5);
    const full = beforeInput("insertFromPaste", null, "pasted");
    title.dispatchEvent(full);
    expect(full.defaultPrevented).toBe(true);
    expect(title.selectionStart).toBe(5);
    expect(setMeta).not.toHaveBeenCalled();

    // Room for three characters: the paste lands cut to them, at the caret, and the
    // caret ends after them (as a native `maxLength` does). The draft really changes
    // here (a stateful document), so the field is not reset to the old value.
    const seen: SeoDraft[] = [];
    function Stateful() {
      const [seo, setSeo] = useState<SeoDraft>({ ...EMPTY, title: "a".repeat(197) });
      const live = {
        ...docWith(seo).doc,
        setMeta: ({ seo: next }: { seo: SeoDraft }) => {
          seen.push(next);
          setSeo(next);
        },
      } as unknown as EditorDocument;
      return (
        <SeoSection doc={live} siteUrl="https://example.com" siteName="Example" disabled={false} />
      );
    }
    rerender(<Stateful />);
    const field = screen.getByRole<HTMLInputElement>("textbox", { name: "SEO title" });
    field.setSelectionRange(5, 5);
    const cut = beforeInput("insertFromPaste", null, "\u{1F30A}bcdef");
    act(() => {
      field.dispatchEvent(cut);
    });
    expect(cut.defaultPrevented).toBe(true);
    expect(seen.at(-1)).toEqual({ ...EMPTY, title: `aaaaa\u{1F30A}bc${"a".repeat(192)}` });
    expect(field.value).toBe(`aaaaa\u{1F30A}bc${"a".repeat(192)}`);
    expect(field.selectionStart).toBe(5 + "\u{1F30A}bc".length);
  });

  test("a_huge_paste_is_cut_at_once_without_splitting_a_pair", () => {
    // Pasting a whole article into the 500-character description must not hang the tab:
    // the cut is found in a few passes, not one pass per pasted character.
    const emoji = "\u{1F30A}";
    const { doc, setMeta } = docWith({ ...EMPTY, description: "xyz" });
    renderSeo(doc);
    const description = screen.getByRole<HTMLTextAreaElement>("textbox", {
      name: "Meta description",
    });
    description.setSelectionRange(2, 2);
    // 50 000 code points; room for 497, which ends on a whole pair.
    const pasted = `${"a".repeat(496)}${emoji}${"a".repeat(49_503)}`;
    expect([...pasted]).toHaveLength(50_000);
    const paste = beforeInput("insertFromPaste", null, pasted);
    const started = performance.now();
    act(() => {
      description.dispatchEvent(paste);
    });
    const took = performance.now() - started;
    expect(paste.defaultPrevented).toBe(true);
    const kept = `xy${"a".repeat(496)}${emoji}z`;
    expect([...kept]).toHaveLength(500);
    expect(setMeta).toHaveBeenCalledTimes(1);
    const next = setMeta.mock.calls[0]?.[0] as { seo: SeoDraft };
    expect(next.seo.description).toBe(kept);
    // No lone surrogate: the cut did not split the pair.
    expect(
      /[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/.test(
        next.seo.description,
      ),
    ).toBe(false);
    // Generous, so a slow runner does not flake it; the quadratic cut took many seconds.
    expect(took).toBeLessThan(250);
  });

  test("the preview trims like the server, keeping a BOM", () => {
    const { doc } = docWith({ ...EMPTY, title: "﻿" });
    const { container } = renderSeo(doc);
    // A BOM-only title is stored and shown by the site: the preview does not fall back
    // to the item title.
    expect(container.querySelector("[data-slot=search-preview]")).not.toHaveTextContent(
      "Harbour notes",
    );
  });
});
