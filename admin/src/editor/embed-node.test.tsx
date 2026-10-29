// The embed node view (P11) and the image caption: an embed shows its provider and URL,
// never a live player (the admin CSP has no `frame-src`), and its URL is changed in an
// inline form that takes only YouTube and Vimeo video links; an image's caption is a
// plain-text field under it.

import { EditorContent, useEditor, type Editor } from "@tiptap/react";
import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { describe, expect, test } from "vitest";
import { editorExtensions } from "./schema";

function mount(content: unknown, editable = true): Editor {
  const holder: { editor: Editor | null } = { editor: null };
  function Harness() {
    const editor = useEditor({
      extensions: editorExtensions({
        placeholder: "Write",
        resolveMedia: () => Promise.resolve("/media/x.png"),
      }),
      content: content as object,
      editable,
      immediatelyRender: true,
      shouldRerenderOnTransaction: false,
      injectCSS: false,
    });
    holder.editor = editor;
    return <EditorContent editor={editor} />;
  }
  render(<Harness />);
  if (!holder.editor) throw new Error("no editor");
  return holder.editor;
}

const wire = (editor: Editor) =>
  JSON.parse(JSON.stringify(editor.getJSON())) as { content: { attrs?: object }[] };

const YT = "https://www.youtube.com/watch?v=dQw4w9WgXcQ";
const doc = (...content: object[]) => ({ type: "doc", content });
const embed = (provider: string | null, url: string) => ({
  type: "embed",
  attrs: { provider, url },
});

describe("embed node", () => {
  test("shows_provider_and_url_without_a_player", async () => {
    mount(doc(embed("youtube", YT), embed("vimeo", "https://vimeo.com/76979871")));
    // React node views render asynchronously, one after another.
    const views = await waitFor(() => {
      const found = document.querySelectorAll<HTMLElement>("[data-slot=editor-embed]");
      expect(found).toHaveLength(2);
      return found;
    });
    expect(within(views[0]!).getByText("YouTube video")).toBeVisible();
    expect(within(views[0]!).getByText(YT)).toBeVisible();
    expect(within(views[1]!).getByText("Vimeo video")).toBeVisible();
    expect(document.querySelector("iframe")).toBeNull();
  });

  test("an_unplayable_url_says_the_site_links_it", () => {
    mount(doc(embed("youtube", "https://example.com/v")));
    const view = document.querySelector<HTMLElement>("[data-slot=editor-embed]")!;
    expect(within(view).getByText("Video link")).toBeVisible();
    expect(within(view).getByText("The site shows this URL as a link.")).toBeVisible();
  });

  test("change_url_takes_only_video_links", async () => {
    const editor = mount(doc(embed("youtube", YT)));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Change URL" }));
    const field = screen.getByRole("textbox", { name: "Video URL" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveValue(YT);

    await user.clear(field);
    await user.type(field, "https://example.com/v{Enter}");
    expect(field).toHaveAttribute("aria-invalid", "true");
    expect(screen.getByText("Use a YouTube or Vimeo video link.")).toBeVisible();
    expect(wire(editor).content[0]!.attrs).toEqual({ provider: "youtube", url: YT });

    await user.clear(field);
    await user.type(field, "https://player.vimeo.com/video/76979871");
    await user.click(screen.getByRole("button", { name: "Embed" }));
    expect(screen.queryByRole("textbox", { name: "Video URL" })).toBeNull();
    expect(wire(editor).content[0]!.attrs).toEqual({
      provider: "vimeo",
      url: "https://vimeo.com/76979871",
    });
  });

  test("escape_leaves_the_url_as_it_was", async () => {
    const editor = mount(doc(embed("youtube", YT)));
    const user = userEvent.setup();
    await user.click(screen.getByRole("button", { name: "Change URL" }));
    await user.type(screen.getByRole("textbox", { name: "Video URL" }), "x{Escape}");
    expect(screen.queryByRole("textbox", { name: "Video URL" })).toBeNull();
    expect(wire(editor).content[0]!.attrs).toEqual({ provider: "youtube", url: YT });
  });

  test("insert_opens_the_form_and_cancel_removes_the_empty_embed", async () => {
    const editor = mount(doc({ type: "paragraph", content: [{ type: "text", text: "A" }] }));
    act(() => {
      editor.chain().focus("end").insertEmbed().run();
    });
    const field = await screen.findByRole("textbox", { name: "Video URL" });
    await waitFor(() => expect(field).toHaveFocus());
    expect(field).toHaveValue("");
    await userEvent.setup().click(screen.getByRole("button", { name: "Cancel" }));
    expect(JSON.stringify(wire(editor))).not.toContain("embed");
  });

  test("insert_then_apply_makes_a_canonical_embed", async () => {
    const editor = mount(doc({ type: "paragraph", content: [{ type: "text", text: "A" }] }));
    act(() => {
      editor.chain().focus("end").insertEmbed().run();
    });
    const field = await screen.findByRole("textbox", { name: "Video URL" });
    await waitFor(() => expect(field).toHaveFocus());
    await userEvent.setup().type(field, "https://youtu.be/dQw4w9WgXcQ{Enter}");
    const embeds = wire(editor).content.filter((n) => (n as { type?: string }).type === "embed");
    expect(embeds).toEqual([{ type: "embed", attrs: { provider: "youtube", url: YT } }]);
  });

  test("read_only_has_no_change_button", () => {
    mount(doc(embed("youtube", YT)), false);
    expect(screen.getByText("YouTube video")).toBeVisible();
    expect(screen.queryByRole("button", { name: "Change URL" })).toBeNull();
  });
});

describe("image caption", () => {
  const image = (attrs: object) => doc({ type: "image", attrs: { src: "/media/a.png", ...attrs } });

  test("typing_sets_the_caption_and_clearing_removes_it", async () => {
    const editor = mount(image({ alt: "A" }));
    const user = userEvent.setup();
    const field = screen.getByRole("textbox", { name: "Caption" });
    expect(field).toHaveValue("");
    await user.type(field, "At dawn");
    expect(wire(editor).content[0]!.attrs).toMatchObject({ caption: "At dawn" });
    await user.clear(field);
    expect(wire(editor).content[0]!.attrs).not.toHaveProperty("caption");
  });

  test("a_stored_caption_is_shown", () => {
    mount(image({ caption: "Stored" }));
    expect(screen.getByRole("textbox", { name: "Caption" })).toHaveValue("Stored");
  });

  test("read_only_shows_the_caption_as_text", () => {
    mount(image({ caption: "Stored" }), false);
    expect(screen.queryByRole("textbox", { name: "Caption" })).toBeNull();
    const caption = document.querySelector("figcaption");
    expect(caption).toHaveTextContent("Stored");
  });

  test("read_only_without_a_caption_has_none", () => {
    mount(image({}), false);
    expect(document.querySelector("figcaption")).toBeNull();
  });
});
