// Content set-up through the API for specs that need their own data (E2E data hygiene):
// a request context on a harness account's shared session, plus helpers to create a
// content type (admin only), taxonomies, terms and items in any status, and to remove a
// type with everything in it afterwards, so the sidebar of later screenshots stays clean;
// menus (read, replace, delete); and comments (list per item, moderate, delete).

import { expect, request as apiRequest, type APIRequestContext } from "@playwright/test";
import type { FixtureFile } from "./files";
import type { Backend } from "./server";
import { authFile } from "./server";
import type { Role } from "./users";

export interface Item {
  id: string;
  body?: unknown;
  type: string;
  title: string;
  slug: string;
  status: string;
  version: number;
  parent_id?: string | null;
  published_at?: string | null;
}

/** A media item as `GET /media/{id}` answers (the members the specs read). */
export interface Media {
  id: string;
  title: string;
  alt: string;
  mime: string;
  original_name: string;
  status: string;
  version: number;
  sanitized: boolean;
  author_id: string;
  variants: { name: string; url: string; width: number; height: number }[];
}

export interface Term {
  id: string;
  name: string;
  slug: string;
  description: string;
  parent_id?: string | null;
}

/** A menu as `GET /menus/{name}` answers. */
export interface Menu {
  name: string;
  label: string;
  items: {
    id: string;
    kind: "item" | "term" | "url";
    target_id?: string | null;
    url?: string | null;
    label?: string;
    parent_id?: string | null;
    position?: number;
  }[];
}

/** A comment as `GET /comments` lists it (the members the specs read). */
export interface QueuedComment {
  id: string;
  author_name: string;
  author_email: string;
  author_url: string;
  body: string;
  status: string;
  parent_id?: string | null;
  item: { id: string; type: string; title: string; permalink: string | null; status: string };
}

export class ContentApi {
  private constructor(
    readonly ctx: APIRequestContext,
    readonly me: { id: string; login: string },
    private readonly csrf: string,
  ) {}

  /** A context on `role`'s shared session (never logged out, F23), sending the `Origin`. */
  static async as(backend: Backend, url: string, role: Role): Promise<ContentApi> {
    const ctx = await apiRequest.newContext({
      baseURL: url,
      storageState: authFile(backend, role),
      extraHTTPHeaders: { Origin: url },
    });
    const res = await ctx.get("/api/v1/auth/me");
    expect(res.status(), `me of ${role}`).toBe(200);
    const me = (await res.json()) as { id: string; login: string; csrf_token: string };
    return new ContentApi(ctx, { id: me.id, login: me.login }, me.csrf_token);
  }

  private headers(version?: number): Record<string, string> {
    return {
      "X-CSRF-Token": this.csrf,
      ...(version === undefined ? {} : { "If-Match": `"${version}"` }),
    };
  }

  private async ok<T>(res: Awaited<ReturnType<APIRequestContext["get"]>>, what: string) {
    expect(res.ok(), `${what}: ${res.status()} ${await res.text()}`).toBe(true);
    return (res.status() === 204 ? undefined : await res.json()) as T;
  }

  /** A content type named `name` (needs `manage_settings`), with a field schema if given. */
  async createType(
    name: string,
    label: string,
    hierarchical = false,
    fields?: unknown[],
  ): Promise<void> {
    await this.ok(
      await this.ctx.post("/api/v1/types", {
        headers: this.headers(),
        data: { name, label, hierarchical, has_archive: false, ...(fields ? { fields } : {}) },
      }),
      `create type ${name}`,
    );
  }

  /** `PATCH /types/{name}` (a new schema, a label). */
  async patchType(name: string, data: Record<string, unknown>): Promise<void> {
    await this.ok(
      await this.ctx.patch(`/api/v1/types/${name}`, { headers: this.headers(), data }),
      `patch type ${name}`,
    );
  }

  async createTaxonomy(
    name: string,
    label: string,
    contentTypes: string[],
    hierarchical = false,
  ): Promise<void> {
    await this.ok(
      await this.ctx.post("/api/v1/taxonomies", {
        headers: this.headers(),
        data: { name, label, content_types: contentTypes, hierarchical },
      }),
      `create taxonomy ${name}`,
    );
  }

  async createTerm(
    taxonomy: string,
    name: string,
    extra: { parent_id?: string; description?: string } = {},
  ): Promise<Term> {
    return this.ok(
      await this.ctx.post(`/api/v1/taxonomies/${taxonomy}/terms`, {
        headers: this.headers(),
        data: { name, ...extra },
      }),
      `create term ${name}`,
    );
  }

  /** `PATCH /terms/{id}` (merged into the stored term). */
  async patchTerm(id: string, data: Record<string, unknown>): Promise<Term> {
    return this.ok(
      await this.ctx.patch(`/api/v1/terms/${id}`, { headers: this.headers(), data }),
      `patch term ${id}`,
    );
  }

  async listTerms(taxonomy: string): Promise<Term[]> {
    const res = await this.ctx.get(`/api/v1/taxonomies/${taxonomy}/terms`);
    return (await this.ok<{ items: Term[] }>(res, `terms of ${taxonomy}`)).items;
  }

  /**
   * Deletes taxonomy `name` with its terms (sub-terms first: a term with sub-terms
   * cannot be deleted). Best effort: a failure only leaves a stray taxonomy behind.
   */
  async removeTaxonomy(name: string): Promise<void> {
    for (let round = 0; round < 64; round++) {
      const res = await this.ctx.get(`/api/v1/taxonomies/${name}/terms`);
      if (!res.ok()) break;
      const terms = ((await res.json()) as { items: Term[] }).items;
      if (terms.length === 0) break;
      const parents = new Set(terms.flatMap((t) => (t.parent_id ? [t.parent_id] : [])));
      for (const term of terms.filter((t) => !parents.has(t.id))) {
        await this.ctx.delete(`/api/v1/terms/${term.id}`, { headers: this.headers() });
      }
    }
    await this.ctx.delete(`/api/v1/taxonomies/${name}`, { headers: this.headers() });
  }

  /** Trashes and then deletes `item` for good (best effort, like `removeType`). */
  async purge(item: Item): Promise<void> {
    const current = await this.get(item.type, item.id);
    const trashed = current.status === "trash" ? current : await this.trash(current);
    await this.ctx.delete(`/api/v1/content/${item.type}/${item.id}?force=true`, {
      headers: this.headers(trashed.version),
    });
  }

  /** Removes a term (best effort: a failure only leaves a stray term behind). */
  async deleteTerm(id: string): Promise<void> {
    await this.ctx.delete(`/api/v1/terms/${id}`, { headers: this.headers() });
  }

  async create(
    type: string,
    title: string,
    extra: { parent_id?: string; terms?: string[]; body?: unknown; fields?: unknown } = {},
  ): Promise<Item> {
    return this.ok(
      await this.ctx.post(`/api/v1/content/${type}`, {
        headers: this.headers(),
        data: { title, ...extra },
      }),
      `create ${title}`,
    );
  }

  /** A transition (`publish`, `submit`, `restore`, `schedule` with `at`). */
  async transition(
    item: Item,
    action: "publish" | "submit" | "restore" | "schedule",
    at?: string,
  ): Promise<Item> {
    return this.ok(
      await this.ctx.post(`/api/v1/content/${item.type}/${item.id}/${action}`, {
        headers: this.headers(item.version),
        ...(at ? { data: { at } } : {}),
      }),
      `${action} ${item.title}`,
    );
  }

  async trash(item: Item): Promise<Item> {
    return this.ok(
      await this.ctx.delete(`/api/v1/content/${item.type}/${item.id}`, {
        headers: this.headers(item.version),
      }),
      `trash ${item.title}`,
    );
  }

  async patch(item: Item, data: Record<string, unknown>): Promise<Item> {
    return this.ok(
      await this.ctx.patch(`/api/v1/content/${item.type}/${item.id}`, {
        headers: this.headers(item.version),
        data,
      }),
      `patch ${item.title}`,
    );
  }

  async get(type: string, id: string): Promise<Item> {
    return this.ok(await this.ctx.get(`/api/v1/content/${type}/${id}`), `get ${id}`);
  }

  /** An item created in `status` (private cannot be reached through the API, F7). */
  async createIn(
    type: string,
    title: string,
    status: "draft" | "pending" | "scheduled" | "published" | "trash",
    extra: { parent_id?: string; terms?: string[] } = {},
  ): Promise<Item> {
    const item = await this.create(type, title, extra);
    switch (status) {
      case "draft":
        return item;
      case "pending":
        return this.transition(item, "submit");
      case "scheduled":
        return this.transition(
          item,
          "schedule",
          new Date(Date.now() + 3 * 86_400_000).toISOString(),
        );
      case "published":
        return this.transition(item, "publish");
      case "trash":
        return this.trash(item);
    }
  }

  /**
   * Deletes type `name` with all its items (children first) and the given taxonomies.
   * Best effort: a failure here only leaves a stray type behind.
   */
  async removeType(name: string, taxonomies: string[] = []): Promise<void> {
    const all = "draft,pending,scheduled,published,private,trash";
    for (let round = 0; round < 5; round++) {
      const res = await this.ctx.get(`/api/v1/content/${name}?status=${all}&limit=200`);
      if (!res.ok()) break;
      const items = ((await res.json()) as { items: Item[] }).items;
      if (items.length === 0) break;
      // Children before their parents: a parent with children cannot be deleted.
      const parents = new Set(items.flatMap((i) => (i.parent_id ? [i.parent_id] : [])));
      for (const item of items.filter((i) => !parents.has(i.id))) {
        const trashed = item.status === "trash" ? item : await this.trash(item);
        await this.ctx.delete(`/api/v1/content/${name}/${item.id}?force=true`, {
          headers: this.headers(trashed.version),
        });
      }
    }
    for (const tax of taxonomies) await this.removeTaxonomy(tax);
    await this.ctx.delete(`/api/v1/types/${name}`, { headers: this.headers() });
  }

  /** `POST /media` (multipart) with an optional title and alt text. */
  async upload(file: FixtureFile, extra: { title?: string; alt?: string } = {}): Promise<Media> {
    return this.ok(
      await this.ctx.post("/api/v1/media", {
        headers: this.headers(),
        multipart: {
          ...(extra.title ? { title: extra.title } : {}),
          ...(extra.alt ? { alt: extra.alt } : {}),
          file: { name: file.name, mimeType: file.mimeType, buffer: file.buffer },
        },
      }),
      `upload ${file.name}`,
    );
  }

  async getMedia(id: string): Promise<Media> {
    return this.ok(await this.ctx.get(`/api/v1/media/${id}`), `get media ${id}`);
  }

  /** The status of a media item, or 404 once it is gone. */
  async mediaStatus(id: string): Promise<number> {
    return (await this.ctx.get(`/api/v1/media/${id}`)).status();
  }

  /** `GET /media` with `query` (one page). */
  async listMedia(query: Record<string, string>): Promise<Media[]> {
    const res = await this.ctx.get(`/api/v1/media?${new URLSearchParams(query).toString()}`);
    return (await this.ok<{ items: Media[] }>(res, "list media")).items;
  }

  async trashMedia(media: Media): Promise<void> {
    await this.ok(
      await this.ctx.delete(`/api/v1/media/${media.id}`, { headers: this.headers(media.version) }),
      `trash media ${media.id}`,
    );
  }

  /** Trashes (if needed) and deletes a media item for good; best effort. */
  async purgeMedia(id: string): Promise<void> {
    const res = await this.ctx.get(`/api/v1/media/${id}`);
    if (!res.ok()) return;
    let media = (await res.json()) as Media;
    if (media.status !== "trash") {
      await this.ctx.delete(`/api/v1/media/${id}`, { headers: this.headers(media.version) });
      media = (await (await this.ctx.get(`/api/v1/media/${id}`)).json()) as Media;
    }
    await this.ctx.delete(`/api/v1/media/${id}?force=true`, {
      headers: this.headers(media.version),
    });
  }

  /** `GET /menus/{name}`; `null` when there is none. */
  async getMenu(name: string): Promise<Menu | null> {
    const res = await this.ctx.get(`/api/v1/menus/${name}`);
    if (res.status() === 404) return null;
    return this.ok<Menu>(res, `get menu ${name}`);
  }

  /** `PUT /menus/{name}`: creates the menu or replaces its whole tree. */
  async putMenu(name: string, menu: { label: string; items: Menu["items"] }): Promise<Menu> {
    return this.ok(
      await this.ctx.put(`/api/v1/menus/${name}`, { headers: this.headers(), data: menu }),
      `put menu ${name}`,
    );
  }

  /** Deletes menu `name` (best effort: a missing one is fine). */
  async deleteMenu(name: string): Promise<void> {
    await this.ctx.delete(`/api/v1/menus/${name}`, { headers: this.headers() });
  }

  /** The comments of item `item` in `status` (needs `moderate_comments`). */
  async listComments(item: string, status: string): Promise<QueuedComment[]> {
    const res = await this.ctx.get(`/api/v1/comments?item=${item}&status=${status}&limit=200`);
    return (await this.ok<{ items: QueuedComment[] }>(res, "list comments")).items;
  }

  /** `PATCH /comments/{id}` with `status`. */
  async moderateComment(id: string, status: string): Promise<QueuedComment> {
    return this.ok(
      await this.ctx.patch(`/api/v1/comments/${id}`, { headers: this.headers(), data: { status } }),
      `moderate comment ${id}`,
    );
  }

  /** `DELETE /comments/{id}` (spam or trash only; its replies go too). */
  async deleteComment(id: string): Promise<void> {
    await this.ok(
      await this.ctx.delete(`/api/v1/comments/${id}`, { headers: this.headers() }),
      `delete comment ${id}`,
    );
  }

  async dispose(): Promise<void> {
    await this.ctx.dispose();
  }
}
