// How the item may look as a search result: the SEO title (else the item's title), the
// address, the SEO description (else the excerpt), cut with an ellipsis the way a result
// snippet is. A sketch for the writer, not a promise about any search engine.

import { GlobeIcon } from "lucide-react";
import { useTranslation } from "react-i18next";

export interface SearchPreviewProps {
  title: string;
  /** The full address (`https://example.com/2026/10/post/`), or `null` before the first save. */
  url: string | null;
  description: string;
  siteName: string;
}

/** `https://example.com/a/b/` → `example.com › a › b`. */
function breadcrumbOf(url: string): string {
  try {
    const u = new URL(url);
    const parts = u.pathname.split("/").filter(Boolean).map(decodeURIComponent);
    return [u.host, ...parts].join(" › ");
  } catch {
    return url;
  }
}

export function SearchPreview({ title, url, description, siteName }: SearchPreviewProps) {
  const { t } = useTranslation("editor");
  return (
    <figure
      data-slot="search-preview"
      aria-label={t("seo.preview")}
      className="m-0 flex flex-col gap-1 rounded-lg border bg-card px-3.5 py-3"
    >
      <div className="flex min-w-0 items-center gap-2">
        <span
          aria-hidden="true"
          className="flex size-6 shrink-0 items-center justify-center rounded-full border bg-muted text-muted-foreground"
        >
          <GlobeIcon className="size-3.5" />
        </span>
        <div className="flex min-w-0 flex-col">
          <span className="truncate text-xs text-foreground" title={siteName}>
            {siteName}
          </span>
          <span className="truncate text-xs text-muted-foreground" title={url ?? undefined}>
            {url ? breadcrumbOf(url) : t("seo.previewNoUrl")}
          </span>
        </div>
      </div>
      <p
        data-slot="search-preview-title"
        className="line-clamp-1 text-base leading-snug font-medium break-all text-info"
        title={title}
      >
        {title}
      </p>
      <p
        data-slot="search-preview-description"
        className="line-clamp-2 text-sm leading-5 text-muted-foreground"
        title={description || undefined}
      >
        {description || t("seo.previewNoDescription")}
      </p>
    </figure>
  );
}
