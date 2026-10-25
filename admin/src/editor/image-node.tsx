// The image node (`src`, `alt`, `title`; the core renderer reads exactly these) with a
// React node view: `ferrite:media/<uuid>` resolves through `GET /media/{id}` to the
// variant URL; a local path or http(s) URL shows as is; anything the site would drop,
// a missing or trashed media item, or a file that fails to load shows a broken-image
// state with the stored `src`, so nothing is silently lost. Inserting images arrives
// with the media picker (M2b-2); M2b-1 displays and keeps them.

import { Image, type ImageOptions } from "@tiptap/extension-image";
import { NodeViewWrapper, ReactNodeViewRenderer, type ReactNodeViewProps } from "@tiptap/react";
import { ImageOffIcon } from "lucide-react";
import { cn } from "cn";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";

const MEDIA_PREFIX = "ferrite:media/";
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;

export interface MediaImageOptions extends ImageOptions {
  /** `ferrite:media/<uuid>` → the URL to show, or `null` when the media is gone. */
  resolveMedia: (id: string) => Promise<string | null>;
}

type Source = { kind: "url"; url: string } | { kind: "media"; id: string } | { kind: "bad" };

function classify(src: unknown): Source {
  if (typeof src !== "string") return { kind: "bad" };
  const s = src.trim();
  if (s.startsWith(MEDIA_PREFIX)) {
    const id = s.slice(MEDIA_PREFIX.length);
    return UUID.test(id) ? { kind: "media", id: id.toLowerCase() } : { kind: "bad" };
  }
  if (s.startsWith("/") && !s.startsWith("//") && !s.includes("\\")) return { kind: "url", url: s };
  if (/^https?:\/\//i.test(s)) return { kind: "url", url: s };
  return { kind: "bad" };
}

type Shown = { state: "loading" } | { state: "ready"; url: string } | { state: "broken" };

function useImageUrl(src: unknown, resolve: (id: string) => Promise<string | null>): Shown {
  const source = classify(src);
  const key = source.kind === "media" ? source.id : null;
  const [resolved, setResolved] = useState<{ id: string; url: string | null } | null>(null);
  useEffect(() => {
    if (key === null) return;
    let live = true;
    resolve(key).then(
      (url) => {
        if (live) setResolved({ id: key, url });
      },
      () => {
        if (live) setResolved({ id: key, url: null });
      },
    );
    return () => {
      live = false;
    };
  }, [key, resolve]);
  if (source.kind === "bad") return { state: "broken" };
  if (source.kind === "url") return { state: "ready", url: source.url };
  if (resolved?.id !== source.id) return { state: "loading" };
  return resolved.url === null ? { state: "broken" } : { state: "ready", url: resolved.url };
}

function ImageView({ node, selected, extension }: ReactNodeViewProps) {
  const { t } = useTranslation("editor");
  const options = extension.options as MediaImageOptions;
  const attrs = node.attrs as { src?: unknown; alt?: unknown; title?: unknown };
  const shown = useImageUrl(attrs.src, options.resolveMedia);
  const [failed, setFailed] = useState<string | null>(null);
  const alt = typeof attrs.alt === "string" ? attrs.alt : "";
  const src = typeof attrs.src === "string" ? attrs.src : "";
  const title = typeof attrs.title === "string" ? attrs.title : undefined;
  const broken = shown.state === "broken" || (shown.state === "ready" && failed === shown.url);

  return (
    <NodeViewWrapper
      as="figure"
      data-slot="editor-image"
      data-state={broken ? "broken" : shown.state}
      className={cn("editor-figure", selected && "is-selected")}
    >
      {broken ? (
        <div
          className="editor-image-broken"
          role="img"
          aria-label={t("image.brokenLabel", { alt })}
        >
          <ImageOffIcon aria-hidden="true" className="size-5 shrink-0" />
          <div className="min-w-0">
            <p className="font-medium text-foreground">{t("image.broken")}</p>
            <p className="truncate font-mono text-sm" title={src}>
              {src}
            </p>
          </div>
        </div>
      ) : shown.state === "loading" ? (
        <div className="editor-image-loading" aria-label={t("image.loading")} role="img" />
      ) : (
        <img
          src={shown.url}
          alt={alt}
          {...(title ? { title } : {})}
          draggable={false}
          onError={() => setFailed(shown.url)}
        />
      )}
    </NodeViewWrapper>
  );
}

/** The core `image` node with the media-resolving node view. */
export const MediaImage = Image.extend<MediaImageOptions>({
  addOptions() {
    return {
      ...(this.parent?.() as ImageOptions),
      resolveMedia: () => Promise.resolve(null),
    };
  },
  addNodeView() {
    return ReactNodeViewRenderer(ImageView);
  },
});
