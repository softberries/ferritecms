// Comments for the queue's specs, made the way readers make them: a guest `POST` to the
// public submit endpoint, each from its own `X-Forwarded-For` address through the
// trusted loopback proxy (the per-address limits are 3 a minute and 20 an hour, 07 §1).
// A guest's first comment is held under the default `comments.moderation = "known"`, so
// a fresh e-mail gives a pending comment. The moderation calls are on `ContentApi`.

import { expect, request as apiRequest } from "@playwright/test";
import { randomInt } from "node:crypto";
import { uid } from "./uid";

/**
 * A random private address for one guest: never the same twice in a run (nor, in all
 * likelihood, across runs within the hour), so the per-address limits never bite.
 */
export function guestAddress(): string {
  return `10.${randomInt(100, 250)}.${randomInt(0, 256)}.${randomInt(1, 255)}`;
}

export interface GuestComment {
  name?: string;
  email?: string;
  body: string;
  parent_id?: string;
}

/** A guest's comment on post `postId`; returns its id and status (`pending` if held). */
export async function guestComment(
  siteUrl: string,
  postId: string,
  comment: GuestComment,
): Promise<{ id: string; status: string }> {
  const ctx = await apiRequest.newContext({
    baseURL: siteUrl,
    extraHTTPHeaders: { Origin: siteUrl, "X-Forwarded-For": guestAddress() },
  });
  try {
    const res = await ctx.post(`/api/v1/content/post/${postId}/comments`, {
      data: {
        author_name: comment.name ?? `Guest ${uid("g").slice(-6)}`,
        author_email: comment.email ?? `${uid("guest")}@example.test`,
        body: comment.body,
        ...(comment.parent_id ? { parent_id: comment.parent_id } : {}),
      },
    });
    expect([201, 202], `guest comment: ${res.status()} ${await res.text()}`).toContain(
      res.status(),
    );
    return (await res.json()) as { id: string; status: string };
  } finally {
    await ctx.dispose();
  }
}
