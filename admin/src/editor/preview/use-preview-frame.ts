// What the live preview's iframe loads (design 15 §3, ruling P7 D16). The frame is
// sandboxed without `allow-same-origin`, so it runs in an opaque origin and cannot use the
// admin's session: it loads a signed preview link of the item instead, made when the pane
// opens (1 h), with `?working=1` (the caller's autosave slot over the stored item) or, in
// change mode, `?release=<id>` (the entry's staged change). Every refresh (an autosave
// landed, a save) loads it again; a new link is made shortly before the old one expires,
// and after a refused create the pane offers Retry.
//
// The opaque origin also means the pane cannot read or restore the frame's scroll
// position across reloads (P7): each reload starts at the top.

import { useCallback, useEffect, useRef, useState } from "react";
import { createPreviewLink, linkPath } from "@/api/preview-links";

/** A link is renewed this long before it expires. */
export const LINK_REFRESH_MARGIN_MS = 5 * 60_000;
/** The live preview's links last one hour (P7). */
const LINK_HOURS = 1;

export interface PreviewFrameOptions {
  type: string;
  /** `null`: a new item, not saved yet (nothing to preview). */
  itemId: string | null;
  /** Change mode: preview the staged change of this release's entry. */
  releaseId?: string | null;
  /** Bumped to load the frame again (the pane's refresh). */
  refreshKey: number;
  /** The pane is open (no link is made while it is closed). */
  enabled: boolean;
}

export interface PreviewFrame {
  /** The frame's `src`, or `null` while there is none (yet). */
  src: string | null;
  /** Making the link was refused or failed. */
  failed: boolean;
  /** Tries again after a failure. */
  retry: () => void;
}

interface Link {
  /** The item it previews (a link of another item is never shown). */
  item: string;
  path: string;
  expiresAt: number;
}

export function usePreviewFrame({
  type,
  itemId,
  releaseId,
  refreshKey,
  enabled,
}: PreviewFrameOptions): PreviewFrame {
  const [link, setLink] = useState<Link | null>(null);
  const [failed, setFailed] = useState(false);
  const [attempt, setAttempt] = useState(0);
  const live = useRef(true);
  useEffect(() => {
    live.current = true;
    return () => {
      live.current = false;
    };
  }, []);

  // A link per item, made when the pane opens, again on Retry and before it expires.
  useEffect(() => {
    if (!enabled || itemId === null) return;
    let cancelled = false;
    let renew: ReturnType<typeof setTimeout> | null = null;
    const make = () => {
      createPreviewLink({ kind: "item", type, id: itemId }, LINK_HOURS).then(
        (dto) => {
          if (cancelled || !live.current) return;
          const path = linkPath(dto);
          if (!path) {
            setFailed(true);
            return;
          }
          const expiresAt = Date.parse(dto.expires_at);
          setFailed(false);
          setLink({ item: itemId, path, expiresAt });
          const wait = Math.max(30_000, expiresAt - Date.now() - LINK_REFRESH_MARGIN_MS);
          renew = setTimeout(make, wait);
        },
        () => {
          if (cancelled || !live.current) return;
          setFailed(true);
          setLink(null);
        },
      );
    };
    make();
    return () => {
      cancelled = true;
      if (renew !== null) clearTimeout(renew);
    };
  }, [enabled, type, itemId, attempt]);

  const retry = useCallback(() => {
    setFailed(false);
    setAttempt((a) => a + 1);
  }, []);

  let src: string | null = null;
  if (link && enabled && link.item === itemId && !failed) {
    const query = releaseId ? `release=${encodeURIComponent(releaseId)}` : "working=1";
    // A changed `src` is what reloads the frame (its document cannot be reached).
    src = `${link.path}?${query}${refreshKey > 0 ? `&r=${refreshKey}` : ""}`;
  }
  return { src, failed, retry };
}
