import { screen } from "@testing-library/react";
import { describe, expect, test } from "vitest";
import type { CommentDto } from "@/api/comments";
import { renderWithRouter } from "@/test/render";
import { authorLink } from "./author-link";
import { CommentRow } from "./comment-row";

const COMMENT: CommentDto = {
  id: "0190a1b2-c3d4-7e5f-8a9b-000000000001",
  author_name: "Ada",
  author_email: "ada@example.test",
  author_url: "",
  body: "Hello",
  created_at: "2026-10-25T08:00:00Z",
  ip_hash: "0123456789abcdef0123456789abcdef",
  item: {
    id: "0190a1b2-c3d4-7e5f-8a9b-0000000000aa",
    type: "post",
    title: "First post",
    permalink: "/first-post/",
    status: "published",
  },
  item_id: "0190a1b2-c3d4-7e5f-8a9b-0000000000aa",
  parent_id: null,
  status: "pending",
  user_id: null,
};

function row(patch: Partial<CommentDto> = {}) {
  renderWithRouter(<CommentRow comment={{ ...COMMENT, ...patch }} />);
}

describe("comment_row_test", () => {
  test("a javascript: URL is no link", async () => {
    row({ author_url: "javascript:alert(1)" });
    expect(await screen.findByText("javascript:alert(1)")).toBeVisible();
    for (const link of screen.queryAllByRole("link")) {
      expect(link.getAttribute("href") ?? "").not.toMatch(/^javascript:/i);
    }
    expect(authorLink("javascript:alert(1)")).toBeNull();
    expect(authorLink("JaVaScRiPt:alert(1)")).toBeNull();
    expect(authorLink("data:text/html,x")).toBeNull();
    expect(authorLink("//evil.example")).toBeNull();
    expect(authorLink("not a url")).toBeNull();
  });

  test("an http(s) URL is an outgoing link marked as user content", async () => {
    row({ author_url: "https://ada.example/blog" });
    const link = await screen.findByRole("link", { name: /ada\.example/ });
    expect(link).toHaveAttribute("href", "https://ada.example/blog");
    expect(link).toHaveAttribute("rel", "noopener noreferrer nofollow ugc");
    expect(link).toHaveAttribute("target", "_blank");
  });

  test("the body is text, never parsed as HTML", async () => {
    const body = '<img src=x onerror=alert(1)>\n<b>bold</b> & <a href="javascript:x">a</a>';
    const author = "<script>alert(2)</script>";
    renderWithRouter(
      <div data-testid="row">
        <CommentRow comment={{ ...COMMENT, body, author_name: author }} />
      </div>,
    );
    const root = await screen.findByTestId("row");
    expect(root.querySelector("img, b, script, a[href^='javascript']")).toBeNull();
    const shown = root.querySelector("[data-slot=comment-body]");
    expect(shown?.textContent).toBe(body);
    expect(screen.getByText(author)).toBeVisible();
  });

  test("e-mail, address hash, item and reply line", async () => {
    renderWithRouter(
      <CommentRow
        comment={{ ...COMMENT, parent_id: "0190a1b2-c3d4-7e5f-8a9b-000000000002" }}
        parentName="Grace"
      />,
    );
    const mail = await screen.findByRole("link", { name: "ada@example.test" });
    expect(mail).toHaveAttribute("href", "mailto:ada%40example.test");
    const hash = screen.getByText("01234567");
    expect(hash).toHaveAttribute("title", expect.stringContaining(COMMENT.ip_hash ?? ""));
    expect(screen.getByRole("link", { name: "First post" })).toHaveAttribute(
      "href",
      `/admin/content/post/${COMMENT.item.id}`,
    );
    expect(screen.getByRole("link", { name: /View/ })).toHaveAttribute("href", "/first-post/");
    expect(screen.getByText("Reply to Grace")).toBeVisible();
  });

  test("a commenter's address cannot add recipients or text to the e-mail", async () => {
    const email = "ada@example.test?bcc=eve@example.test&body=Hi";
    row({ author_email: email });
    const mail = await screen.findByRole("link", { name: email });
    const href = mail.getAttribute("href") ?? "";
    expect(href).toBe(`mailto:${encodeURIComponent(email)}`);
    // One address part: no query (`?`) and no second header (`&`).
    expect(href).not.toMatch(/[?&]/);
  });

  test("no View link without a public page, and an unknown parent still says it is a reply", async () => {
    renderWithRouter(
      <CommentRow
        comment={{
          ...COMMENT,
          item: { ...COMMENT.item, permalink: null, status: "draft" },
          parent_id: "0190a1b2-c3d4-7e5f-8a9b-000000000002",
        }}
        parentName={null}
      />,
    );
    expect(await screen.findByText("Reply to another comment")).toBeVisible();
    expect(screen.queryByRole("link", { name: /View/ })).toBeNull();
  });
});
