// A `rich_text` field follows its value (M2b-2 final review, parked): a value that
// changes from outside (another row moved under it, a draft put back) is shown, while
// the field's own edits coming back as its value do not reset it.

import { act, render, screen, waitFor } from "@testing-library/react";
import type { Editor } from "@tiptap/react";
import { useState } from "react";
import { describe, expect, test, vi } from "vitest";
import type { ControlIds } from "./fields-context";
import { RichTextField } from "./rich-text-field";

const ids: ControlIds = {
  id: "f-blurb",
  labelId: "f-blurb-label",
  describedBy: "f-blurb-hint",
  invalid: false,
};

const doc = (text: string) => ({
  type: "doc",
  content: [{ type: "paragraph", content: [{ type: "text", text }] }],
});

const box = () => screen.getByRole("textbox");

describe("rich_text field", () => {
  test("a_value_changed_from_outside_is_shown", async () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <RichTextField ids={ids} value={doc("First")} onChange={onChange} disabled={false} />,
    );
    await waitFor(() => expect(box()).toHaveTextContent("First"));
    rerender(
      <RichTextField ids={ids} value={doc("Second")} onChange={onChange} disabled={false} />,
    );
    await waitFor(() => expect(box()).toHaveTextContent("Second"));
    rerender(<RichTextField ids={ids} value={null} onChange={onChange} disabled={false} />);
    await waitFor(() => expect(box()).toHaveTextContent(""));
    // Shown, not edited: nothing is reported back.
    expect(onChange).not.toHaveBeenCalled();
  });

  test("its_own_edits_do_not_reset_it", async () => {
    let latest: unknown = null;
    function Holder() {
      const [value, setValue] = useState<unknown>(doc("Start"));
      latest = value;
      return <RichTextField ids={ids} value={value} onChange={setValue} disabled={false} />;
    }
    render(<Holder />);
    await waitFor(() => expect(box()).toHaveTextContent("Start"));
    // (jsdom cannot type into ProseMirror: the edit goes through the editor TipTap hangs
    // on its element.)
    const editor = (box() as unknown as { editor: Editor }).editor;
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, " more");
    });
    await waitFor(() => expect(box()).toHaveTextContent("Start more"));
    expect(latest).toEqual(doc("Start more"));
  });

  test("a_late_echo_of_its_own_edit_does_not_drop_a_newer_one", async () => {
    // Typing faster than the form renders: the value of the first keystroke comes back
    // after the second was made.
    const onChange = vi.fn();
    const { rerender } = render(
      <RichTextField ids={ids} value={doc("Start")} onChange={onChange} disabled={false} />,
    );
    await waitFor(() => expect(box()).toHaveTextContent("Start"));
    const editor = (box() as unknown as { editor: Editor }).editor;
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, " a");
    });
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, "b");
    });
    const [first] = onChange.mock.calls[0] as [unknown];
    rerender(<RichTextField ids={ids} value={first} onChange={onChange} disabled={false} />);
    // Another keystroke before the second value comes back.
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, "c");
    });
    for (const [value] of onChange.mock.calls.slice(1) as [unknown][]) {
      rerender(<RichTextField ids={ids} value={value} onChange={onChange} disabled={false} />);
    }
    await waitFor(() => expect(box()).toHaveTextContent("Start abc"));
    expect(onChange).toHaveBeenLastCalledWith(doc("Start abc"));
  });

  test("an_earlier_own_document_coming_back_after_an_outside_value_is_shown", async () => {
    // Its own edit A, then a value from outside (B), then A again (a move back, a draft
    // put back): A is shown, not ignored as an echo.
    const onChange = vi.fn();
    const { rerender } = render(
      <RichTextField ids={ids} value={doc("Start")} onChange={onChange} disabled={false} />,
    );
    await waitFor(() => expect(box()).toHaveTextContent("Start"));
    const editor = (box() as unknown as { editor: Editor }).editor;
    act(() => {
      editor.commands.insertContentAt(editor.state.doc.content.size - 1, " mine");
    });
    const [own] = onChange.mock.calls[0] as [unknown];
    rerender(<RichTextField ids={ids} value={own} onChange={onChange} disabled={false} />);
    rerender(
      <RichTextField ids={ids} value={doc("Outside")} onChange={onChange} disabled={false} />,
    );
    await waitFor(() => expect(box()).toHaveTextContent("Outside"));
    rerender(<RichTextField ids={ids} value={own} onChange={onChange} disabled={false} />);
    await waitFor(() => expect(box()).toHaveTextContent("Start mine"));
  });
});
