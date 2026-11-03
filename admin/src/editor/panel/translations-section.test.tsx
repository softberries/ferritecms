import { QueryClient } from "@tanstack/react-query";
import { render, screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import { qk } from "@/api/keys";
import type { SchemaContentSummaryDto } from "@/api/schema";
import { LocaleChips } from "@/content/list-cells";
import { siteLocales } from "@/lib/locales";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import type { EditorDocument } from "../editor-state";
import { TranslationsSection } from "./translations-section";

const ID = "0190f1c0-0000-7000-8000-0000000000aa";
const DE = "0190f1c0-0000-7000-8000-0000000000dd";

const META = {
  api: "v1",
  site_url: "https://example.test",
  version: "0",
  default_locale: "en",
  locales: [
    { tag: "en", label: "English", prefix: null },
    { tag: "de", label: "Deutsch", prefix: "de" },
    { tag: "fr", label: "Français", prefix: "fr" },
  ],
};

function client(meta: unknown = META): QueryClient {
  const qc = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  qc.setQueryData(qk.meta, meta);
  return qc;
}

function doc(dirty = false): EditorDocument {
  return {
    item: {
      id: ID,
      type: "post",
      locale: "en",
      translation_group: ID,
      translations: [
        { id: DE, locale: "de", status: "draft", title: "Hallo", permalink: "/de/hallo/" },
      ],
      allowed: { edit: true },
    },
    readOnly: null,
    dirty,
  } as unknown as EditorDocument;
}

test("translations_section_lists_and_translates_missing", async () => {
  const posted: unknown[] = [];
  stubRoutes({
    "/api/v1/meta": () => json(META),
    [`/api/v1/content/post/${ID}/translations`]: async (_url, req) => {
      posted.push(await req.json());
      return json({ item: { id: "new-fr" }, notes: ["parent_untranslated"] }, 201);
    },
  });
  renderWithRouter(<TranslationsSection doc={doc()} type="post" />, client());
  expect(await screen.findByRole("link", { name: "Hallo" })).toBeInTheDocument();
  expect(screen.getByText("Language: English (en)")).toBeInTheDocument();
  // German exists: only French can be made.
  expect(screen.queryByRole("button", { name: "Translate into Deutsch" })).toBeNull();
  await userEvent.click(screen.getByRole("button", { name: "Translate into Français" }));
  await waitFor(() => expect(posted).toEqual([{ locale: "fr" }]));
});

test("unsaved_changes_block_translate", async () => {
  stubRoutes({ "/api/v1/meta": () => json(META) });
  renderWithRouter(<TranslationsSection doc={doc(true)} type="post" />, client());
  expect(await screen.findByRole("button", { name: "Translate into Français" })).toBeDisabled();
});

test("single_locale_renders_no_locale_controls", async () => {
  const one = { ...META, locales: [{ tag: "en", label: "English", prefix: null }] };
  stubRoutes({ "/api/v1/meta": () => json(one) });
  renderWithRouter(<TranslationsSection doc={doc()} type="post" />, client(one));
  await waitFor(() => expect(screen.queryByText("Translations")).toBeNull());
});

test("chips_show_status_and_missing", () => {
  const row = {
    locale: "en",
    translations: [{ locale: "de", status: "published" }],
  } as unknown as SchemaContentSummaryDto;
  render(<LocaleChips item={row} locales={siteLocales(META.locales, "en")} />);
  expect(screen.getByLabelText("Deutsch: Published")).toBeInTheDocument();
  expect(screen.getByLabelText("Not translated into Français")).toBeInTheDocument();
});
