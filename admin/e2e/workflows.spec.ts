// Approval workflows under the real server (M6-3, design 15 §6, §11.4): an admin sets "One
// editor approves" on posts; the author submits a post with a note and assigns the editor;
// the editor approves from the queue; the author publishes. An edit after approval makes
// the approval outdated (Resubmit); a published post offers the author "Save as change";
// an admin publishes another post with an override and a reason, which the audit log shows.
// axe on the workflow editor, the queue and the Review section. The workflow on posts is
// site-wide state: this spec runs with the site-wide ones (`projects.ts`) and removes it at
// the end.

import type { Page } from "@playwright/test";
import { ContentApi, type Item } from "./support/content-api";
import { expect, test } from "./support/fixtures";
import { mod } from "./support/keys";
import { uid } from "./support/uid";

interface Workflow {
  version: number;
}
interface Stored extends Item {
  review?: { id: string; status: string; current: boolean } | null;
  allowed: { publish: boolean; stage: boolean; edit: boolean };
}

const panel = (page: Page) => page.locator("[data-slot=editor-panel]");
const review = (page: Page) => panel(page).locator("[data-slot=panel-review]");
const title = (page: Page) => page.getByRole("textbox", { name: "Title", exact: true });

async function openEditor(page: Page, item: Item) {
  await page.setViewportSize({ width: 1280, height: 900 });
  await page.goto(`/admin/content/${item.type}/${item.id}`);
  await expect(title(page)).toBeVisible();
  await expect(panel(page)).toBeVisible();
}

test("an_editor_approves_and_the_author_publishes", async ({
  as,
  site,
  siteKey,
  snap,
  expectAccessible,
}) => {
  test.setTimeout(180_000);
  const adminApi = await ContentApi.as(siteKey, site.url, "admin");
  const authorApi = await ContentApi.as(siteKey, site.url, "author");
  const editorApi = await ContentApi.as(siteKey, site.url, "editor");
  const tag = uid();
  const created: Item[] = [];
  try {
    // The admin sets the preset on posts.
    const admin = await as("admin");
    await admin.goto("/admin/structure/types/post");
    const wf = admin.getByRole("region", { name: "Workflow" });
    await expect(wf.getByText(/No workflow/)).toBeVisible();
    await wf.getByRole("button", { name: "One editor approves" }).click();
    await expect(wf.getByLabel("Label")).toHaveValue("Editor approval");
    await wf.getByRole("button", { name: "Save workflow" }).click();
    await expect(
      admin.locator("[data-sonner-toast]").filter({ hasText: "Workflow saved" }),
    ).toBeVisible();
    await snap(admin, "type-workflow");
    await expectAccessible(admin);

    // The author cannot publish; submits with a note.
    const post = await authorApi.create("post", `Workflow ${tag}`);
    created.push(post);
    const author = await as("author");
    await openEditor(author, post);
    await expect(panel(author).getByRole("button", { name: /^Publish/ })).toHaveCount(0);
    await expect(review(author)).toContainText("Not submitted for review yet");
    await review(author).getByRole("button", { name: "Submit for review…" }).click();
    const submit = author.locator("[data-slot=submit-dialog]");
    await submit.getByLabel("Note").fill("Please check the dates");
    await expectAccessible(author);
    await submit.getByRole("button", { name: "Submit for review" }).click();
    await expect(review(author).locator("[data-slot=review-status]")).toHaveText("In review");
    await expect(review(author)).toContainText("Please check the dates");
    const submitted = (await authorApi.get("post", post.id)) as Stored;
    expect(submitted.status).toBe("pending");
    // The author asks the editor (the API: an author cannot list users).
    await authorApi.call("PUT", `/reviews/${submitted.review?.id}/assignees`, {
      data: { step: 0, users: [editorApi.me.id] },
    });

    // The editor approves from the queue.
    const editor = await as("editor");
    await editor.goto("/admin/reviews");
    const queue = editor.getByRole("table", { name: "Reviews" });
    await expect(queue.getByText(`Workflow ${tag}`)).toBeVisible();
    await expectAccessible(editor);
    await queue.getByText(`Workflow ${tag}`).click();
    await expect(title(editor)).toHaveValue(`Workflow ${tag}`);
    await review(editor).getByRole("button", { name: "Approve…" }).click();
    const decision = editor.locator("[data-slot=decision-dialog]");
    await decision.getByLabel("Comment").fill("Looks good");
    await decision.getByRole("button", { name: "Approve" }).click();
    await expect(review(editor).locator("[data-slot=review-status]")).toHaveText("Approved");
    await snap(editor, "editor-review-approved", { fullPage: false });
    await expectAccessible(editor);

    // An edit after the approval makes it outdated: Resubmit.
    await author.reload();
    await expect(panel(author).getByRole("button", { name: "Publish…" })).toBeVisible();
    await title(author).fill(`Workflow ${tag} v2`);
    await author.keyboard.press(`${await mod(author)}+s`);
    await expect(review(author).getByRole("button", { name: "Resubmit…" })).toBeVisible();
    await expect(panel(author).getByRole("button", { name: "Publish…" })).toHaveCount(0);
    await review(author).getByRole("button", { name: "Resubmit…" }).click();
    await author
      .locator("[data-slot=submit-dialog]")
      .getByRole("button", { name: "Submit for review" })
      .click();
    const again = (await authorApi.get("post", post.id)) as Stored;
    await editorApi.call("POST", `/reviews/${again.review?.id}/decisions`, {
      data: { decision: "approve" },
      version: (await editorApi.call<{ version: number }>("GET", `/reviews/${again.review?.id}`))
        .version,
    });

    // The author publishes.
    await author.reload();
    await panel(author).getByRole("button", { name: "Publish…" }).click();
    await author.locator("[data-slot=publish-confirm]").click();
    await expect(panel(author).locator("[data-slot=status-badge]")).toHaveText("Published");
    // Published, in a workflow type: the author stages changes instead.
    await author.reload();
    await expect(panel(author).getByRole("button", { name: "Save as change…" })).toBeVisible();
    await expect(panel(author).getByRole("button", { name: "Update" })).toHaveCount(0);

    // An admin overrides with a reason; the audit log shows it.
    const other = await adminApi.create("post", `Override ${tag}`);
    created.push(other);
    await openEditor(admin, other);
    await panel(admin).getByRole("button", { name: "Publish…" }).click();
    const dialog = admin.locator("[data-slot=publish-dialog]");
    await expect(dialog.locator("[data-slot=publish-confirm]")).toBeDisabled();
    await dialog.getByText("Override approval").click();
    await dialog.getByRole("checkbox", { name: "Publish without the missing approvals" }).check();
    await dialog.getByLabel("Reason").fill("Press embargo lifted");
    await expectAccessible(admin);
    await dialog.locator("[data-slot=publish-confirm]").click();
    await expect(panel(admin).locator("[data-slot=status-badge]")).toHaveText("Published");
    await admin.goto("/admin/system/audit");
    await expect(admin.getByText("workflow.override").first()).toBeVisible();
  } finally {
    const wf = await adminApi.ctx.get("/api/v1/workflows/post");
    if (wf.ok()) {
      const { version } = (await wf.json()) as Workflow;
      await adminApi.call("DELETE", "/workflows/post", { version });
    }
    for (const item of created) await adminApi.purge(item);
    await adminApi.dispose();
    await authorApi.dispose();
    await editorApi.dispose();
  }
});
