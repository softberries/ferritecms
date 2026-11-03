// Signed, expiring preview links (M6-3, design 15 §7): `ferp_…` tokens that open a preview
// without a session. The editor's live preview loads through one (P7 D16: the iframe is
// sandboxed without `allow-same-origin`, so it cannot use the admin's cookie), "Preview
// site" opens one for a release, and "Share preview…" hands one out. The token comes back
// once, on create: callers keep it in component state, never in the query cache.

import { queryOptions } from "@tanstack/react-query";
import { api, unwrap } from "./client";
import type { SchemaPreviewLinkDto } from "./schema";

export type PreviewLinkDto = SchemaPreviewLinkDto;

/** The expiries the admin offers, in hours (the server takes 1 to 168). */
export const LINK_HOURS = [1, 24, 168] as const;
export type LinkHours = (typeof LINK_HOURS)[number];

/** A link's target: an item (with its type) or a release. */
export type LinkTarget =
  { kind: "item"; type: string; id: string } | { kind: "release"; id: string };

export const previewLinksKey = (target: LinkTarget) =>
  target.kind === "item"
    ? (["preview-links", "item", target.id] as const)
    : (["preview-links", "release", target.id] as const);

export function createPreviewLink(target: LinkTarget, hours: number): Promise<PreviewLinkDto> {
  const body = { expires_in_hours: hours };
  return target.kind === "item"
    ? unwrap(
        api.POST("/api/v1/content/{type}/{id}/preview-links", {
          params: { path: { type: target.type, id: target.id } },
          body,
        }),
      )
    : unwrap(
        api.POST("/api/v1/releases/{id}/preview-links", {
          params: { path: { id: target.id } },
          body,
        }),
      );
}

export async function listPreviewLinks(target: LinkTarget): Promise<PreviewLinkDto[]> {
  const list =
    target.kind === "item"
      ? await unwrap(
          api.GET("/api/v1/content/{type}/{id}/preview-links", {
            params: { path: { type: target.type, id: target.id } },
          }),
        )
      : await unwrap(
          api.GET("/api/v1/releases/{id}/preview-links", { params: { path: { id: target.id } } }),
        );
  return list.items;
}

export function previewLinksQuery(target: LinkTarget) {
  return queryOptions({
    queryKey: previewLinksKey(target),
    queryFn: () => listPreviewLinks(target),
    staleTime: 5_000,
  });
}

export async function revokePreviewLink(id: string): Promise<void> {
  await unwrap(api.DELETE("/api/v1/preview-links/{id}", { params: { path: { id } } }));
}

/**
 * The link's path on this site (`/_preview/link/ferp_…`): the server answers an absolute
 * URL on `site.url`, which may name another host than the one the admin runs on (a
 * proxy, a dev server); the path is what the admin loads. `null` without a URL.
 */
export function linkPath(link: Pick<PreviewLinkDto, "url" | "token">): string | null {
  if (link.url) {
    try {
      return new URL(link.url, window.location.origin).pathname;
    } catch {
      return null;
    }
  }
  return link.token ? `/_preview/link/${link.token}` : null;
}
