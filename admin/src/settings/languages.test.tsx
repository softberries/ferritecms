import { screen, waitFor } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { expect, test } from "vitest";
import type { SettingsDto } from "@/api/settings";
import { canonicalTag } from "@/lib/locales";
import { json, renderWithRouter, stubRoutes } from "@/test/render";
import { LanguagesGroup } from "./languages";
import { checkLocales } from "./locale-list";

const STORED: SettingsDto = {
  "site.title": null,
  "site.description": null,
  "site.locale": "en",
  "site.locales": [
    { tag: "en", label: "English" },
    { tag: "de", label: "Deutsch" },
  ],
  "site.locale_texts": {},
  "reading.per_page": null,
  "reading.front_page": null,
  "reading.discourage_indexing": null,
  "permalink.post": null,
  "permalink.page": null,
  "comments.enabled": null,
  "comments.moderation": null,
  "comments.per_page": null,
  "mcp.enabled": null,
};

function api(onPatch?: (body: SettingsDto) => Response) {
  const patches: SettingsDto[] = [];
  stubRoutes({
    "/api/v1/meta": () =>
      json({
        api: "v1",
        site_url: "https://example.test",
        version: "0",
        default_locale: "en",
        locales: [],
      }),
    "/api/v1/settings": async (_url, req) => {
      if (req.method === "PATCH") {
        const body = (await req.json()) as SettingsDto;
        patches.push(body);
        return onPatch?.(body) ?? json({ ...STORED, ...body });
      }
      return json(STORED);
    },
  });
  return patches;
}

test("tags_are_spelled_as_core_spells_them", () => {
  expect(canonicalTag("pt_br")).toBe("pt-BR");
  expect(canonicalTag("zh-hant-tw")).toBe("zh-Hant-TW");
  expect(canonicalTag("es-419")).toBe("es-419");
  expect(canonicalTag("not a tag")).toBeNull();
  expect(canonicalTag("de-DE-x")).toBeNull();
  expect(checkLocales([])).toEqual({ code: "localeList", detail: "empty" });
  expect(
    checkLocales([
      { tag: "de", label: "" },
      { tag: "DE", label: "" },
    ]),
  ).toEqual({ code: "localeList", detail: "duplicate" });
});

test("languages_validates_tags_and_adds_one", async () => {
  const patches = api();
  renderWithRouter(<LanguagesGroup settings={STORED} />);
  const tag = await screen.findByRole("textbox", { name: "Language tag" });
  await userEvent.type(tag, "not a tag");
  expect(screen.getByRole("button", { name: "Add language" })).toBeDisabled();
  await userEvent.clear(tag);
  await userEvent.type(tag, "de");
  expect(screen.getByText("The site has this language already.")).toBeInTheDocument();
  await userEvent.clear(tag);
  await userEvent.type(tag, "pt_br");
  await userEvent.type(screen.getByRole("textbox", { name: "Label" }), "Português");
  await userEvent.click(screen.getByRole("button", { name: "Add language" }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() =>
    expect(patches).toEqual([
      {
        "site.locales": [
          { tag: "en", label: "English" },
          { tag: "de", label: "Deutsch" },
          { tag: "pt-BR", label: "Português" },
        ],
      },
    ]),
  );
});

test("languages_blocks_removing_used_locale", async () => {
  const patches = api(
    () =>
      new Response(
        JSON.stringify({
          type: "https://ferritecms.dev/problems/conflict",
          title: "Conflict",
          status: 409,
          detail: "de still holds 3 items, 1 term",
        }),
        { status: 409, headers: { "Content-Type": "application/problem+json" } },
      ),
  );
  renderWithRouter(<LanguagesGroup settings={STORED} />);
  await userEvent.click(await screen.findByRole("button", { name: "Remove de" }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await waitFor(() => expect(patches).toHaveLength(1));
  expect(await screen.findByText("de still holds 3 items, 1 term")).toBeInTheDocument();
});

test("default_change_confirms", async () => {
  const patches = api();
  renderWithRouter(<LanguagesGroup settings={STORED} />);
  await userEvent.click(await screen.findByRole("button", { name: "Make default" }));
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  const dialog = await screen.findByRole("alertdialog");
  expect(dialog).toHaveTextContent("Content in en will move under its own prefix");
  await userEvent.click(screen.getByRole("button", { name: "Cancel" }));
  expect(patches).toEqual([]);
  await userEvent.click(screen.getByRole("button", { name: "Save changes" }));
  await userEvent.click(await screen.findByRole("button", { name: "Change the default" }));
  await waitFor(() =>
    expect(patches).toEqual([
      {
        "site.locales": [
          { tag: "de", label: "Deutsch" },
          { tag: "en", label: "English" },
        ],
      },
    ]),
  );
});
