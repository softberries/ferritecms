// The comments queue under the real server (M2b-2 Task 12; 07 §3 Comments), as an
// editor: approving a held comment and seeing it on the public post; Spam with Undo;
// a bulk Trash where one comment was deleted meanwhile (the others go, the failure is
// listed); Approve and reply; the queue keys; a hostile comment shown as text (Review
// Focus 3); an author without the queue; and the review screens. Comments are made as
// guests through the public endpoint, each from its own address (`support/comments.ts`);
// every test narrows the queue to its own post (`?item=`), so other tests' comments never
// show (E2E data hygiene).

import { request as apiRequest, type Page } from "@playwright/test";
import { guestAddress, guestComment } from "./support/comments";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { uid } from "./support/uid";

const queue = (page: Page) => page.locator("[data-slot=comments-queue]");
/** The table row of the comment by `name`. */
const row = (page: Page, name: string) =>
  queue(page).locator("tr[data-row]", {
    has: page.getByRole("article", { name, exact: true }),
  });
const tab = (page: Page, name: string) =>
  queue(page).getByRole("navigation", { name: "Comment status" }).getByRole("link", { name });
const tabCount = (page: Page, name: string) => tab(page, name).locator("[data-slot=tab-count]");

/** A published post of this test, through the API as the editor. */
async function newPost(api: ContentApi, label: string): Promise<Item> {
  return api.createIn("post", `${label} ${uid("p").slice(-8)}`, "published");
}

/**
 * The queue narrowed to `post`, on `status` (pending by default). The filter chip names
 * the post once a comment on it is known (else it says "one item").
 */
async function openQueue(
  page: Page,
  post: Item,
  status?: string,
  chip: string = post.title,
): Promise<void> {
  await page.goto(`/admin/comments?item=${post.id}${status ? `&status=${status}` : ""}`);
  await expect(queue(page)).toBeVisible();
  await expect(queue(page).locator("[data-slot=item-chip]")).toContainText(chip);
}

/** Waits for the `PATCH` of one comment and returns its status code. */
function patchOf(page: Page, id?: string) {
  return page.waitForResponse(
    (r) => r.request().method() === "PATCH" && r.url().includes(`/api/v1/comments/${id ?? ""}`),
  );
}

/** The browser's own requests (the moderator's replies) come from an address of their own. */
async function ownAddress(page: Page): Promise<void> {
  await page.context().setExtraHTTPHeaders({ "X-Forwarded-For": guestAddress() });
}

test("approve_from_queue_shows_on_site", async ({ as, site, backend, expectAccessible }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Approve");
  const name = `Reader ${uid("r").slice(-6)}`;
  const body = `Lovely post ${uid("b")}`;
  const c = await guestComment(site.url, post.id, { name, body });
  expect(c.status).toBe("pending");

  const page = await as("editor");
  await openQueue(page, post);
  await expect(row(page, name)).toBeVisible();
  await expect(tabCount(page, "Pending")).toHaveText("1");
  await expectAccessible(page);
  const permalink = await row(page, name).getByRole("link", { name: /View/ }).getAttribute("href");
  expect(permalink).toBeTruthy();

  const patch = patchOf(page, c.id);
  await row(page, name)
    .getByRole("button", { name: `Approve: comment by ${name}` })
    .click();
  expect((await patch).status()).toBe(200);
  await expect(row(page, name)).toHaveCount(0);
  await expect(page.getByText("Comment approved", { exact: true })).toBeVisible();
  await expect(tabCount(page, "Pending")).toHaveText("0");
  await expect(tabCount(page, "Approved")).toHaveText("1");
  await expect(queue(page).getByText("Nothing waits for moderation")).toBeVisible();

  // The public post shows it (its page is rendered again after the moderation).
  const guest = await apiRequest.newContext({ baseURL: site.url });
  await expect
    .poll(async () => (await (await guest.get(permalink ?? "/")).text()).includes(body), {
      timeout: 10_000,
    })
    .toBe(true);
  await guest.dispose();
  expect((await api.listComments(post.id, "approved")).map((x) => x.id)).toEqual([c.id]);
  await api.dispose();
});

test("spam_and_undo", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Spam");
  const name = `Seller ${uid("s").slice(-6)}`;
  const c = await guestComment(site.url, post.id, { name, body: "Cheap watches" });

  const page = await as("editor");
  await openQueue(page, post);
  const patch = patchOf(page, c.id);
  await row(page, name)
    .getByRole("button", { name: `Spam: comment by ${name}` })
    .click();
  // Optimistic: the row leaves before the server answers.
  await expect(row(page, name)).toHaveCount(0);
  expect((await patch).status()).toBe(200);
  const toast = page.locator("[data-sonner-toast]", { hasText: "Comment marked as spam" });
  await expect(toast).toBeVisible();
  await expect(tabCount(page, "Spam")).toHaveText("1");
  expect((await api.listComments(post.id, "spam")).map((x) => x.id)).toEqual([c.id]);

  // Undo sets the previous status again.
  const back = patchOf(page, c.id);
  await toast.getByRole("button", { name: "Undo" }).click();
  expect((await back).status()).toBe(200);
  await expect(row(page, name)).toBeVisible();
  await expect(tabCount(page, "Pending")).toHaveText("1");
  await expect(tabCount(page, "Spam")).toHaveText("0");
  expect((await api.listComments(post.id, "pending")).map((x) => x.id)).toEqual([c.id]);
  await api.dispose();
});

test("bulk_trash_with_partial_failure", async ({ as, site, backend, guard, snap }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Bulk");
  const names = [1, 2, 3].map((n) => `Bulk ${n} ${uid("n").slice(-5)}`);
  const ids: string[] = [];
  for (const name of names) {
    ids.push((await guestComment(site.url, post.id, { name, body: `From ${name}` })).id);
  }

  const page = await as("editor");
  await openQueue(page, post);
  for (const name of names) await expect(row(page, name)).toBeVisible();

  // Someone else removes the second comment meanwhile (trash, then delete for good).
  await api.moderateComment(ids[1] ?? "", "trash");
  await api.deleteComment(ids[1] ?? "");
  // Its PATCH answers 404; the browser logs the failed request.
  guard.allowFailedResource(/\/api\/v1\/comments\//);

  await queue(page).getByRole("checkbox", { name: "Select all rows" }).check();
  const bar = page.getByRole("region", { name: "Selection" });
  await expect(bar).toContainText("3");
  await bar.getByRole("button", { name: "Trash" }).click();

  const report = page.locator("[data-slot=failure-report]");
  await expect(report).toContainText("1 of 3 comments could not be changed");
  await expect(report).toContainText(names[1] ?? "");
  await expect(report).toContainText("It no longer exists.");
  await expect(page.getByText("2 comments moved to trash")).toBeVisible();
  await expect(tabCount(page, "Trash")).toHaveText("2");
  await expect(tabCount(page, "Pending")).toHaveText("0");
  expect((await api.listComments(post.id, "trash")).map((x) => x.id).sort()).toEqual(
    [ids[0], ids[2]].sort(),
  );
  await snap(page, "comments-failure-report", {
    prepare: async () => {
      await page.evaluate(() => {
        for (const t of document.querySelectorAll("[data-sonner-toaster]")) t.remove();
      });
    },
  });
  await api.dispose();
});

test("reply_as_moderator", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Reply");
  const name = `Asker ${uid("a").slice(-6)}`;
  const c = await guestComment(site.url, post.id, { name, body: "Does this work offline?" });

  const page = await as("editor");
  await ownAddress(page);
  await openQueue(page, post);
  // A pending comment: approve first, then the reply.
  await row(page, name)
    .getByRole("button", { name: `Approve and reply: comment by ${name}` })
    .click();
  const box = row(page, name).locator("[data-slot=reply-box]");
  await expect(box.getByRole("textbox", { name: `Reply to ${name}` })).toBeFocused();
  const reply = `Yes, it does ${uid("y").slice(-6)}`;
  await box.getByRole("textbox").fill(reply);
  const posted = page.waitForResponse(
    (r) => r.request().method() === "POST" && r.url().includes(`/content/post/${post.id}/comments`),
  );
  await box.getByRole("button", { name: "Approve and reply" }).click();
  expect((await posted).status()).toBe(201);
  await expect(page.getByText("Comment approved and reply posted")).toBeVisible();
  await expect(row(page, name)).toHaveCount(0);

  // Both are approved; the reply says what it answers.
  await tab(page, "Approved").click();
  await expect(tabCount(page, "Approved")).toHaveText("2");
  const answer = queue(page).locator("tr[data-row]", { hasText: reply });
  await expect(answer).toContainText(`Reply to ${name}`);
  const approved = await api.listComments(post.id, "approved");
  expect(approved.find((x) => x.body === reply)?.parent_id).toBe(c.id);

  // Reply to an approved comment: posted at once.
  await row(page, name)
    .getByRole("button", { name: `Reply: comment by ${name}` })
    .click();
  const second = `And on mobile ${uid("m").slice(-6)}`;
  const box2 = row(page, name).locator("[data-slot=reply-box]");
  await box2.getByRole("textbox").fill(second);
  await box2.getByRole("button", { name: "Post reply" }).click();
  await expect(page.getByText("Reply posted", { exact: true })).toBeVisible();
  await expect(queue(page).locator("tr[data-row]", { hasText: second })).toBeVisible();
  await expect(tabCount(page, "Approved")).toHaveText("3");
  await api.dispose();
});

test("keyboard_moderation", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Keys");
  // Newest first: C, B, A.
  const names = ["A", "B", "C"].map((n) => `Key ${n} ${uid("k").slice(-5)}`);
  const ids: string[] = [];
  for (const name of names) {
    ids.push((await guestComment(site.url, post.id, { name, body: `Body of ${name}` })).id);
  }
  const [a, b, c] = names as [string, string, string];

  const page = await as("editor");
  await openQueue(page, post);
  await expect(queue(page).locator("tr[data-row]")).toHaveCount(3);

  // `?` lists the queue's keys.
  await page.keyboard.press("?");
  const sheet = page.getByRole("dialog", { name: "Keyboard shortcuts" });
  await expect(sheet.getByRole("heading", { name: "Comments queue" })).toBeVisible();
  await expect(
    sheet.getByText("Approve the selection or the focused comment", { exact: true }),
  ).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(sheet).toBeHidden();

  // `j` and `k` move the focus down and up the rows.
  await row(page, c).focus();
  await page.keyboard.press("j");
  await expect(row(page, b)).toBeFocused();
  await page.keyboard.press("j");
  await expect(row(page, a)).toBeFocused();
  await page.keyboard.press("k");
  await expect(row(page, b)).toBeFocused();
  await page.keyboard.press("k");
  await expect(row(page, c)).toBeFocused();

  // `a` approves the focused row; focus moves on to the next one.
  await page.keyboard.press("a");
  await expect(row(page, c)).toHaveCount(0);
  await expect(row(page, b)).toBeFocused();
  // `s` marks it as spam.
  await page.keyboard.press("s");
  await expect(row(page, b)).toHaveCount(0);
  await expect(row(page, a)).toBeFocused();
  // `x` selects, `#` trashes the selection.
  await page.keyboard.press("x");
  await expect(row(page, a)).toHaveAttribute("data-state", "selected");
  await page.keyboard.press("#");
  await expect(row(page, a)).toHaveCount(0);

  await expect
    .poll(async () => [
      (await api.listComments(post.id, "approved")).map((x) => x.id),
      (await api.listComments(post.id, "spam")).map((x) => x.id),
      (await api.listComments(post.id, "trash")).map((x) => x.id),
    ])
    .toEqual([[ids[2]], [ids[1]], [ids[0]]]);

  // `u` unapproves; `enter` opens the item in the editor.
  await openQueue(page, post, "approved");
  await row(page, c).focus();
  await page.keyboard.press("u");
  await expect(row(page, c)).toHaveCount(0);
  await expect
    .poll(async () => (await api.listComments(post.id, "pending")).map((x) => x.id))
    .toEqual([ids[2]]);
  await openQueue(page, post, "trash");
  await row(page, a).focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(new RegExp(`/admin/content/post/${post.id}$`));
  await api.dispose();
});

test("hostile_comment_renders_as_text", async ({ as, site, backend }) => {
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Hostile");
  const name = `<img src=x onerror=alert(1)> ${uid("h").slice(-5)}`;
  const body = `<img src=x onerror=alert(1)>\n<script>alert(2)</script>\n<b>not bold</b>`;
  const c = await guestComment(site.url, post.id, { name, body });

  const page = await as("editor");
  const dialogs: string[] = [];
  page.on("dialog", (d) => {
    dialogs.push(d.message());
    void d.dismiss();
  });
  // Core refuses a `javascript:` website on submit, so the list answer carries one here,
  // as a stored row from elsewhere (an import, an older version) might.
  await page.route(/\/api\/v1\/comments\?/, async (route) => {
    const res = await route.fetch();
    const json = (await res.json()) as { items: { id: string; author_url: string }[] };
    for (const item of json.items) if (item.id === c.id) item.author_url = "javascript:alert(1)";
    await route.fulfill({ response: res, json });
  });
  await openQueue(page, post);
  const article = page.getByRole("article", { name, exact: true });
  await expect(article).toBeVisible();
  await expect(article.locator("[data-slot=comment-body]")).toHaveText(body);
  await expect(article.getByText(name, { exact: true })).toBeVisible();
  await expect(article.getByText("javascript:alert(1)", { exact: true })).toBeVisible();
  // Nothing of it became markup.
  await expect(queue(page).locator("img, script, b")).toHaveCount(0);
  await expect(queue(page).locator("a[href^='javascript' i]")).toHaveCount(0);
  expect(dialogs).toEqual([]);
  await api.dispose();
});

test("author_has_no_queue", async ({ as, snap, expectAccessible }) => {
  const page = await as("author");
  await page.goto("/admin/");
  const nav = page.getByRole("navigation", { name: "Main" });
  await expect(nav.getByRole("link", { name: "Media" })).toBeVisible();
  await expect(nav.getByRole("link", { name: /^Comments/ })).toHaveCount(0);
  await page.goto("/admin/comments");
  await expect(page.getByText("You don't have access to this")).toBeVisible();
  await expect(queue(page)).toHaveCount(0);
  await snap(page, "comments-forbidden");
  await expectAccessible(page);
});

test("screens", async ({ as, site, backend, snap, expectAccessible }) => {
  test.setTimeout(240_000);
  const api = await ContentApi.as(backend, site.url, "editor");
  const post = await newPost(api, "Screens");
  const empty = await newPost(api, "Quiet");
  // An approved thread starter, a pending reply to it, a long pending comment, one with
  // a website, one for Spam and one for Trash.
  const starter = await guestComment(site.url, post.id, {
    name: "Grace Hopper",
    body: "It's easier to ask forgiveness than it is to get permission.",
  });
  await api.moderateComment(starter.id, "approved");
  const long = Array.from(
    { length: 14 },
    (_, i) => `Line ${i + 1}: a thought that goes on for a while, as long comments do.`,
  ).join("\n");
  await guestComment(site.url, post.id, { name: "Ada Lovelace", body: long });
  await guestComment(site.url, post.id, {
    name: "Alan Turing",
    body: "Agreed. We can only see a short distance ahead.",
    parent_id: starter.id,
  });
  const linked = await guestComment(site.url, post.id, {
    name: "Margaret Hamilton",
    body: "Looks good to me.",
  });
  void linked;
  const spam = await guestComment(site.url, post.id, {
    name: "Best Deals",
    body: "Buy now!!! Limited offer.",
  });
  await api.moderateComment(spam.id, "spam");
  const trash = await guestComment(site.url, post.id, { name: "Old Note", body: "Never mind." });
  await api.moderateComment(trash.id, "trash");

  const page = await as("editor");
  await openQueue(page, post);
  await expect(queue(page).locator("tr[data-row]")).toHaveCount(3);
  await expect(row(page, "Alan Turing")).toContainText("Reply to Grace Hopper");
  await snap(page, "comments-pending");
  await expectAccessible(page);

  // A long body, expanded.
  await row(page, "Ada Lovelace").getByRole("button", { name: "Show more" }).click();
  await expect(row(page, "Ada Lovelace").getByRole("button", { name: "Show less" })).toBeVisible();
  await snap(page, "comments-row-expanded");
  await expectAccessible(page);

  // The reply box under a pending comment (Approve and reply).
  await row(page, "Margaret Hamilton")
    .getByRole("button", { name: "Approve and reply: comment by Margaret Hamilton" })
    .click();
  await expect(row(page, "Margaret Hamilton").locator("[data-slot=reply-box]")).toBeVisible();
  await snap(page, "comments-reply-box", {
    prepare: async () => {
      await row(page, "Margaret Hamilton")
        .locator("[data-slot=reply-box]")
        .scrollIntoViewIfNeeded();
    },
  });
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  // A selection with the bar.
  await queue(page).getByRole("checkbox", { name: "Select all rows" }).check();
  await expect(page.getByRole("region", { name: "Selection" })).toBeVisible();
  await snap(page, "comments-selected");
  await expectAccessible(page);
  await page.keyboard.press("Escape");

  for (const status of ["approved", "spam", "trash"]) {
    await openQueue(page, post, status);
    await expect(queue(page).locator("tr[data-row]")).toHaveCount(1);
    await snap(page, `comments-${status}`);
    await expectAccessible(page);
  }

  // Delete permanently asks first and says the replies go too.
  await row(page, "Old Note")
    .getByRole("button", { name: "Delete permanently: comment by Old Note" })
    .click();
  const confirm = page.getByRole("alertdialog");
  await expect(confirm).toContainText("every reply to it");
  await snap(page, "comments-delete-confirm");
  await expectAccessible(page);
  await confirm.getByRole("button", { name: "Delete permanently" }).click();
  await expect(page.getByText("Comment deleted", { exact: true })).toBeVisible();
  await expect(queue(page).getByText("Trash is empty")).toBeVisible();

  // An item without comments: the empty Pending tab; the chip still names the item
  // (`GET /content/refs`, M2b-2 Task 14).
  await openQueue(page, empty, undefined, empty.title);
  await expect(queue(page).getByText("Nothing waits for moderation")).toBeVisible();
  await snap(page, "comments-empty-pending");
  await expectAccessible(page);
  await api.dispose();
});
