// Content set-up through the API for specs that need their own data (E2E data hygiene):
// a request context on a harness account's shared session, plus helpers to create a
// content type (admin only), taxonomies, terms and items in any status, and to remove a
// type with everything in it afterwards, so the sidebar of later screenshots stays clean.

import { expect, request as apiRequest, type APIRequestContext } from "@playwright/test";
import type { Backend } from "./server";
import { authFile } from "./server";
import type { Role } from "./users";

export interface Item {
  id: string;
  type: string;
  title: string;
  slug: string;
  status: string;
  version: number;
  parent_id?: string | null;
  published_at?: string | null;
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

  /** A content type named `name` (needs `manage_settings`). */
  async createType(name: string, label: string, hierarchical = false): Promise<void> {
    await this.ok(
      await this.ctx.post("/api/v1/types", {
        headers: this.headers(),
        data: { name, label, hierarchical, has_archive: false },
      }),
      `create type ${name}`,
    );
  }

  async createTaxonomy(name: string, label: string, contentTypes: string[]): Promise<void> {
    await this.ok(
      await this.ctx.post("/api/v1/taxonomies", {
        headers: this.headers(),
        data: { name, label, content_types: contentTypes },
      }),
      `create taxonomy ${name}`,
    );
  }

  async createTerm(taxonomy: string, name: string): Promise<{ id: string; name: string }> {
    return this.ok(
      await this.ctx.post(`/api/v1/taxonomies/${taxonomy}/terms`, {
        headers: this.headers(),
        data: { name },
      }),
      `create term ${name}`,
    );
  }

  async create(
    type: string,
    title: string,
    extra: { parent_id?: string; terms?: string[] } = {},
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
    for (const tax of taxonomies) {
      const res = await this.ctx.get(`/api/v1/taxonomies/${tax}/terms`);
      const terms = res.ok() ? ((await res.json()) as { items: { id: string }[] }).items : [];
      for (const term of terms) {
        await this.ctx.delete(`/api/v1/terms/${term.id}`, { headers: this.headers() });
      }
      await this.ctx.delete(`/api/v1/taxonomies/${tax}`, { headers: this.headers() });
    }
    await this.ctx.delete(`/api/v1/types/${name}`, { headers: this.headers() });
  }

  async dispose(): Promise<void> {
    await this.ctx.dispose();
  }
}
